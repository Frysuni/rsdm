{ pkgs, rsdm }:

let
  python = pkgs.python3.withPackages (packages: [ packages.dbus-python packages.pygobject3 ]);
  flakyLocker = pkgs.writeShellScript "test-idle-locker" ''
    if [ ! -e "$XDG_RUNTIME_DIR/rsdm-idle-first-attempt" ]; then
      touch "$XDG_RUNTIME_DIR/rsdm-idle-first-attempt"
      exit 1
    fi
    exec ${rsdm}/bin/rsdm lock
  '';
in
pkgs.testers.runNixOSTest {
  name = "rsdm-lock-power";
  extraPythonPackages = packages: [ packages.pillow ];
  globalTimeout = 120;
  nodes.machine = { ... }: {
    virtualisation.memorySize = 2048;
    virtualisation.qemu.options = [ "-vga none -device virtio-gpu-pci" ];
    users.users.alice = {
      isNormalUser = true;
      uid = 1000;
      group = "alice";
      password = "test-password";
    };
    users.groups.alice = {};
    security.pam.services.rsdm-lock = {};
    services.getty.autologinUser = "alice";
    programs.sway.enable = true;
    environment.systemPackages = [ rsdm pkgs.dbus ];
    environment.variables = { SWAYSOCK = "/tmp/sway-ipc.sock"; WLR_RENDERER = "pixman"; };
    environment.etc."sway/config".text = ''
      output * bg #ff00ff solid_color
    '';
    environment.etc."rsdm.toml".text = "[lock]\nenable = true\n";
    environment.etc."rsdm-idle-retry.toml".text = ''
      [idle]
      enable = true
      timeout = 1
      lock_command = ["${flakyLocker}"]
      [lock]
      enable = true
    '';
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
    machine.wait_until_succeeds(user + "swaymsg -t get_outputs --raw")
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

    with subtest("PAM rejects a wrong password and unlocks with the correct password"):
        machine.succeed(user + "systemd-run --user --unit=rsdm-test-auth-lock --collect --setenv=WAYLAND_DISPLAY=wayland-1 -- rsdm lock")
        machine.wait_until_succeeds(user + "rsdm status | grep 'active lock: pid '")
        before = machine.succeed(user + "rsdm status | grep 'active lock: pid '")
        machine.send_chars("wrong-password")
        machine.send_key("ret")
        machine.wait_until_succeeds("journalctl --no-pager -b | grep 'pam_unix(rsdm-lock:auth): authentication failure'")
        machine.wait_until_fails("pgrep -u alice -f 'rsdm lock-auth-helper'")
        assert machine.succeed(user + "rsdm status | grep 'active lock: pid '") == before
        machine.sleep(1)
        machine.send_chars("test-password")
        machine.send_key("ret")
        machine.wait_until_succeeds(user + "rsdm status | grep 'active lock: none '")
        machine.wait_until_fails(user + "systemctl --user is-active --quiet rsdm-test-auth-lock.service")
        machine.screenshot("desktop-after-pam-unlock")
        unlocked = Image.open(machine.out_dir / "desktop-after-pam-unlock.png").convert("RGB")
        assert unlocked.getpixel((unlocked.width // 2, unlocked.height // 2)) == (255, 0, 255)

    with subtest("Idle retries a failed locker without another activity transition"):
        # Send no input after monitor startup: recovery must use the pending
        # retry rather than a second transition from activity to inactivity.
        machine.send_key("spc")
        machine.succeed(user + "systemd-run --user --unit=rsdm-test-idle --collect --setenv=WAYLAND_DISPLAY=wayland-1 -- rsdm --config /etc/rsdm-idle-retry.toml idle")
        machine.wait_until_succeeds(user + "journalctl --user --no-pager -u rsdm-test-idle | grep 'idle monitoring started'")
        machine.wait_for_file("/run/user/1000/rsdm-idle-first-attempt")
        machine.wait_until_succeeds(user + "journalctl --user --no-pager -u rsdm-test-idle | grep 'idle locker failed'")
        machine.wait_until_succeeds(user + "rsdm status | grep 'active lock: pid '")
        before = machine.succeed(user + "rsdm status | grep 'active lock: pid '")
        machine.succeed(user + "systemctl --user stop rsdm-test-idle.service")
        assert machine.succeed(user + "rsdm status | grep 'active lock: pid '") == before
        machine.send_chars("test-password")
        machine.send_key("ret")
        machine.wait_until_succeeds(user + "rsdm status | grep 'active lock: none '")

    with subtest("disabled SM with a stale generation still sends Lock power requests to logind"):
        machine.succeed("printf '[session_manager]\\nenabled = false\\n[lock]\\nenable = true\\n' > /run/rsdm-lock-disabled.toml")
        bus = "unix:path=/run/user/1000/lock-power-bus"
        machine.succeed(user + "dbus-daemon --session --fork --nopidfile --address=" + bus)
        machine.succeed(user + "systemd-run --user --unit=rsdm-test-lock-logind --setenv=DBUS_SESSION_BUS_ADDRESS=" + bus + " -- ${python}/bin/python3 ${./power-manager.py}")
        machine.wait_for_file("/run/user/1000/power-manager-ready")
        machine.succeed("touch /run/user/1000/power-deny-request")
        machine.succeed(user + "systemd-run --user --unit=rsdm-test-disabled-lock --collect --setenv=WAYLAND_DISPLAY=wayland-1 --setenv=RSDM_SESSION_GENERATION=invalid --setenv=DBUS_SYSTEM_BUS_ADDRESS=" + bus + " -- rsdm --config /run/rsdm-lock-disabled.toml lock")
        machine.wait_until_succeeds(user + "rsdm status | grep 'active lock: pid '")
        before = machine.succeed(user + "rsdm status | grep 'active lock: pid '")
        machine.send_key("f12", delay=0.2)
        machine.send_key("f12", delay=0.2)
        machine.wait_for_file("/run/user/1000/power-requests")
        assert machine.succeed("cat /run/user/1000/power-requests") == "1000\n"
        machine.wait_until_succeeds(user + "journalctl --user --no-pager -u rsdm-test-disabled-lock | grep 'power request denied'")
        assert machine.succeed(user + "rsdm status | grep 'active lock: pid '") == before
        machine.succeed(user + "systemctl --user is-active --quiet rsdm-test-disabled-lock.service")
        machine.screenshot("disabled-sm-lock-after-power-denial")
        locked = Image.open(machine.out_dir / "disabled-sm-lock-after-power-denial.png").convert("RGB")
        assert (255, 0, 255) not in locked.getdata(), "desktop background leaked through the lock"
        machine.succeed("rsdm unlock --uid 1000")
        machine.wait_until_succeeds(user + "rsdm status | grep 'active lock: none '")
  '';
}
