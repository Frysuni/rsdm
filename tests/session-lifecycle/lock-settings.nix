{ pkgs }:

let
  slowSave = pkgs.runCommand "test-slow-lock-save.so" {
    nativeBuildInputs = [ pkgs.stdenv.cc ];
  } ''
    cc -shared -fPIC -Wall -Wextra -Werror ${./slow-lock-save.c} -ldl -o "$out"
  '';
in
''
    with subtest("Lock saves personal settings without blocking emergency unlock"):
        import tomllib

        path = "/home/alice/.config/rsdm/lock.toml"
        system_config = machine.succeed("cat /etc/rsdm.toml")
        machine.succeed(user + "install -dm700 /home/alice/.config/rsdm")
        machine.succeed(user + "sh -c 'printf \"[lock]\\nsize = 1\\n\" > " + path + "; chmod 600 " + path + "'")

        def increase_size():
            machine.send_key("f1")
            for _ in range(7):
                machine.send_key("down")
            machine.send_key("ret")
            machine.send_key("down")
            machine.send_key("ret")

        def save_settings():
            machine.send_key("f1")
            for _ in range(9):
                machine.send_key("down")
            machine.send_key("ret")

        machine.succeed(user + "systemd-run --user --unit=rsdm-test-save-lock --collect --setenv=WAYLAND_DISPLAY=wayland-1 -- rsdm lock")
        machine.wait_until_succeeds(user + "rsdm status | grep 'active lock: pid '")
        increase_size()
        save_settings()
        machine.wait_until_succeeds("grep -qx 'size = 2' " + path)
        saved = machine.succeed("cat " + path)
        data = tomllib.loads(saved)
        assert set(data) == {"lock"}, data
        assert set(data["lock"]) == {"size", "size_reset", "secondary_output", "design"}, data
        assert machine.succeed("stat -c %a " + path).strip() == "600"
        machine.succeed("rsdm unlock --uid 1000")
        machine.wait_until_fails(user + "systemctl --user is-active --quiet rsdm-test-save-lock.service")

        machine.succeed(user + "systemd-run --user --unit=rsdm-test-save-blocked --collect --setenv=WAYLAND_DISPLAY=wayland-1 --setenv=LD_PRELOAD=${slowSave} -- rsdm lock")
        machine.wait_until_succeeds(user + "rsdm status | grep 'active lock: pid '")
        increase_size()
        save_settings()
        machine.wait_for_file("/run/user/1000/rsdm-save-blocked", timeout=10)
        machine.send_key("ret")
        machine.succeed("rsdm unlock --uid 1000")
        machine.wait_until_fails(user + "systemctl --user is-active --quiet rsdm-test-save-blocked.service")
        assert machine.succeed("cat " + path) == saved
        assert machine.succeed("find /home/alice/.config/rsdm -name '*.tmp' | wc -l").strip() == "1"
        machine.succeed("touch /run/user/1000/rsdm-save-continue")

        machine.succeed(user + "systemd-run --user --unit=rsdm-test-save-failed --collect --setenv=WAYLAND_DISPLAY=wayland-1 -- rsdm lock")
        machine.wait_until_succeeds(user + "rsdm status | grep 'active lock: pid '")
        machine.succeed("mv " + path + " " + path + ".previous; mkdir " + path)
        save_settings()
        machine.wait_until_succeeds(user + "journalctl --user --no-pager -u rsdm-test-save-failed | grep 'could not save runtime lock settings'", timeout=10)
        assert machine.succeed("cat " + path + ".previous") == saved
        assert machine.succeed("cat /etc/rsdm.toml") == system_config
        machine.send_key("esc")
        machine.send_chars("test-password")
        machine.send_key("ret")
        machine.wait_until_fails(user + "systemctl --user is-active --quiet rsdm-test-save-failed.service")
        machine.wait_until_succeeds(user + "rsdm status | grep 'active lock: none '")
        machine.screenshot("desktop-after-settings-save-failure")
        unlocked = Image.open(machine.out_dir / "desktop-after-settings-save-failure.png").convert("RGB")
        assert unlocked.getpixel((unlocked.width // 2, unlocked.height // 2)) == (255, 0, 255)
''
