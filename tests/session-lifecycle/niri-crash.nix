''
    with subtest("native, managed and detached sessions preserve compositor SIGABRT"):
        import tomllib

        for mode in ["native", "managed", "detached"]:
            result = "/tmp/niri-crash-" + mode
            command = "niri -- rsdm session finalize" if mode == "managed" else "niri-session"
            provider = "managed" if mode == "managed" else "niri"
            machine.send_chars("rsdm session start -- " + command + " > " + result + ".log 2>&1; echo $? > " + result + ".exit\n")
            machine.wait_until_succeeds(control + "rsdm session status | grep '^" + provider + ": running '", timeout=60)
            environment = machine.succeed(user + "systemctl --user show-environment")
            values = dict(line.split("=", 1) for line in environment.splitlines())
            directory = "/run/user/1000/rsdm/sessions/" + values["RSDM_SESSION_GENERATION"]
            record = tomllib.loads(machine.succeed("cat " + directory + "/session.toml"))
            assert record["compositor_invocation"]
            if mode == "detached":
                machine.succeed("printf '[session_manager]\\nenabled = false\\n' > /etc/rsdm-toggle.toml; mv /etc/rsdm-toggle.toml /etc/rsdm.toml")
                machine.wait_until_fails(user + "busctl --user list | grep org.rsdm.Session1")
            machine.succeed(user + "systemctl --user kill --kill-whom=main --signal=ABRT " + record["compositor_unit"])
            machine.wait_for_file(result + ".exit")
            assert machine.succeed("cat " + result + ".exit").strip() == "134", machine.succeed("cat " + result + ".log")
            machine.wait_until_fails(user + "systemctl --user is-active --quiet graphical-session.target")
            if mode == "detached":
                machine.succeed("printf '[session_manager]\\nenabled = true\\n' > /etc/rsdm-toggle.toml; mv /etc/rsdm-toggle.toml /etc/rsdm.toml")
''
