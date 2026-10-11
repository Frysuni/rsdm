{ xsmpClient }:
''
    with subtest("hot disable cancels XSMP saves and dialogs while clients and PAM stay alive"):
        for mode in ["idle", "save", "interact", "phase2", "saved"]:
            session = start_session()
            generation = session.split("RSDM_SESSION_GENERATION=")[1].split()[0]
            directory = runtime + "/rsdm/sessions/" + generation
            peers = [mode]
            if mode in ["phase2", "saved"]:
                peers.append("save")
            for index, peer in enumerate(peers):
                machine.succeed(session + "rsdm app --shutdown-timeout 2 --shutdown-method xsmp -- ${xsmpClient}/bin/xsmp-client hot-" + peer + " hot-" + str(index))
                machine.wait_for_file(runtime + "/xsmp-hot-" + str(index) + "-ready")
            record = tomllib.loads(machine.succeed("cat " + directory + "/session.toml"))
            units = machine.succeed("basename -a -s .toml " + directory + "/apps/*.toml").split()
            before = {unit: machine.succeed(user + "systemctl --user show " + unit + " -p MainPID -p InvocationID") for unit in units}
            wm = machine.succeed(user + "systemctl --user show " + record["compositor_unit"] + " -p MainPID -p InvocationID")
            launcher = machine.succeed("systemctl show rsdm-test-session -p MainPID -p InvocationID")
            if mode != "idle":
                machine.succeed(session + "sh -c 'rsdm session stop > /run/user/1000/xsmp-hot-stop-result 2>&1 &'")
                marker = {"save": "saving", "interact": "interacted", "phase2": "phase2-requested", "saved": "saved"}[mode]
                machine.wait_for_file(runtime + "/xsmp-hot-0-" + marker)
                if len(peers) == 2:
                    machine.wait_for_file(runtime + "/xsmp-hot-1-saving")
            machine.succeed("printf '[session_manager]\\nenabled = false\\n' > /etc/rsdm-toggle.toml; mv /etc/rsdm-toggle.toml /etc/rsdm.toml")
            machine.wait_until_succeeds("grep -q '^disabled = true$' " + directory + "/session.toml")
            machine.wait_until_fails(user + "busctl --user list | grep org.rsdm.Session1")
            machine.wait_until_fails("systemd-inhibit --list --no-pager | grep 'Saving registered graphical applications'")
            machine.wait_until_fails("test -e " + shlex.quote(dict(record["exported_environment"])["ICEAUTHORITY"]))
            for index in range(len(peers)):
                machine.wait_for_file(runtime + "/xsmp-hot-" + str(index) + "-disconnected")
                if mode != "idle":
                    machine.succeed("test -e " + runtime + "/xsmp-hot-" + str(index) + "-cancelled")
                machine.fail("test -e " + runtime + "/xsmp-hot-" + str(index) + "-cancel-missing")
                machine.fail("test -e " + runtime + "/xsmp-hot-" + str(index) + "-die")
            if mode == "phase2":
                machine.fail("test -e " + runtime + "/xsmp-hot-0-phase2")
            if mode != "idle":
                machine.succeed("grep -q unknown " + runtime + "/xsmp-hot-stop-result")
            machine.succeed("printf '[session_manager]\\nenabled = true\\n' > /etc/rsdm-toggle.toml; mv /etc/rsdm-toggle.toml /etc/rsdm.toml")
            for unit in units:
                machine.succeed(session + "rsdm session app-stop --generation " + generation + " --unit " + unit)
            machine.succeed(session + "rsdm session cleanup --generation " + generation)
            machine.sleep(8)
            for unit in units:
                assert machine.succeed(user + "systemctl --user show " + unit + " -p MainPID -p InvocationID") == before[unit], unit
            assert machine.succeed(user + "systemctl --user show " + record["compositor_unit"] + " -p MainPID -p InvocationID") == wm
            assert machine.succeed("systemctl show rsdm-test-session -p MainPID -p InvocationID") == launcher
            machine.succeed("loginctl show-session " + record["identity"]["login_session_id"])
            machine.succeed("test -e " + runtime + "/display-alive")
            machine.succeed(user + "systemctl --user stop graphical-session.target")
            machine.wait_until_fails("test -e " + runtime + "/display-alive")
            machine.wait_until_fails("systemctl is-active --quiet rsdm-test-session")
''
