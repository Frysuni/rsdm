{ application, stubborn }:
''
    with subtest("hot disable detaches infrastructure while PAM, WM and apps remain alive"):
        for preparing in [False, True]:
            session = start_session()
            generation = session.split("RSDM_SESSION_GENERATION=")[1].split()[0]
            directory = runtime + "/rsdm/sessions/" + generation
            if preparing:
                machine.succeed(session + "rsdm app --shutdown-timeout 1 -- ${stubborn}")
                machine.wait_for_file(runtime + "/stubborn-ready")
                machine.succeed(session + "sh -c 'rsdm session stop > /run/user/1000/hot-stop-result 2>&1 &'")
                machine.wait_until_succeeds(session + "rsdm session status | grep ': preparing '")
            else:
                machine.succeed(session + "rsdm app -- ${application} hot-disable")
                machine.wait_for_file(runtime + "/app-hot-disable-ready")
            record = tomllib.loads(machine.succeed("cat " + directory + "/session.toml"))
            app = machine.succeed("basename -s .toml " + directory + "/apps/*.toml").strip()
            app_state = machine.succeed(user + "systemctl --user show " + app + " -p MainPID -p InvocationID")
            wm_state = machine.succeed(user + "systemctl --user show " + record["compositor_unit"] + " -p MainPID -p InvocationID")
            launcher_state = machine.succeed("systemctl show rsdm-test-session -p MainPID -p InvocationID")
            machine.succeed("printf '[session_manager]\\nenabled = false\\n' > /etc/rsdm-toggle.toml; mv /etc/rsdm-toggle.toml /etc/rsdm.toml")
            machine.wait_until_succeeds("grep -q '^disabled = true$' " + directory + "/session.toml")
            machine.wait_until_fails(user + "busctl --user list | grep org.rsdm.Session1")
            machine.wait_until_fails("systemd-inhibit --list --no-pager | grep 'Saving registered graphical applications'")
            authority = dict(record["exported_environment"]).get("ICEAUTHORITY")
            if authority:
                machine.wait_until_fails("test -e " + shlex.quote(authority))
            if preparing:
                machine.wait_until_succeeds("grep -q unknown " + runtime + "/hot-stop-result")
            saved = machine.succeed("cat " + directory + "/session.toml")
            registered = machine.succeed("cat " + directory + "/apps/" + app + ".toml")
            machine.succeed("printf '[session_manager]\\nenabled = true\\n' > /etc/rsdm-toggle.toml; mv /etc/rsdm-toggle.toml /etc/rsdm.toml")
            machine.succeed(session + "rsdm session app-stop --generation " + generation + " --unit " + app)
            machine.succeed(session + "rsdm session cleanup --generation " + generation)
            machine.sleep(6)
            assert machine.succeed("cat " + directory + "/session.toml") == saved
            assert machine.succeed("cat " + directory + "/apps/" + app + ".toml") == registered
            assert machine.succeed(user + "systemctl --user show " + app + " -p MainPID -p InvocationID") == app_state
            assert machine.succeed(user + "systemctl --user show " + record["compositor_unit"] + " -p MainPID -p InvocationID") == wm_state
            assert machine.succeed("systemctl show rsdm-test-session -p MainPID -p InvocationID") == launcher_state
            machine.succeed("systemctl is-active --quiet rsdm-test-session")
            machine.succeed("loginctl show-session " + record["identity"]["login_session_id"])
            machine.succeed("test -e " + runtime + "/display-alive")
            machine.fail("test -e " + runtime + "/app-hot-disable")
            assert ("RSDM_SESSION_GENERATION=" + generation) in machine.succeed(user + "systemctl --user show-environment")
            machine.fail(user + "busctl --user list | grep org.rsdm.Session1")
            # The test now terminates the WM explicitly; only this exit may
            # release the launcher and its PAM session.
            machine.succeed(user + "systemctl --user stop graphical-session.target")
            machine.wait_until_fails("test -e " + runtime + "/display-alive")
            machine.wait_until_fails("systemctl is-active --quiet rsdm-test-session")
    machine.fail("journalctl --no-pager -b | grep 'XSMP error:'")
''
