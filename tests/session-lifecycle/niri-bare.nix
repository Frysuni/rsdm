{ command }:
''
    with subtest("bare Niri finalizes its own endpoints and closes a GTK application"):
        import shlex
        import tomllib

        machine.succeed("rm -f /run/user/1000/clean-closes /run/user/1000/clean-term")
        machine.send_chars("rsdm session start -- niri -- rsdm session finalize > /tmp/rsdm-niri-bare.log 2>&1; touch /tmp/rsdm-niri-bare-exited\n")
        try:
            machine.wait_until_succeeds(control + "rsdm session status | grep '^managed: running '", timeout=60)
        except Exception:
            print(machine.succeed("cat /tmp/rsdm-niri-bare.log; journalctl --no-pager -n 100"))
            raise

        environment = machine.succeed(user + "systemctl --user show-environment")
        values = dict(line.split("=", 1) for line in environment.splitlines())
        generation = values["RSDM_SESSION_GENERATION"]
        directory = "/run/user/1000/rsdm/sessions/" + generation
        record = tomllib.loads(machine.succeed("cat " + directory + "/session.toml"))
        assert record["provider"] == "managed"
        assert record["owns_targets"]
        assert record["compositor_unit"].startswith("session-rsdm-")
        assert record["compositor_invocation"]
        assert dict(record["exported_environment"])["NIRI_SOCKET"] == values["NIRI_SOCKET"]
        assert dict(record["exported_environment"])["WAYLAND_DISPLAY"] == values["WAYLAND_DISPLAY"]
        machine.fail(user + "systemctl --user is-active --quiet niri.service")
        compositor = record["compositor_unit"]
        compositor_state = machine.succeed(user + "systemctl --user show " + compositor + " -p MainPID -p InvocationID")
        session = control + "RSDM_SESSION_GENERATION=" + generation + " WAYLAND_DISPLAY=" + shlex.quote(values["WAYLAND_DISPLAY"]) + " NIRI_SOCKET=" + shlex.quote(values["NIRI_SOCKET"]) + " "
        machine.succeed(session + "niri msg version")

        machine.succeed(session + "rsdm app --shutdown-timeout 5 --on-timeout cancel -- ${command} clean")
        machine.wait_until_succeeds(session + "niri msg --json windows | grep 'clean 1'")
        application = tomllib.loads(machine.succeed("cat " + directory + "/apps/*.toml"))
        machine.succeed(user + "systemctl --user stop " + application["unit"])
        machine.wait_until_succeeds("test -e " + directory + "/results/" + application["unit"] + ".toml")
        saved = tomllib.loads(machine.succeed("cat " + directory + "/results/" + application["unit"] + ".toml"))
        assert saved["dispatch"]["method"] == "window_close"
        assert saved["dispatch"]["state"] == "sent"
        assert saved["preparation"]["outcome"] == "clean"
        assert not saved["preparation"]["escalated"]
        assert machine.succeed("cat /run/user/1000/clean-closes").splitlines() == ["close"]
        machine.fail("test -e /run/user/1000/clean-term")
        assert machine.succeed(user + "systemctl --user show " + compositor + " -p MainPID -p InvocationID") == compositor_state

        machine.succeed(session + "rsdm session stop")
        machine.wait_until_fails(user + "systemctl --user is-active --quiet " + compositor)
        machine.wait_until_fails(user + "systemctl --user is-active --quiet graphical-session.target")
        machine.wait_for_file("/tmp/rsdm-niri-bare-exited")
''
