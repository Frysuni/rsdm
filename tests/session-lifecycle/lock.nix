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
  literalLocker = pkgs.writeShellScript "test-idle-argv" ''
    printf '%s\n' "$@" > "$XDG_RUNTIME_DIR/rsdm-idle-argv"
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
    security.pam.services.rsdm-lock-cancel-test.text = ''
      auth optional ${pkgs.linux-pam}/lib/security/pam_faildelay.so delay=30000000
      auth required ${pkgs.linux-pam}/lib/security/pam_unix.so
      account required ${pkgs.linux-pam}/lib/security/pam_unix.so
    '';
    environment.etc."rsdm-lock-cancel.toml".text = ''
      [lock]
      enable = true
      pam_service = "rsdm-lock-cancel-test"
      [security]
      max_failed_attempts = 1
      failure_delay_ms = 60000
    '';
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
    environment.etc."rsdm-idle-update.toml".text = ''
      [idle]
      enable = true
      timeout = 3
      [lock]
      enable = true
    '';
    environment.etc."rsdm-idle-literal.toml".text = ''
      [idle]
      enable = true
      timeout = 1
      lock_command = ["${literalLocker}", "$HOME", "$$", "two words"]
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

    with subtest("service-managed Idle preserves literal locker arguments"):
        expected = "$HOME\n$$\ntwo words\n"
        machine.succeed(user + "${literalLocker} '$HOME' '$$' 'two words'")
        assert machine.succeed("cat /run/user/1000/rsdm-idle-argv") == expected
        machine.succeed("rm /run/user/1000/rsdm-idle-argv")
        machine.send_key("spc")
        machine.succeed(user + "systemd-run --user --unit=rsdm-test-idle-literal --collect --setenv=WAYLAND_DISPLAY=wayland-1 -- rsdm --config /etc/rsdm-idle-literal.toml idle")
        machine.wait_for_file("/run/user/1000/rsdm-idle-argv", timeout=10)
        machine.succeed(user + "systemctl --user stop rsdm-test-idle-literal.service")
        actual = machine.succeed("cat /run/user/1000/rsdm-idle-argv")
        assert actual == expected, (expected, actual)

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

    with subtest("PAM rejects a wrong password and unlocks after executable replacement"):
        machine.succeed("install -Dm755 ${rsdm}/bin/rsdm /run/test-lock/rsdm")
        machine.succeed(user + "systemd-run --user --unit=rsdm-test-auth-lock --collect --setenv=WAYLAND_DISPLAY=wayland-1 -- /run/test-lock/rsdm lock")
        machine.wait_until_succeeds(user + "rsdm status | grep 'active lock: pid '")
        before = machine.succeed(user + "rsdm status | grep 'active lock: pid '")
        machine.send_chars("wrong-password")
        machine.send_key("ret")
        machine.wait_until_succeeds("journalctl --no-pager -b | grep 'pam_unix(rsdm-lock:auth): authentication failure'")
        machine.wait_until_fails("pgrep -u alice -f ' lock-auth-helper --fd '")
        assert machine.succeed(user + "rsdm status | grep 'active lock: pid '") == before
        machine.sleep(1)
        machine.succeed("install -m755 ${rsdm}/bin/rsdm /run/test-lock/rsdm.next; mv -f /run/test-lock/rsdm.next /run/test-lock/rsdm")
        machine.succeed(user + "sh -c 'readlink /proc/$(systemctl --user show rsdm-test-auth-lock -p MainPID --value)/exe' | grep -F '(deleted)'")
        machine.send_chars("test-password")
        machine.send_key("ret")
        machine.wait_until_succeeds(user + "rsdm status | grep 'active lock: none '")
        machine.wait_until_fails(user + "systemctl --user is-active --quiet rsdm-test-auth-lock.service")
        machine.screenshot("desktop-after-pam-unlock")
        unlocked = Image.open(machine.out_dir / "desktop-after-pam-unlock.png").convert("RGB")
        assert unlocked.getpixel((unlocked.width // 2, unlocked.height // 2)) == (255, 0, 255)

    with subtest("Cancelling a submitted password cannot bypass the attempt limit"):
        helper = "pgrep -u alice -f ' lock-auth-helper --fd '"
        machine.succeed(user + "systemd-run --user --unit=rsdm-test-cancel-lock --collect --setenv=WAYLAND_DISPLAY=wayland-1 -- ${rsdm}/bin/rsdm --config /etc/rsdm-lock-cancel.toml lock")
        machine.wait_until_succeeds(user + "rsdm status | grep 'active lock: pid '")
        machine.send_chars("cancelled-wrong-password")
        machine.send_key("ret")
        machine.wait_until_succeeds("journalctl --no-pager -b | grep -F 'pam_unix(rsdm-lock-cancel-test:auth): authentication failure'")
        machine.succeed(helper)
        machine.send_key("esc")
        machine.wait_until_fails(helper)
        machine.sleep(1)
        machine.send_chars("another-wrong-password")
        machine.send_key("ret")
        machine.sleep(1)
        try:
            machine.fail(helper)
            machine.succeed("journalctl --no-pager -b | grep -F 'pam_unix(rsdm-lock-cancel-test:auth): authentication failure' | test $(wc -l) -eq 1")
        except Exception:
            print(machine.succeed(user + "journalctl --user --no-pager -u rsdm-test-cancel-lock"))
            raise
        machine.succeed("rsdm unlock --uid 1000")
        machine.wait_until_succeeds(user + "rsdm status | grep 'active lock: none '")

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

    with subtest("Idle starts its default locker after executable replacement"):
        machine.send_key("spc")
        machine.succeed("install -Dm755 ${rsdm}/bin/rsdm /run/test-idle/rsdm")
        machine.succeed(user + "systemd-run --user --unit=rsdm-test-idle-update --collect --setenv=WAYLAND_DISPLAY=wayland-1 -- /run/test-idle/rsdm --config /etc/rsdm-idle-update.toml idle")
        machine.wait_until_succeeds(user + "journalctl --user --no-pager -u rsdm-test-idle-update | grep 'idle monitoring started'")
        machine.succeed("install -m755 ${rsdm}/bin/rsdm /run/test-idle/rsdm.next; mv -f /run/test-idle/rsdm.next /run/test-idle/rsdm")
        machine.succeed(user + "sh -c 'readlink /proc/$(systemctl --user show rsdm-test-idle-update -p MainPID --value)/exe' | grep -F '(deleted)'")
        machine.send_key("spc")
        machine.wait_until_succeeds(user + "rsdm status | grep 'active lock: pid '", timeout=12)
        machine.wait_until_succeeds(user + "journalctl --user --no-pager -u rsdm-test-idle-update | grep 'idle locker is active'")
        machine.succeed(user + "systemctl --user stop rsdm-test-idle-update.service")
        machine.succeed(user + "rsdm status | grep 'active lock: pid '")
        machine.send_chars("test-password")
        machine.send_key("ret")
        machine.wait_until_succeeds(user + "rsdm status | grep 'active lock: none '")

    with subtest("an unavailable state file cannot make Idle kill a confirmed locker"):
        machine.succeed("mkdir /run/user/1000/rsdm/lock.toml")
        machine.send_key("spc")
        machine.succeed(user + "systemd-run --user --unit=rsdm-test-idle-state --collect --setenv=WAYLAND_DISPLAY=wayland-1 -- rsdm --config /etc/rsdm-idle-update.toml idle")
        machine.wait_until_succeeds("journalctl --no-pager -b | grep 'lock runtime state is unavailable'")
        locker = machine.succeed("pgrep -u alice -f '^/proc/[0-9]+/exe lock --config /etc/rsdm-idle-update.toml( |$)'").strip()
        assert locker.isdecimal(), locker
        machine.sleep(11)
        try:
            machine.succeed("kill -0 " + locker)
        except Exception:
            print(machine.succeed(user + "journalctl --user --no-pager -u rsdm-test-idle-state"))
            raise
        machine.succeed(user + "systemctl --user stop rsdm-test-idle-state.service")
        machine.send_chars("test-password")
        machine.send_key("ret")
        machine.wait_until_fails("kill -0 " + locker)
        machine.succeed("rmdir /run/user/1000/rsdm/lock.toml")
        machine.screenshot("desktop-after-unpublished-lock")
        unlocked = Image.open(machine.out_dir / "desktop-after-unpublished-lock.png").convert("RGB")
        assert unlocked.getpixel((unlocked.width // 2, unlocked.height // 2)) == (255, 0, 255)

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
  '' + import ./lock-settings.nix { inherit pkgs; };
}
