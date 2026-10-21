{ pkgs }:

let
  slowHook = pkgs.writeShellScript "test-slow-on-lock" ''
    set -eu
    touch "$XDG_RUNTIME_DIR/slow-on-lock-started"
    while ! test -e "$XDG_RUNTIME_DIR/slow-on-lock-release"; do sleep 0.05; done
    touch "$XDG_RUNTIME_DIR/slow-on-lock-finished"
  '';
in
''
    with subtest("Slow on_lock hooks cannot hide the next idle threshold"):
        import shlex

        config = "[idle]\nenable = true\ntimeout = 1\n"
        config += 'on_lock = [["${slowHook}"]]\n'
        config += 'on_unlock = [["touch", "/run/user/1000/slow-on-lock-unlocked"]]\n'
        config += "[lock]\nenable = true\n"
        machine.succeed("printf %s " + shlex.quote(config) + " > /run/rsdm-idle-slow-hook.toml")
        machine.send_key("spc")
        machine.succeed(user + "systemd-run --user --unit=rsdm-test-slow-hook --collect --setenv=WAYLAND_DISPLAY=wayland-1 -- rsdm --config /run/rsdm-idle-slow-hook.toml idle")
        machine.wait_until_succeeds(user + "rsdm status | grep 'active lock: pid '")
        first = machine.succeed(user + "rsdm status | grep 'active lock: pid '")
        machine.wait_for_file("/run/user/1000/slow-on-lock-started", timeout=5)
        machine.send_chars("test-password")
        machine.send_key("ret")
        machine.wait_until_succeeds(user + "rsdm status | grep 'active lock: pid ' | grep -vF " + shlex.quote(first.strip()), timeout=5)
        machine.fail("test -e /run/user/1000/slow-on-lock-finished")
        machine.fail("test -e /run/user/1000/slow-on-lock-unlocked")
        machine.succeed("touch /run/user/1000/slow-on-lock-release")
        machine.wait_for_file("/run/user/1000/slow-on-lock-unlocked", timeout=5)
        machine.succeed("test -e /run/user/1000/slow-on-lock-finished")
        machine.succeed(user + "systemctl --user stop rsdm-test-slow-hook.service")
        machine.succeed("rsdm unlock --uid 1000")
        machine.wait_until_succeeds(user + "rsdm status | grep 'active lock: none '")
''
