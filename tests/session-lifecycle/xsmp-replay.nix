{ xsmpClient }:
''
    with subtest("a sent XSMP Die survives cancellation and a later logout"):
        session = start_session()
        generation = session.split("RSDM_SESSION_GENERATION=")[1].split()[0]
        machine.succeed(session + "rsdm app --shutdown-timeout 1 --on-timeout cancel -- ${xsmpClient}/bin/xsmp-client no-replay claimed")
        machine.wait_for_file(runtime + "/xsmp-claimed-ready")
        for attempt in range(2):
            machine.fail(session + "rsdm session stop")
            machine.succeed("test -e " + runtime + "/xsmp-claimed-die -a -e " + runtime + "/display-alive")
            record = tomllib.loads(machine.succeed("cat " + runtime + "/rsdm/sessions/" + generation + "/apps/*.toml"))
            assert record["dispatch"]["state"] == "sent", record
            assert record["dispatch"]["xsmp_die_state"] == "sent", record
            assert record["preparation"]["outcome"] == "cancelled", record
            machine.succeed(session + "rsdm session status | grep ': running '")
        machine.fail("test -e " + runtime + "/xsmp-claimed-save-replayed")
        machine.fail("test -e " + runtime + "/xsmp-claimed-die-replayed")
        machine.succeed(user + "systemctl --user kill --signal=KILL app-rsdm-*service")
        machine.succeed(session + "rsdm session stop")
        machine.fail("test -e " + runtime + "/display-alive")
''
