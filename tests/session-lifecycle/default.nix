{ pkgs, rsdm }:

let
  controller = import ./controller.nix { inherit pkgs; };
  compositor = pkgs.writeShellScript "test-compositor" ''
    set -eu
    touch "$XDG_RUNTIME_DIR/display-alive"
    trap 'rm -f "$XDG_RUNTIME_DIR/display-alive"; exit 0' TERM INT
    ${rsdm}/bin/rsdm session finalize
    while :; do sleep 1; done
  '';
  application = pkgs.writeShellScript "test-application" ''
    set -eu
    report="$XDG_RUNTIME_DIR/app-$1"
    touch "$report-ready"
    trap 'test -e "$XDG_RUNTIME_DIR/display-alive"; echo saving >> "$report"; sleep 0.2; echo saved >> "$report"; exit 0' TERM
    while :; do sleep 1; done
  '';
  stubborn = pkgs.writeShellScript "test-stubborn-application" ''
    trap "" TERM INT
    touch "$XDG_RUNTIME_DIR/stubborn-ready"
    while :; do sleep 1; done
  '';
  concurrentApplication = pkgs.writeShellScript "test-concurrent-application" ''
    set -eu
    report="$XDG_RUNTIME_DIR/app-parallel-$1"
    finish() {
      touch "$report-requested"
      for index in 1 2 3 4 5 6 7 8; do
        while ! test -e "$XDG_RUNTIME_DIR/app-parallel-$index-requested"; do sleep 0.05; done
      done
      test -e "$XDG_RUNTIME_DIR/display-alive"
      touch "$report-saved"
      exit 0
    }
    trap finish TERM
    touch "$report-ready"
    while :; do sleep 1; done
  '';
  parent = pkgs.writeShellScript "test-application-with-child" ''
    set -eu
    trap 'touch "$XDG_RUNTIME_DIR/parent-saved"; exit 0' TERM
    ${stubborn} &
    touch "$XDG_RUNTIME_DIR/parent-ready"
    while :; do sleep 1; done
  '';
  arguments = pkgs.writeShellScript "test-arguments" ''
    printf '<%s>\n' "$@" > "$XDG_RUNTIME_DIR/arguments"
  '';
  commandApplication = pkgs.writeShellScript "test-command-application" ''
    set -eu
    trap "" TERM INT
    fifo="$XDG_RUNTIME_DIR/quit-request"
    mkfifo "$fifo"
    touch "$XDG_RUNTIME_DIR/command-ready"
    read -r request < "$fifo"
    test "$request" = quit
    test -e "$XDG_RUNTIME_DIR/display-alive"
    touch "$XDG_RUNTIME_DIR/command-saved"
    rm "$fifo"
  '';
  quitCommand = pkgs.writeShellScript "test-quit-command" ''
    printf 'quit\n' > "$XDG_RUNTIME_DIR/quit-request"
  '';
  xsmpClient = pkgs.runCommand "test-xsmp-client" {
    nativeBuildInputs = [ pkgs.pkg-config pkgs.stdenv.cc ];
    buildInputs = [ pkgs.libsm pkgs.libice pkgs.xorgproto ];
  } ''
    mkdir -p "$out/bin"
    cc -std=c11 -Wall -Wextra -Werror ${./xsmp-client.c} -o "$out/bin/xsmp-client" $(pkg-config --cflags --libs sm ice)
  '';
in
pkgs.testers.runNixOSTest {
  name = "rsdm-session-lifecycle";
  globalTimeout = 300;
  nodes.machine = { ... }: {
    virtualisation.memorySize = 1024;
    users.users.alice = { isNormalUser = true; uid = 1000; group = "alice"; };
    users.groups.alice = {};
    services.logind.settings.Login.InhibitDelayMaxSec = 5;
    security.polkit.enable = true;
    security.pam.services.login.startSession = true;
    environment.systemPackages = [ rsdm pkgs.coreutils pkgs.util-linux ];
    environment.etc."rsdm.toml".text = "";
    systemd.user.targets.xdg-desktop-autostart = {};
  };

  testScript = ''
    import shlex
    import time
    import tomllib

    start_all()
    machine.wait_for_unit("multi-user.target")
    machine.succeed("loginctl enable-linger alice")
    machine.wait_for_unit("user@1000.service")
    runtime = "/run/user/1000"
    user = "runuser -u alice -- env XDG_RUNTIME_DIR=" + runtime + " DBUS_SESSION_BUS_ADDRESS=unix:path=" + runtime + "/bus "
    control = "${controller} system rsdm-test-session.service "

    def start_session():
        machine.wait_until_succeeds("test \"$(systemctl show --property=LoadState --value rsdm-test-session.service)\" = not-found", timeout=30)
        machine.succeed("rm -f " + runtime + "/display-alive " + runtime + "/app-* " + runtime + "/stubborn-ready " + runtime + "/xsmp-*")
        machine.succeed("systemd-run --unit=rsdm-test-session --collect --uid=alice --property=PAMName=login --property=TTYPath=/dev/tty1 --setenv=XDG_SESSION_TYPE=wayland --setenv=XDG_RUNTIME_DIR=" + runtime + " --setenv=DBUS_SESSION_BUS_ADDRESS=unix:path=" + runtime + "/bus -- rsdm session start -- ${compositor}")
        try:
            machine.wait_until_succeeds(control + "rsdm session status | grep ': running '", timeout=30)
        except Exception:
            print(machine.succeed("journalctl --no-pager -u rsdm-test-session -n 80"))
            raise
        generation = machine.succeed(user + "systemctl --user show-environment | sed -n 's/^RSDM_SESSION_GENERATION=//p'").strip()
        assert generation in machine.succeed(control + "busctl --user call org.rsdm.Session1 /org/rsdm/Session1 org.rsdm.Session1 Handshake")
        return control + "RSDM_SESSION_GENERATION=" + generation + " "

    with subtest("disabled entry points launch ordinary commands without session infrastructure"):
        machine.succeed("printf '[session_manager]\\nenabled = false\\n' > /run/rsdm-disabled.toml")
        disabled = user + "RSDM_SESSION_GENERATION=invalid rsdm --config /run/rsdm-disabled.toml "
        for command in [
            "session finalize 'INVALID NAME'", "session stop", "session cancel",
            "session app-stop --generation ../invalid --unit ../invalid",
            "session cleanup --generation ../invalid",
        ]:
            machine.succeed(disabled + command)
        machine.succeed(disabled + "session status | grep '^disabled:'")
        for command in ["app --shutdown-timeout 0 --shutdown-method xsmp", "session start --mode external --native-unit missing.service"]:
            code, output = machine.execute(disabled + command + " -- /bin/sh -c 'printf done; exit 7'")
            assert code == 7 and output == "done", (code, output)
        machine.fail("test -e " + runtime + "/rsdm")
        machine.fail(user + "systemctl --user list-units --all --no-legend 'app-rsdm-*' 'rsdm-session-*' | grep .")
        machine.fail(user + "busctl --user list | grep org.rsdm.Session1")
        machine.fail("systemd-inhibit --list --no-pager | grep RSDM")

    with subtest("startup failures leave no published environment or unrecoverable empty generation"):
        for generation, options in [
            ("ffffffffffffffffffffffffffffff01", []),
            ("ffffffffffffffffffffffffffffff02", ["--mode", "external", "--native-unit", "missing.service"]),
        ]:
            command = [
                "systemd-run", "--unit=rsdm-test-startup", "--wait", "--collect", "--uid=alice",
                "--property=PAMName=login", "--property=TTYPath=/dev/tty1",
                "--setenv=XDG_SESSION_TYPE=wayland", "--setenv=XDG_RUNTIME_DIR=" + runtime,
                "--setenv=DBUS_SESSION_BUS_ADDRESS=unix:path=" + runtime + "/bus",
                "--setenv=RSDM_SESSION_GENERATION=" + generation,
                "rsdm", "session", "start",
            ] + options + ["--", "/nonexistent-rsdm-test-compositor"]
            machine.fail(shlex.join(command))
            machine.succeed(user + "rsdm session cleanup --generation " + generation)
            try:
                machine.fail(user + "systemctl --user show-environment | grep '^RSDM_SESSION_GENERATION='")
            except Exception:
                print(machine.succeed(user + "systemctl --user show-environment"))
                print(machine.succeed("cat " + runtime + "/rsdm/sessions/" + generation + "/session.toml"))
                raise
            machine.fail(user + "systemctl --user is-active --quiet graphical-session.target")
            machine.wait_until_succeeds("test \"$(systemctl show --property=LoadState --value rsdm-test-startup.service)\" = not-found")

    with subtest("disabled generation hooks stay passive after configuration is reenabled"):
        session = start_session()
        machine.succeed(session + "rsdm app -- ${application} disabled-record")
        machine.wait_for_file(runtime + "/app-disabled-record-ready")
        generation = session.split("RSDM_SESSION_GENERATION=")[1].split()[0]
        directory = runtime + "/rsdm/sessions/" + generation
        record = tomllib.loads(machine.succeed("cat " + directory + "/session.toml"))
        app = machine.succeed("basename -s .toml " + directory + "/apps/*.toml").strip()
        app_state = machine.succeed(user + "systemctl --user show " + app + " -p MainPID -p InvocationID")
        wm_state = machine.succeed(user + "systemctl --user show " + record["compositor_unit"] + " -p MainPID -p InvocationID")
        machine.succeed("systemctl kill --kill-whom=main --signal=KILL rsdm-test-session")
        machine.wait_until_fails("systemctl is-active --quiet rsdm-test-session")
        machine.succeed(user + "sed -i 's/^disabled = false$/disabled = true/' " + directory + "/session.toml")
        saved = machine.succeed("cat " + directory + "/session.toml")
        assert tomllib.loads(saved)["disabled"]
        registered = machine.succeed("cat " + directory + "/apps/" + app + ".toml")
        for enabled in ["false", "true"]:
            machine.succeed("printf '[session_manager]\\nenabled = " + enabled + "\\n' > /run/rsdm-toggle.toml")
            hooks = user + "rsdm --config /run/rsdm-toggle.toml session "
            machine.succeed(hooks + "app-stop --generation " + generation + " --unit " + app)
            machine.succeed(hooks + "cleanup --generation " + generation)
            assert machine.succeed("cat " + directory + "/session.toml") == saved
            assert machine.succeed("cat " + directory + "/apps/" + app + ".toml") == registered
            assert machine.succeed(user + "systemctl --user show " + app + " -p MainPID -p InvocationID") == app_state
            assert machine.succeed(user + "systemctl --user show " + record["compositor_unit"] + " -p MainPID -p InvocationID") == wm_state
            machine.succeed("test -e " + runtime + "/display-alive")
            machine.fail("test -e " + runtime + "/app-disabled-record")
            assert ("RSDM_SESSION_GENERATION=" + generation) in machine.succeed(user + "systemctl --user show-environment")
        machine.succeed(user + "systemctl --user stop graphical-session.target")
        machine.wait_until_fails("test -e " + runtime + "/display-alive")

    with subtest("applications save before the compositor is stopped"):
        session = start_session()
        machine.succeed(session + shlex.join(["rsdm", "app", "--", "${arguments}", "$HOME", "", "--flag", "two words"]))
        machine.wait_for_file(runtime + "/arguments")
        assert machine.succeed("cat " + runtime + "/arguments") == "<$HOME>\n<>\n<--flag>\n<two words>\n"
        machine.succeed(session + "rsdm app -- ${application} first")
        machine.wait_for_file(runtime + "/app-first-ready")
        machine.succeed(session + "rsdm session stop")
        machine.succeed("grep -q saved " + runtime + "/app-first")
        machine.fail("test -e " + runtime + "/display-alive")
        machine.wait_until_fails("systemctl is-active --quiet rsdm-test-session")

    with subtest("termination signals prepare applications before stopping the compositor"):
        for signal in ["TERM", "INT"]:
            session = start_session()
            name = "signal-" + signal.lower()
            machine.succeed(session + "rsdm app -- ${application} " + name)
            machine.wait_for_file(runtime + "/app-" + name + "-ready")
            machine.succeed("systemctl kill --kill-whom=main --signal=" + signal + " rsdm-test-session")
            machine.wait_until_fails("systemctl is-active --quiet rsdm-test-session")
            machine.succeed("grep -q saved " + runtime + "/app-" + name)
            machine.fail("test -e " + runtime + "/display-alive")

    with subtest("all eight applications receive requests before any earlier application exits"):
        session = start_session()
        for index in range(1, 9):
            machine.succeed(session + "rsdm app -- ${concurrentApplication} " + str(index))
            machine.wait_for_file(runtime + "/app-parallel-" + str(index) + "-ready")
        machine.succeed(session + "rsdm session stop", timeout=20)
        for index in range(1, 9):
            machine.succeed("test -e " + runtime + "/app-parallel-" + str(index) + "-saved")
        machine.fail("test -e " + runtime + "/display-alive")

    with subtest("signal-triggered preparation can cancel and a later signal can stop the session"):
        session = start_session()
        machine.succeed(session + "rsdm app --shutdown-timeout 1 --on-timeout cancel -- ${stubborn}")
        machine.wait_for_file(runtime + "/stubborn-ready")
        machine.succeed("systemctl kill --kill-whom=main --signal=TERM rsdm-test-session")
        machine.wait_until_succeeds(session + "rsdm session status | grep ': preparing '")
        machine.wait_until_succeeds(session + "rsdm session status | grep ': running '")
        machine.succeed("test -e " + runtime + "/display-alive")
        machine.succeed(user + "systemctl --user is-active --quiet app-rsdm-*service")
        machine.succeed(user + "systemctl --user kill --signal=KILL app-rsdm-*service")
        machine.succeed("systemctl kill --kill-whom=main --signal=INT rsdm-test-session")
        machine.wait_until_fails("systemctl is-active --quiet rsdm-test-session")
        machine.fail("test -e " + runtime + "/display-alive")

    with subtest("a cancel timeout preserves the compositor and protected application"):
        session = start_session()
        machine.succeed(session + "rsdm app --shutdown-timeout 1 --on-timeout cancel -- ${stubborn}")
        machine.wait_for_file(runtime + "/stubborn-ready")
        machine.fail(session + "rsdm session stop")
        machine.succeed("test -e " + runtime + "/display-alive")
        machine.succeed(session + "rsdm session status | grep ': running '")
        generation = session.split("RSDM_SESSION_GENERATION=")[1].split()[0]
        record = tomllib.loads(machine.succeed("cat " + runtime + "/rsdm/sessions/" + generation + "/apps/*.toml"))
        assert record["preparation"]["outcome"] == "cancelled"
        assert not record["preparation"]["escalated"]
        machine.succeed(user + "systemctl --user kill --signal=KILL app-rsdm-*service")
        machine.succeed(session + "rsdm session stop")

    with subtest("stale generation requests cannot launch or finalize in a new session"):
        session = start_session()
        stale = user + "RSDM_SESSION_GENERATION=00000000000000000000000000000000 "
        machine.fail(stale + "rsdm app -- true")
        machine.fail(stale + "rsdm session finalize")
        machine.succeed("test -e " + runtime + "/display-alive")
        machine.succeed(session + "rsdm session stop")

    with subtest("an independent app hook consumes a later reduction of the saved deadline"):
        session = start_session()
        machine.succeed(session + "rsdm app --shutdown-method term --shutdown-timeout 30 -- ${stubborn}")
        machine.wait_for_file(runtime + "/stubborn-ready")
        generation = session.split("RSDM_SESSION_GENERATION=")[1].split()[0]
        directory = runtime + "/rsdm/sessions/" + generation
        app = tomllib.loads(machine.succeed("cat " + directory + "/apps/*.toml"))
        unit = shlex.quote(app["unit"])
        machine.succeed(user + "systemctl --user stop --no-block " + unit)
        machine.wait_until_succeeds(user + "systemctl --user show " + unit + " -p ControlPID --value | grep -E '^[1-9][0-9]*$'")
        machine.wait_until_succeeds("grep '^quit_started = true$' " + directory + "/apps/*.toml")
        record = tomllib.loads(machine.succeed("cat " + directory + "/session.toml"))
        assert "shutdown_deadline_usec" not in record
        deadline = int(float(machine.succeed("cut -d ' ' -f1 /proc/uptime")) * 1_000_000) + 3_000_000
        machine.succeed("sed -i '1i shutdown_deadline_usec = " + str(deadline) + "' " + directory + "/session.toml")
        machine.wait_until_succeeds(user + "systemctl --user show " + unit + " -p ActiveState --value | grep -E '^(inactive|failed)$'", timeout=10)
        result_path = shlex.quote(directory + "/results/" + app["unit"] + ".toml")
        machine.wait_until_succeeds("test -f " + result_path, timeout=10)
        result = tomllib.loads(machine.succeed("cat " + result_path))
        assert result["preparation"]["outcome"] == "forced"
        assert result["preparation"]["escalated"]
        machine.succeed("test -e " + runtime + "/display-alive")
        machine.succeed(session + "rsdm session status | grep ': running '")
        machine.succeed(session + "rsdm session stop")

    with subtest("an explicit quit command uses the app environment and a separate cgroup"):
        session = start_session()
        machine.succeed(session + "rsdm app --quit-command ${quitCommand} -- ${commandApplication}")
        machine.wait_for_file(runtime + "/command-ready")
        generation = session.split("RSDM_SESSION_GENERATION=")[1].split()[0]
        app_record = runtime + "/rsdm/sessions/" + generation + "/apps/*.toml"
        deadline = int(float(machine.succeed("cut -d ' ' -f1 /proc/uptime")) * 1_000_000) + 30_000_000
        machine.succeed("sed -i '1i deadline_usec = " + str(deadline) + "' " + app_record)
        prepared = tomllib.loads(machine.succeed("cat " + app_record))
        assert prepared["deadline_usec"] == deadline and not prepared["quit_started"]
        assert "dispatch" not in prepared
        machine.succeed(session + "rsdm session stop")
        machine.succeed("test -e " + runtime + "/command-saved")
        result = tomllib.loads(machine.succeed("cat " + runtime + "/rsdm/sessions/" + generation + "/results/*.toml"))
        assert result["invocation_id"] == prepared["invocation_id"]
        assert result["preparation"]["outcome"] == "clean"
        assert not result["preparation"]["escalated"]
        assert result["preparation"]["transaction_id"] == result["dispatch"]["transaction_id"]

    with subtest("generation recovery saves apps after the coordinator was killed"):
        session = start_session()
        machine.succeed(session + "rsdm app -- ${application} recovery")
        machine.wait_for_file(runtime + "/app-recovery-ready")
        generation = session.split("RSDM_SESSION_GENERATION=")[1].split()[0]
        machine.succeed("systemctl kill --kill-whom=main --signal=KILL rsdm-test-session")
        machine.wait_until_fails("systemctl is-active --quiet rsdm-test-session")
        machine.succeed(user + "rsdm session cleanup --generation " + generation)
        machine.succeed("grep -q saved " + runtime + "/app-recovery")
        machine.fail("test -e " + runtime + "/display-alive")

    with subtest("an external target stop uses ExecStop while the compositor is alive"):
        session = start_session()
        machine.succeed(session + "rsdm app -- ${application} external")
        machine.wait_for_file(runtime + "/app-external-ready")
        machine.succeed(user + "systemctl --user stop graphical-session.target")
        machine.succeed("grep -q saved " + runtime + "/app-external")
        machine.wait_until_fails("test -e " + runtime + "/display-alive")

    with subtest("XSMP waits for first phases, phase 2, saving and then actual exit"):
        session = start_session()
        for mode, name in [("phase2", "second"), ("slow-first", "first")]:
            machine.succeed(session + "rsdm app --shutdown-method xsmp -- ${xsmpClient}/bin/xsmp-client " + mode + " " + name)
            machine.wait_for_file(runtime + "/xsmp-" + name + "-ready")
        machine.succeed(session + "rsdm session status | grep xsmp")
        machine.succeed(session + "rsdm session stop")
        machine.succeed("test -e " + runtime + "/xsmp-second-phase2")
        machine.succeed("test -e " + runtime + "/xsmp-first-die -a -e " + runtime + "/xsmp-second-die")
        machine.fail("test -e " + runtime + "/display-alive")

    with subtest("XSMP interaction can cancel before infrastructure is stopped"):
        session = start_session()
        machine.succeed(session + "rsdm app --on-timeout cancel -- ${xsmpClient}/bin/xsmp-client cancel protected")
        machine.wait_for_file(runtime + "/xsmp-protected-ready")
        machine.fail(session + "rsdm session stop")
        machine.wait_for_file(runtime + "/xsmp-protected-cancelled")
        machine.succeed("test -e " + runtime + "/display-alive")
        machine.succeed(session + "rsdm session status | grep ': running '")
        machine.fail("test -e " + runtime + "/xsmp-protected-die")
        machine.wait_for_file(runtime + "/xsmp-protected-resumed")
        machine.succeed(session + "rsdm session stop")
        machine.succeed("test -e " + runtime + "/xsmp-protected-die")

    with subtest("XSMP save failure cancels controlled logout and permits a later retry"):
        session = start_session()
        machine.succeed(session + "rsdm app -- ${xsmpClient}/bin/xsmp-client failed failure")
        machine.wait_for_file(runtime + "/xsmp-failure-ready")
        machine.fail(session + "rsdm session stop")
        machine.succeed("test -e " + runtime + "/display-alive")
        machine.fail("test -e " + runtime + "/xsmp-failure-die")
        machine.wait_for_file(runtime + "/xsmp-failure-cancelled")
        machine.wait_for_file(runtime + "/xsmp-failure-resumed")
        machine.succeed(session + "rsdm session stop")
        machine.succeed("test -e " + runtime + "/xsmp-failure-die")

    with subtest("SaveYourselfDone and Die do not count as process exit"):
        session = start_session()
        machine.succeed(session + "rsdm app --shutdown-timeout 1 -- ${xsmpClient}/bin/xsmp-client stay survivor")
        machine.wait_for_file(runtime + "/xsmp-survivor-ready")
        started = time.monotonic()
        machine.succeed(session + "rsdm session stop")
        assert time.monotonic() - started >= 1
        machine.succeed("test -e " + runtime + "/xsmp-survivor-die")
        machine.fail("test -e " + runtime + "/display-alive")

    with subtest("explicit XSMP without a connection fails before application signals"):
        session = start_session()
        machine.succeed(session + "rsdm app --shutdown-method xsmp -- ${application} no-peer")
        machine.wait_for_file(runtime + "/app-no-peer-ready")
        machine.fail(session + "rsdm session stop")
        machine.succeed("test -e " + runtime + "/display-alive")
        machine.fail("test -e " + runtime + "/app-no-peer")
        machine.succeed(user + "systemctl --user kill --signal=KILL app-rsdm-*service")
        machine.succeed(session + "rsdm session stop")

    with subtest("XSMP rejects wrong cookies and unregistered process identities"):
        session = start_session()
        environment = machine.succeed(user + "systemctl --user show-environment")
        values = dict(line.split("=", 1) for line in environment.splitlines())
        authority = values["ICEAUTHORITY"]
        assert machine.succeed("stat -c %a " + shlex.quote(authority)).strip() == "600"
        assert machine.succeed("stat -c %a " + shlex.quote(authority.rsplit("/", 1)[0])).strip() == "700"
        machine.succeed(session + "ICEAUTHORITY=/dev/null rsdm app -- ${xsmpClient}/bin/xsmp-client rejected bad-cookie")
        machine.wait_for_file(runtime + "/xsmp-bad-cookie-rejected")
        outsider = user + "SESSION_MANAGER=" + shlex.quote(values["SESSION_MANAGER"]) + " ICEAUTHORITY=" + shlex.quote(authority) + " "
        machine.succeed(outsider + "${xsmpClient}/bin/xsmp-client rejected outsider")
        machine.succeed("test -e " + runtime + "/xsmp-outsider-rejected")
        machine.succeed("test -e " + runtime + "/display-alive")
        machine.succeed(session + "rsdm session stop")

    with subtest("remaining child processes are forced before the compositor exits"):
        session = start_session()
        generation = session.split("RSDM_SESSION_GENERATION=")[1].split()[0]
        machine.succeed(session + "rsdm app --shutdown-timeout 1 -- ${parent}")
        machine.wait_for_file(runtime + "/parent-ready")
        machine.wait_for_file(runtime + "/stubborn-ready")
        output = machine.succeed(session + "rsdm session stop 2>&1")
        assert "forced shutdown:" in output
        result = tomllib.loads(machine.succeed("cat " + runtime + "/rsdm/sessions/" + generation + "/results/*.toml"))
        assert result["preparation"]["outcome"] == "forced"
        assert result["preparation"]["escalated"]
        machine.succeed("test -e " + runtime + "/parent-saved")
        machine.fail("test -e " + runtime + "/display-alive")

    with subtest("per-application force deadlines run in parallel"):
        session = start_session()
        for _ in range(2):
            machine.succeed(session + "rsdm app --shutdown-timeout 1 -- ${stubborn}")
        started = time.monotonic()
        output = machine.succeed(session + "rsdm session stop 2>&1")
        assert time.monotonic() - started < 9
        assert output.count("forced shutdown:") == 2

    with subtest("cancel timeout interrupts another apps force grace before SIGKILL"):
        session = start_session()
        machine.succeed(session + "rsdm app --shutdown-timeout 1 -- ${stubborn}")
        machine.succeed(session + "rsdm app --shutdown-timeout 2 --on-timeout cancel -- ${stubborn}")
        machine.fail(session + "rsdm session stop")
        units = machine.succeed(user + "systemctl --user list-units --state=active --plain --no-legend 'app-rsdm-*service'")
        names = [line.split()[0] for line in units.splitlines() if line.strip()]
        assert len(names) == 2, units
        for name in names:
            pid = machine.succeed(user + "systemctl --user show --property=MainPID --value " + shlex.quote(name)).strip()
            assert int(pid) > 0, name
            machine.succeed("test -d /proc/" + pid)
        machine.succeed("test -e " + runtime + "/display-alive")
        machine.succeed(user + "systemctl --user kill --signal=KILL app-rsdm-*service")
        machine.succeed(session + "rsdm session stop")

    with subtest("preparing closes launch and finalize admission while cancel stays responsive"):
        session = start_session()
        machine.succeed(session + "rsdm app --shutdown-timeout 10 --on-timeout cancel -- ${stubborn}")
        machine.succeed(session + "sh -c 'rsdm session stop > /run/user/1000/stop-result 2>&1 &'")
        machine.wait_until_succeeds(session + "rsdm session status | grep ': preparing '")
        machine.fail(session + "rsdm app -- true")
        machine.fail(session + "rsdm session finalize")
        machine.succeed(session + "rsdm session cancel")
        machine.wait_until_succeeds(session + "rsdm session status | grep ': running '")
        machine.succeed("grep -q cancelled " + runtime + "/stop-result")
        machine.succeed("test -e " + runtime + "/display-alive")
        machine.succeed(user + "systemctl --user kill --signal=KILL app-rsdm-*service")
        machine.succeed(session + "rsdm session stop")
    machine.fail("journalctl --no-pager -b | grep 'XSMP error:'")
  '' + (import ./hot-disable.nix { inherit application stubborn; })
    + (import ./retirement.nix { inherit application; })
    + (import ./xsmp-hot-disable.nix { inherit xsmpClient; })
    + (import ./bootstrap-disable.nix { inherit pkgs rsdm; });
}
