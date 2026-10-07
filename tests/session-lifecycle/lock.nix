{ pkgs, rsdm }:

pkgs.testers.runNixOSTest {
  name = "rsdm-lock-power";
  extraPythonPackages = packages: [ packages.pillow ];
  globalTimeout = 120;
  nodes.machine = { ... }: {
    virtualisation.memorySize = 2048;
    virtualisation.qemu.options = [ "-vga none -device virtio-gpu-pci" ];
    users.users.alice = { isNormalUser = true; uid = 1000; group = "alice"; };
    users.groups.alice = {};
    services.getty.autologinUser = "alice";
    programs.sway.enable = true;
    environment.systemPackages = [ rsdm ];
    environment.variables = { SWAYSOCK = "/tmp/sway-ipc.sock"; WLR_RENDERER = "pixman"; };
    environment.etc."sway/config".text = ''
      output * bg #ff00ff solid_color
    '';
    environment.etc."rsdm.toml".text = "[lock]\nenable = true\n";
    programs.bash.loginShellInit = ''
      if [ "$(tty)" = /dev/tty1 ]; then
        sway > /tmp/rsdm-sway.log 2>&1
      fi
    '';
    security.polkit.enable = true;
    security.polkit.extraConfig = ''
      polkit.addRule(function(action, subject) {
        if (subject.user == "alice" && action.id.indexOf("org.freedesktop.login1.power-off") == 0) {
          return polkit.Result.NO;
        }
      });
    '';
  };

  testScript = ''
    import json
    from PIL import Image

    start_all()
    machine.wait_for_unit("multi-user.target")
    machine.wait_for_file("/tmp/sway-ipc.sock")
    user = "runuser -u alice -- env XDG_RUNTIME_DIR=/run/user/1000 DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus SWAYSOCK=/tmp/sway-ipc.sock "
    outputs = json.loads(machine.succeed(user + "swaymsg -t get_outputs --raw"))
    assert outputs and all(output["active"] for output in outputs)
    machine.screenshot("desktop-before-lock")
    desktop = Image.open(machine.out_dir / "desktop-before-lock.png").convert("RGB")
    assert desktop.getpixel((desktop.width // 2, desktop.height // 2)) == (255, 0, 255)

    machine.succeed(user + "systemd-run --user --unit=rsdm-test-lock --collect --setenv=WAYLAND_DISPLAY=wayland-1 -- rsdm lock")
    machine.wait_until_succeeds(user + "rsdm status | grep 'active lock: pid '")
    before = machine.succeed(user + "rsdm status | grep 'active lock: pid '")
    machine.send_key("f12", delay=0.2)
    machine.send_key("f12", delay=0.2)
    try:
        machine.wait_until_succeeds("journalctl --no-pager -b | grep 'rsdm_lock::wayland::power: power request failed'", timeout=10)
    except Exception:
        print(machine.succeed("journalctl --no-pager -b -n 80"))
        print(machine.succeed("cat /tmp/rsdm-sway.log"))
        raise
    assert machine.succeed(user + "rsdm status | grep 'active lock: pid '") == before
    machine.succeed(user + "systemctl --user is-active --quiet rsdm-test-lock.service")
    machine.screenshot("lock-after-power-denial")
    locked = Image.open(machine.out_dir / "lock-after-power-denial.png").convert("RGB")
    assert (255, 0, 255) not in locked.getdata(), "desktop background leaked through the lock"
    machine.succeed("rsdm unlock --uid 1000")
    machine.wait_until_succeeds(user + "rsdm status | grep 'active lock: none '")
    machine.screenshot("desktop-after-emergency-unlock")
    unlocked = Image.open(machine.out_dir / "desktop-after-emergency-unlock.png").convert("RGB")
    assert unlocked.getpixel((unlocked.width // 2, unlocked.height // 2)) == (255, 0, 255)
  '';
}
