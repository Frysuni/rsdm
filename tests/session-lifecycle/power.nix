{ pkgs, rsdm }:

let
  python = pkgs.python3.withPackages (packages: [ packages.dbus-python packages.pygobject3 ]);
  compositor = pkgs.writeShellScript "test-power-compositor" ''
    set -eu
    touch "$XDG_RUNTIME_DIR/display-alive"
    trap 'rm -f "$XDG_RUNTIME_DIR/display-alive"; exit 0' TERM INT
    ${rsdm}/bin/rsdm session finalize
    while :; do sleep 1; done
  '';
  application = pkgs.writeShellScript "test-power-application" ''
    set -eu
    touch "$XDG_RUNTIME_DIR/app-ready"
    trap 'test -e "$XDG_RUNTIME_DIR/display-alive"; touch "$XDG_RUNTIME_DIR/app-saved"; exit 0' TERM
    while :; do sleep 1; done
  '';
  stubborn = pkgs.writeShellScript "test-power-stubborn" ''
    trap "" TERM INT
    touch "$XDG_RUNTIME_DIR/app-ready"
    while :; do sleep 1; done
  '';
  logout = pkgs.writeShellScript "test-power-logout" ''
    trap "" TERM INT
    touch "$XDG_RUNTIME_DIR/logout-ready"
    while :; do sleep 1; done
  '';
in
pkgs.testers.runNixOSTest {
  name = "rsdm-power-lifecycle";
  globalTimeout = 150;
  nodes.machine = { ... }: {
    virtualisation.memorySize = 1024;
    users.users.alice = { isNormalUser = true; uid = 1000; group = "alice"; };
    users.groups.alice = {};
    security.polkit.enable = true;
    security.pam.services.login.startSession = true;
    environment.systemPackages = [ rsdm pkgs.coreutils pkgs.util-linux pkgs.dbus ];
    environment.etc."rsdm.toml".text = "";
    systemd.user.targets.xdg-desktop-autostart = {};
  };

  testScript = ''
    import time

    start_all()
    machine.wait_for_unit("multi-user.target")
    machine.succeed("loginctl enable-linger alice")
    machine.wait_for_unit("user@1000.service")
    runtime = "/run/user/1000"
    user = "runuser -u alice -- env XDG_RUNTIME_DIR=" + runtime + " DBUS_SESSION_BUS_ADDRESS=unix:path=" + runtime + "/bus "
    machine.succeed(user + "dbus-daemon --session --fork --nopidfile --address=unix:path=" + runtime + "/power-bus")
    private = user + "DBUS_SESSION_BUS_ADDRESS=unix:path=" + runtime + "/power-bus "
    machine.succeed(user + "systemd-run --user --unit=rsdm-test-power --setenv=DBUS_SESSION_BUS_ADDRESS=unix:path=" + runtime + "/power-bus -- ${python}/bin/python3 ${./power-manager.py}")
    machine.wait_for_file(runtime + "/power-manager-ready")
    prepare = private + "busctl --address=unix:path=" + runtime + "/power-bus call org.freedesktop.login1 /org/freedesktop/login1 org.rsdm.TestPower Prepare b "
    released = private + "busctl --address=unix:path=" + runtime + "/power-bus call org.freedesktop.login1 /org/freedesktop/login1 org.rsdm.TestPower ReleasedGuards"
    release_time = private + "busctl --address=unix:path=" + runtime + "/power-bus call org.freedesktop.login1 /org/freedesktop/login1 org.rsdm.TestPower LastShutdownReleaseUsec"

    def start_session(options=""):
        machine.wait_until_succeeds("test \"$(systemctl show --property=LoadState --value rsdm-test-session.service)\" = not-found", timeout=30)
        machine.succeed("rm -f " + runtime + "/app-* " + runtime + "/display-alive " + runtime + "/power-deny-*")
        machine.succeed("systemd-run --unit=rsdm-test-session --collect --uid=alice --property=PAMName=login --property=TTYPath=/dev/tty1 --setenv=XDG_RUNTIME_DIR=" + runtime + " --setenv=DBUS_SESSION_BUS_ADDRESS=unix:path=" + runtime + "/bus --setenv=DBUS_SYSTEM_BUS_ADDRESS=unix:path=" + runtime + "/power-bus -- rsdm session start " + options + " -- ${compositor}")
        try:
            machine.wait_until_succeeds(user + "rsdm session status | grep ': running '", timeout=30)
        except Exception:
            print(machine.succeed("journalctl --no-pager -u rsdm-test-session -n 80"))
            print(machine.succeed(user + "journalctl --user --no-pager -u rsdm-test-power -n 80"))
            raise
        generation = machine.succeed(user + "systemctl --user show-environment | sed -n 's/^RSDM_SESSION_GENERATION=//p'").strip()
        return user + "RSDM_SESSION_GENERATION=" + generation + " "

    with subtest("authorization denial leaves apps and compositor untouched"):
        session = start_session()
        machine.succeed(session + "rsdm app -- ${application}")
        machine.wait_for_file(runtime + "/app-ready")
        machine.succeed("touch " + runtime + "/power-deny-check")
        machine.fail(session + "rsdm power poweroff")
        machine.fail("test -e " + runtime + "/app-saved")
        machine.succeed("test -e " + runtime + "/display-alive")
        machine.fail("test -e " + runtime + "/power-requests")
        machine.succeed(session + "rsdm session stop")

    with subtest("a denied request after app preparation preserves the compositor and stays unprivileged"):
        session = start_session()
        machine.succeed(session + "rsdm app -- ${application}")
        machine.wait_for_file(runtime + "/app-ready")
        machine.succeed("touch " + runtime + "/power-deny-request")
        machine.fail(session + "rsdm power poweroff")
        machine.succeed("test -e " + runtime + "/app-saved")
        machine.succeed("test -e " + runtime + "/display-alive")
        assert machine.succeed("cat " + runtime + "/power-requests") == "1000\n"
        machine.succeed(session + "rsdm session stop")

    with subtest("only logind can revoke external shutdown while preparation is running"):
        session = start_session()
        machine.succeed(session + "rsdm app --shutdown-timeout 30 --on-timeout cancel -- ${stubborn}")
        machine.wait_for_file(runtime + "/app-ready")
        machine.succeed(prepare + "true")
        machine.wait_until_succeeds(session + "rsdm session status | grep ': preparing '")
        machine.fail(session + "rsdm session cancel")
        machine.succeed(prepare + "false")
        machine.wait_until_succeeds(session + "rsdm session status | grep ': running '")
        machine.succeed("test -e " + runtime + "/display-alive")
        machine.succeed(user + "systemctl --user is-active --quiet 'app-rsdm-*service'")
        machine.succeed(user + "systemctl --user kill --signal=KILL app-rsdm-*service")
        machine.succeed(session + "rsdm session stop")

    with subtest("external shutdown overrides app cancellation within the five-second delay budget"):
        session = start_session()
        machine.succeed(session + "rsdm app --shutdown-timeout 30 --on-timeout cancel -- ${stubborn}")
        machine.wait_for_file(runtime + "/app-ready")
        started = time.monotonic()
        machine.succeed(prepare + "true")
        machine.wait_until_fails("systemctl is-active --quiet rsdm-test-session", timeout=10)
        assert time.monotonic() - started < 7
        machine.fail("test -e " + runtime + "/display-alive")
        machine.fail(user + "systemctl --user is-active --quiet 'app-rsdm-*service'")
        machine.wait_until_succeeds(release_time + " | awk '$2 > 0 && $2 < 5000000 { found=1 } END { exit !found }'")
        machine.succeed(prepare + "false")

    with subtest("accepted user power releases the inhibitor after actual app and session teardown"):
        session = start_session()
        machine.succeed(session + "rsdm app -- ${application}")
        machine.wait_for_file(runtime + "/app-ready")
        machine.succeed(session + "rsdm power poweroff")
        machine.succeed("test -e " + runtime + "/app-saved")
        machine.fail("test -e " + runtime + "/display-alive")
        assert machine.succeed("cat " + runtime + "/power-requests") == "1000\n1000\n"
        machine.wait_until_succeeds(released + " | grep '^u 5$'")

    with subtest("a stuck logout helper leaves time to stop the compositor"):
        machine.succeed(prepare + "false")
        session = start_session("--mode managed --logout-command ${logout}")
        machine.succeed(prepare + "true")
        machine.wait_for_file(runtime + "/logout-ready")
        machine.wait_until_fails("systemctl is-active --quiet rsdm-test-session", timeout=10)
        machine.fail("test -e " + runtime + "/display-alive")
        machine.wait_until_succeeds(release_time + " | awk '$2 > 0 && $2 < 5000000 { found=1 } END { exit !found }'")
        machine.succeed(prepare + "false")
  '';
}
