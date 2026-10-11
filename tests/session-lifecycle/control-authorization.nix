{ application }:
''
    def denied_control_calls(generation):
        return [
            ["Handshake"], ["Attach"], ["Status"], ["Cancel", "s", generation],
            ["Stop", "ss", generation, "logout"], ["Finalize", "sa(ss)", generation, "0"],
            ["XsmpPrepare", "sasb", generation, "0", "true"],
            ["Launch", "(sasa(ss)stssas)", generation, "1", "${application}", "0", "/", "30", "force", "auto", "0"],
        ]

    def reject_other_login(generation, login_session_id):
        prefix = [
            "systemd-run", "--unit=rsdm-test-other-login", "--wait", "--pipe", "--collect",
            "--uid=alice", "--property=PAMName=login", "--property=TTYPath=/dev/tty1",
            "--setenv=XDG_SESSION_TYPE=wayland", "--setenv=XDG_RUNTIME_DIR=" + runtime,
            "--setenv=DBUS_SESSION_BUS_ADDRESS=unix:path=" + runtime + "/bus",
            "--setenv=RSDM_SESSION_GENERATION=" + generation, "--setenv=XDG_SESSION_ID=" + login_session_id,
            "--", "busctl", "--user", "call", "org.rsdm.Session1", "/org/rsdm/Session1", "org.rsdm.Session1",
        ]
        for call in denied_control_calls(generation):
            assert_control_denied(shlex.join(prefix + call))

    def assert_control_denied(command):
        code, output = machine.execute(command + " 2>&1")
        assert code != 0 and "Call failed: Access denied" in output, (code, output)
''
