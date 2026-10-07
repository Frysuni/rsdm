{ pkgs, rsdm }:

let
  python = pkgs.python3.withPackages (packages: [ packages.dbus-python packages.pygobject3 ]);
  native = pkgs.writeShellScript "test-native-session" ''
    exec ${python}/bin/python3 ${./native-manager.py} "$@"
  '';
  application = pkgs.writeShellScript "test-native-application" ''
    set -eu
    report="$XDG_RUNTIME_DIR/app-$1"
    touch "$report-ready"
    trap 'test -e "$XDG_RUNTIME_DIR/display-alive"; echo saved > "$report"; exit 0' TERM
    while :; do sleep 1; done
  '';
  detached = pkgs.writeShellScript "test-detached-session" ''
    systemctl --user start test-external.service
  '';
in
pkgs.testers.runNixOSTest {
  name = "rsdm-native-de-handoff";
  globalTimeout = 120;
  nodes.machine = { ... }: {
    virtualisation.memorySize = 1024;
    users.users.alice = { isNormalUser = true; uid = 1000; group = "alice"; };
    users.groups.alice = {};
    security.polkit.enable = true;
    security.pam.services.login.startSession = true;
    systemd.user.targets.test-native = {
      wants = [ "graphical-session.target" ];
      after = [ "graphical-session.target" ];
    };
    systemd.user.services.test-external = {
      serviceConfig = {
        Type = "exec";
        ExecStart = "${native} KDE";
      };
    };
    environment.systemPackages = [ rsdm pkgs.coreutils pkgs.util-linux ];
    environment.etc."rsdm.toml".text = "";
  };

  testScript = ''
    import shlex

    start_all()
    machine.wait_for_unit("multi-user.target")
    machine.succeed("loginctl enable-linger alice")
    machine.wait_for_unit("user@1000.service")
    runtime = "/run/user/1000"
    user = "runuser -u alice -- env XDG_RUNTIME_DIR=" + runtime + " DBUS_SESSION_BUS_ADDRESS=unix:path=" + runtime + "/bus "

    for desktop, provider, path in [
        ("GNOME", "gnome", "/org/gnome/SessionManager"),
        ("KDE", "plasma", "/LogoutPrompt"),
    ]:
        with subtest(provider + " retains native dialogs, cancellation and power ownership"):
            machine.wait_until_succeeds("test \"$(systemctl show --property=LoadState --value rsdm-test-session.service)\" = not-found")
            machine.succeed("rm -f " + runtime + "/app-* " + runtime + "/native-*")
            machine.succeed("systemd-run --unit=rsdm-test-session --collect --uid=alice --property=PAMName=login --property=TTYPath=/dev/tty1 --setenv=XDG_SESSION_TYPE=wayland --setenv=XDG_CURRENT_DESKTOP=" + desktop + " --setenv=XDG_RUNTIME_DIR=" + runtime + " --setenv=DBUS_SESSION_BUS_ADDRESS=unix:path=" + runtime + "/bus -- rsdm session start -- ${native} " + desktop + " defer")
            machine.wait_for_file(runtime + "/native-manager-ready")
            # Let the coordinator observe the ready native API without its target.
            machine.sleep(2)
            machine.succeed(user + "rsdm session status | grep ': starting '")
            machine.fail(user + "systemctl --user is-active --quiet graphical-session.target")
            machine.succeed(user + "busctl --user call org.rsdm.TestNative " + shlex.quote(path) + " org.rsdm.TestNative Activate")
            try:
                machine.wait_until_succeeds(user + "rsdm session status | grep '^" + provider + ": running '", timeout=30)
            except Exception:
                print(machine.succeed("journalctl --no-pager -u rsdm-test-session -n 80"))
                raise
            generation = machine.succeed(user + "systemctl --user show-environment | sed -n 's/^RSDM_SESSION_GENERATION=//p'").strip()
            session = user + "RSDM_SESSION_GENERATION=" + generation + " "
            assert "XSMP: unavailable" in machine.succeed(session + "rsdm session status")
            machine.fail(session + "rsdm app --on-timeout cancel -- ${application} rejected")
            machine.fail(session + "rsdm app --shutdown-method xsmp -- ${application} rejected")
            machine.succeed(session + "rsdm app -- ${application} ordinary")
            machine.wait_for_file(runtime + "/app-ordinary-ready")

            for args in ["session stop", "power reboot", "power poweroff"]:
                output = machine.succeed(session + "rsdm " + args)
                assert "delegated" in output, output
                machine.succeed("test -e " + runtime + "/display-alive")
                machine.fail("test -e " + runtime + "/app-ordinary")
                machine.succeed(session + "rsdm session status | grep ': running '")
            assert machine.succeed("cat " + runtime + "/native-requests") == "logout\nreboot\npoweroff\n"

            machine.succeed("touch " + runtime + "/native-deny")
            machine.fail(session + "rsdm session stop")
            machine.succeed("test -e " + runtime + "/display-alive")
            machine.fail("test -e " + runtime + "/app-ordinary")
            machine.succeed("rm " + runtime + "/native-deny")

            machine.succeed(session + "rsdm app --shutdown-method term -- ${application} explicit")
            machine.wait_for_file(runtime + "/app-explicit-ready")
            machine.succeed(session + "rsdm session stop")
            machine.succeed("grep -q saved " + runtime + "/app-explicit")
            machine.fail("test -e " + runtime + "/app-ordinary")
            machine.succeed("test -e " + runtime + "/display-alive")

            machine.succeed(user + "busctl --user call org.rsdm.TestNative " + shlex.quote(path) + " org.rsdm.TestNative Finish")
            machine.wait_until_fails("systemctl is-active --quiet rsdm-test-session")
            machine.succeed("grep -q saved " + runtime + "/app-ordinary")
            machine.fail("test -e " + runtime + "/display-alive")

    with subtest("an external detached launcher is observed through its native graphical lifecycle"):
        machine.wait_until_succeeds("test \"$(systemctl show --property=LoadState --value rsdm-test-session.service)\" = not-found")
        machine.succeed("rm -f " + runtime + "/app-* " + runtime + "/native-*")
        logout = "busctl --user call org.rsdm.TestNative /LogoutPrompt org.rsdm.TestNative Finish"
        argv = shlex.join(["rsdm", "session", "start", "--mode", "external", "--logout-command", logout, "--", "${detached}"])
        machine.succeed("systemd-run --unit=rsdm-test-session --collect --uid=alice --property=PAMName=login --property=TTYPath=/dev/tty1 --setenv=XDG_RUNTIME_DIR=" + runtime + " --setenv=DBUS_SESSION_BUS_ADDRESS=unix:path=" + runtime + "/bus -- " + argv)
        machine.wait_until_succeeds(user + "rsdm session status | grep '^external: running '", timeout=30)
        generation = machine.succeed(user + "systemctl --user show-environment | sed -n 's/^RSDM_SESSION_GENERATION=//p'").strip()
        session = user + "RSDM_SESSION_GENERATION=" + generation + " "
        machine.succeed(user + "systemctl --user is-active --quiet test-external.service")
        machine.succeed(session + "rsdm app -- ${application} detached")
        machine.wait_for_file(runtime + "/app-detached-ready")
        machine.succeed(session + "rsdm session stop")
        machine.succeed("grep -q saved " + runtime + "/app-detached")
        machine.wait_until_fails(user + "systemctl --user is-active --quiet test-external.service")
        machine.fail("test -e " + runtime + "/display-alive")
  '';
}
