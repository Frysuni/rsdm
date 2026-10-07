{ pkgs, rsdm }:

let
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

    start_all()
    machine.wait_for_unit("multi-user.target")
    machine.succeed("loginctl enable-linger alice")
    machine.wait_for_unit("user@1000.service")
    runtime = "/run/user/1000"
    user = "runuser -u alice -- env XDG_RUNTIME_DIR=" + runtime + " DBUS_SESSION_BUS_ADDRESS=unix:path=" + runtime + "/bus "

    def start_session():
        machine.wait_until_succeeds("test \"$(systemctl show --property=LoadState --value rsdm-test-session.service)\" = not-found", timeout=30)
        machine.succeed("rm -f " + runtime + "/display-alive " + runtime + "/app-* " + runtime + "/stubborn-ready " + runtime + "/xsmp-*")
        machine.succeed("systemd-run --unit=rsdm-test-session --collect --uid=alice --property=PAMName=login --property=TTYPath=/dev/tty1 --setenv=XDG_SESSION_TYPE=wayland --setenv=XDG_RUNTIME_DIR=" + runtime + " --setenv=DBUS_SESSION_BUS_ADDRESS=unix:path=" + runtime + "/bus -- rsdm session start -- ${compositor}")
        try:
            machine.wait_until_succeeds(user + "rsdm session status | grep ': running '", timeout=30)
        except Exception:
            print(machine.succeed("journalctl --no-pager -u rsdm-test-session -n 80"))
            raise
        generation = machine.succeed(user + "systemctl --user show-environment | sed -n 's/^RSDM_SESSION_GENERATION=//p'").strip()
        return user + "RSDM_SESSION_GENERATION=" + generation + " "

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
            machine.fail(user + "systemctl --user show-environment | grep '^RSDM_SESSION_GENERATION='")
            machine.fail(user + "systemctl --user is-active --quiet graphical-session.target")
            machine.wait_until_succeeds("test \"$(systemctl show --property=LoadState --value rsdm-test-startup.service)\" = not-found")

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

    with subtest("a cancel timeout preserves the compositor and protected application"):
        session = start_session()
        machine.succeed(session + "rsdm app --shutdown-timeout 1 --on-timeout cancel -- ${stubborn}")
        machine.wait_for_file(runtime + "/stubborn-ready")
        machine.fail(session + "rsdm session stop")
        machine.succeed("test -e " + runtime + "/display-alive")
        machine.succeed(session + "rsdm session status | grep ': running '")
        machine.succeed(user + "systemctl --user kill --signal=KILL app-rsdm-*service")
        machine.succeed(session + "rsdm session stop")

    with subtest("stale generation requests cannot launch or finalize in a new session"):
        session = start_session()
        stale = user + "RSDM_SESSION_GENERATION=00000000000000000000000000000000 "
        machine.fail(stale + "rsdm app -- true")
        machine.fail(stale + "rsdm session finalize")
        machine.succeed("test -e " + runtime + "/display-alive")
        machine.succeed(session + "rsdm session stop")

    with subtest("an explicit quit command uses the app environment and a separate cgroup"):
        session = start_session()
        machine.succeed(session + "rsdm app --quit-command ${quitCommand} -- ${commandApplication}")
        machine.wait_for_file(runtime + "/command-ready")
        machine.succeed(session + "rsdm session stop")
        machine.succeed("test -e " + runtime + "/command-saved")

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
        machine.succeed(session + "rsdm app --shutdown-timeout 1 -- ${parent}")
        machine.wait_for_file(runtime + "/parent-ready")
        machine.wait_for_file(runtime + "/stubborn-ready")
        output = machine.succeed(session + "rsdm session stop 2>&1")
        assert "forced shutdown:" in output
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
  '';
}
