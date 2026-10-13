{ application, commandApplication, quitCommand }:
''
    def enable_attached_session():
        machine.wait_until_succeeds(user + "systemctl --user show rsdm-session-manager.service -p LoadState --value | grep -qx not-found")
        machine.succeed("rm -f " + runtime + "/attach-app-*")
        machine.succeed("printf '[session_manager]\\nenabled = true\\n' > /etc/rsdm-toggle.toml; mv /etc/rsdm-toggle.toml /etc/rsdm.toml")
        assert machine.succeed(control + "rsdm session status") == "inactive: no session manager coordinator is running\n"
        machine.fail(user + "busctl --user list | grep org.rsdm.Session1")
        machine.succeed(user + "systemctl --user show rsdm-session-manager.service -p LoadState --value | grep -qx not-found")

    def attached_generation():
        status = machine.succeed(control + "rsdm session status")
        assert status.startswith("partial: running "), status
        assert "manager: partial\n" in status, status
        assert "compositor-lifecycle: unavailable" in status, status
        assert "PAM lifecycle are not owned" in status, status
        return status.split("generation ")[1].split(")")[0]

    def retire_attached_session():
        machine.succeed("printf '[session_manager]\\nenabled = false\\n' > /etc/rsdm-toggle.toml; mv /etc/rsdm-toggle.toml /etc/rsdm.toml")
        machine.wait_until_fails(user + "systemctl --user is-active --quiet rsdm-session-manager.service")
        machine.wait_until_fails(user + "busctl --user list | grep org.rsdm.Session1")
        assert machine.succeed("systemctl show rsdm-test-external-session -p MainPID -p InvocationID") == wm
        assert machine.succeed(user + "systemctl --user show-environment") == before

    with subtest("disabled attach, serve and finalize do not create a coordinator"):
        for args in ["session attach", "session serve", "session finalize"]:
            machine.succeed(control + "RSDM_SESSION_GENERATION=invalid rsdm " + args)
        machine.fail(user + "busctl --user list | grep org.rsdm.Session1")
        machine.fail(user + "systemctl --user is-active --quiet rsdm-session-manager.service")

    for trigger in ["app", "finalize"]:
        with subtest("first " + trigger + " automatically attaches without a generation hint"):
            enable_attached_session()
            if trigger == "finalize":
                machine.succeed(control + "RSDM_SESSION_GENERATION=stale rsdm session finalize")
            machine.succeed(control + "rsdm app -- ${application}")
            machine.wait_for_file(runtime + "/attach-app-ready")
            current = attached_generation()
            assert current != generation
            assert machine.succeed("cat " + runtime + "/attach-app-generation").strip() == current
            assert "prepared" in machine.succeed(control + "rsdm session stop")
            machine.wait_for_file(runtime + "/attach-app-saved")
            status = machine.succeed(control + "rsdm session status")
            assert "preparation: app-rsdm-" in status, status
            assert "clean, method term, dispatch sent, escalated false" in status, status
            retire_attached_session()

    with subtest("concurrent attach requests join one current coordinator"):
        enable_attached_session()
        command = control + "rsdm session attach"
        machine.succeed("(" + command + " > " + runtime + "/attach-first 2>&1) & first=$!; (" + command + " > " + runtime + "/attach-second 2>&1) & second=$!; wait $first; wait $second")
        current = attached_generation()
        assert current in machine.succeed("cat " + runtime + "/attach-first")
        assert current in machine.succeed("cat " + runtime + "/attach-second")
        record = tomllib.loads(machine.succeed("cat " + runtime + "/rsdm/sessions/" + current + "/session.toml"))
        assert record["identity"]["login_session_id"] == login_session_id
        owner = machine.succeed(user + "systemctl --user show rsdm-session-manager -p MainPID -p InvocationID")
        other = "systemd-run --unit=rsdm-test-attach-other --wait --pipe --collect --uid=alice --property=PAMName=login --property=TTYPath=/dev/tty1 --setenv=XDG_SESSION_TYPE=wayland --setenv=XDG_RUNTIME_DIR=" + runtime + " --setenv=DBUS_SESSION_BUS_ADDRESS=unix:path=" + runtime + "/bus --setenv=RSDM_SESSION_GENERATION=" + current + " --setenv=XDG_SESSION_ID=" + record["identity"]["login_session_id"] + " -- rsdm session attach"
        machine.fail(other)
        assert machine.succeed(user + "systemctl --user show rsdm-session-manager -p MainPID -p InvocationID") == owner
        retire_attached_session()

    with subtest("hot disable leaves newly registered applications alive"):
        enable_attached_session()
        machine.succeed(control + "rsdm app -- ${application}")
        machine.wait_for_file(runtime + "/attach-app-ready")
        current = attached_generation()
        directory = runtime + "/rsdm/sessions/" + current
        application_record = machine.succeed("ls " + directory + "/apps/*.toml").strip()
        app = tomllib.loads(machine.succeed("cat " + application_record))["unit"]
        app_state = machine.succeed(user + "systemctl --user show " + app + " -p MainPID -p InvocationID")
        retire_attached_session()
        assert machine.succeed(user + "systemctl --user show " + app + " -p MainPID -p InvocationID") == app_state
        machine.fail("test -e " + runtime + "/attach-app-saved")
        machine.succeed(user + "systemctl --user stop " + app)

    with subtest("cleanup runs the registered quit command after the attached coordinator is collected"):
        enable_attached_session()
        machine.succeed("rm -f " + runtime + "/attach-command-* " + runtime + "/attach-quit-request")
        machine.succeed(control + "rsdm app --quit-command ${quitCommand} -- ${commandApplication}")
        machine.wait_for_file(runtime + "/attach-command-ready")
        current = attached_generation()
        machine.succeed(user + "systemctl --user kill --signal=KILL rsdm-session-manager.service")
        machine.wait_until_fails(user + "busctl --user list | grep org.rsdm.Session1")
        machine.wait_until_succeeds(user + "systemctl --user show rsdm-session-manager.service -p LoadState --value | grep -qx not-found")
        assert machine.succeed(control + "rsdm session status") == "inactive: no session manager coordinator is running\n"
        machine.fail("test -e " + runtime + "/attach-command-saved")
        machine.succeed(control + "rsdm session cleanup --generation " + current)
        machine.wait_for_file(runtime + "/attach-command-saved")
        record = tomllib.loads(machine.succeed("cat " + runtime + "/rsdm/sessions/" + current + "/session.toml"))
        assert record["phase"] == "closed" and record["provider"] == "partial"
        assert machine.succeed("systemctl show rsdm-test-external-session -p MainPID -p InvocationID") == wm
        assert machine.succeed(user + "systemctl --user show-environment") == before
        machine.succeed("loginctl show-session " + login_session_id)
        machine.fail("test -e " + runtime + "/attach-wm-stopped")
        machine.fail(user + "systemctl --user is-active --quiet graphical-session.target")
        machine.fail(user + "busctl --user list | grep org.rsdm.Session1")
''
