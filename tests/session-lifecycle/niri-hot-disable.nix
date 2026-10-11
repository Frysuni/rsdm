{ application }:
''
    with subtest("hot disable preserves the real native compositor and login until WM exit"):
        import shlex
        import tomllib

        machine.succeed("rm -f /run/user/1000/niri-app-ready /run/user/1000/niri-app-saved")
        machine.send_chars("rsdm session start -- niri-session > /tmp/rsdm-niri-hot.log 2>&1; touch /tmp/rsdm-niri-hot-exited\n")
        machine.wait_until_succeeds(user + "rsdm session status | grep '^niri: running '", timeout=60)
        environment = machine.succeed(user + "systemctl --user show-environment")
        values = dict(line.split("=", 1) for line in environment.splitlines())
        generation = values["RSDM_SESSION_GENERATION"]
        directory = "/run/user/1000/rsdm/sessions/" + generation
        session = user + "RSDM_SESSION_GENERATION=" + generation + " "
        machine.succeed(session + "rsdm app -- ${application}")
        machine.wait_for_file("/run/user/1000/niri-app-ready")
        record = tomllib.loads(machine.succeed("cat " + directory + "/session.toml"))
        wm_state = machine.succeed(user + "systemctl --user show niri.service -p MainPID -p InvocationID")
        app_pid = machine.succeed("cat /run/user/1000/niri-app-ready").strip()
        machine.succeed("printf '[session_manager]\\nenabled = false\\n' > /etc/rsdm-toggle.toml; mv /etc/rsdm-toggle.toml /etc/rsdm.toml")
        machine.wait_until_succeeds("grep -q '^disabled = true$' " + directory + "/session.toml")
        machine.wait_until_fails(user + "busctl --user list | grep org.rsdm.Session1")
        machine.wait_until_fails("systemd-inhibit --list --no-pager | grep 'Saving registered graphical applications'")
        authority = dict(record["exported_environment"]).get("ICEAUTHORITY")
        if authority:
            machine.wait_until_fails("test -e " + shlex.quote(authority))
        machine.succeed("printf '[session_manager]\\nenabled = true\\n' > /etc/rsdm-toggle.toml; mv /etc/rsdm-toggle.toml /etc/rsdm.toml")
        machine.succeed(session + "rsdm session cleanup --generation " + generation)
        machine.sleep(2)
        assert machine.succeed(user + "systemctl --user show niri.service -p MainPID -p InvocationID") == wm_state
        machine.succeed("kill -0 " + app_pid)
        machine.succeed("loginctl show-session " + record["identity"]["login_session_id"])
        machine.fail("test -e /tmp/rsdm-niri-hot-exited")
        machine.fail("test -e /run/user/1000/niri-app-saved")
        machine.succeed(user + "systemctl --user stop niri.service")
        machine.wait_for_file("/tmp/rsdm-niri-hot-exited")
''
