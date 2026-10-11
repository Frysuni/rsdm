{ application }:
''
    with subtest("record publication failure still retires the generation and detaches management"):
        session = start_session()
        generation = session.split("RSDM_SESSION_GENERATION=")[1].split()[0]
        directory = runtime + "/rsdm/sessions/" + generation
        machine.succeed(session + "rsdm app -- ${application} retirement")
        machine.wait_for_file(runtime + "/app-retirement-ready")
        saved = machine.succeed("cat " + directory + "/session.toml")
        record = tomllib.loads(saved)
        app = machine.succeed("basename -s .toml " + directory + "/apps/*.toml").strip()
        app_state = machine.succeed(user + "systemctl --user show " + app + " -p MainPID -p InvocationID")
        wm_state = machine.succeed(user + "systemctl --user show " + record["compositor_unit"] + " -p MainPID -p InvocationID")
        launcher_state = machine.succeed("systemctl show rsdm-test-session -p MainPID -p InvocationID")
        endpoints = dict(record["exported_environment"]).get("SESSION_MANAGER", "").split(",")
        # Keep ordinary observation waiting for its peer while the disable
        # transition exercises record publication. Accepted reads may drain.
        machine.succeed("systemctl kill --kill-whom=main --signal=STOP user@1000.service")
        machine.sleep(1)
        # Existing data remains writable, but neither atomic record replacement
        # nor allocation of a new retirement marker is permitted.
        machine.succeed("chmod 500 " + directory + " " + runtime + "/rsdm/sessions")
        machine.fail(user + "touch " + directory + "/publication-probe")
        machine.succeed("printf '[session_manager]\\nenabled = false\\n' > /etc/rsdm-toggle.toml; mv /etc/rsdm-toggle.toml /etc/rsdm.toml")
        machine.wait_until_succeeds("test $(od -An -tu1 " + directory + ".disabled) = 1")
        machine.wait_until_fails(user + "busctl --user list | grep org.rsdm.Session1")
        machine.wait_until_fails("systemd-inhibit --list --no-pager | grep 'Saving registered graphical applications'")
        for endpoint in endpoints:
            if "/tmp/.ICE-unix/" in endpoint:
                socket = endpoint.split(":", 1)[1].lstrip("@")
                machine.wait_until_fails("grep -F " + shlex.quote(socket) + " /proc/net/unix")
        machine.succeed("systemctl kill --kill-whom=main --signal=CONT user@1000.service")
        assert machine.succeed("cat " + directory + "/session.toml") == saved
        machine.succeed("chmod 700 " + directory + " " + runtime + "/rsdm/sessions")
        machine.succeed("printf '[session_manager]\\nenabled = true\\n' > /etc/rsdm-toggle.toml; mv /etc/rsdm-toggle.toml /etc/rsdm.toml")
        machine.succeed(session + "rsdm session app-stop --generation " + generation + " --unit " + app)
        machine.succeed(session + "rsdm session cleanup --generation " + generation)
        assert machine.succeed("cat " + directory + "/session.toml") == saved
        assert machine.succeed(user + "systemctl --user show " + app + " -p MainPID -p InvocationID") == app_state
        assert machine.succeed(user + "systemctl --user show " + record["compositor_unit"] + " -p MainPID -p InvocationID") == wm_state
        assert machine.succeed("systemctl show rsdm-test-session -p MainPID -p InvocationID") == launcher_state
        machine.succeed("test -e " + runtime + "/display-alive")
        machine.fail("test -e " + runtime + "/app-retirement")
        machine.succeed("loginctl show-session " + record["identity"]["login_session_id"])
        machine.succeed(user + "systemctl --user stop graphical-session.target")
        machine.wait_until_fails("test -e " + runtime + "/display-alive")
        machine.wait_until_fails("systemctl is-active --quiet rsdm-test-session")
''
