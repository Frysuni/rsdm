{ pkgs, rsdm }:
let
  controller = import ./controller.nix { inherit pkgs; };
  external = pkgs.writeShellScript "test-external-wm" ''
    set -eu
    touch "$XDG_RUNTIME_DIR/attach-wm-ready"
    trap 'touch "$XDG_RUNTIME_DIR/attach-wm-stopped"; exit 0' TERM INT
    while :; do sleep 1; done
  '';
  application = pkgs.writeShellScript "test-attached-app" ''
    set -eu
    echo "$RSDM_SESSION_GENERATION" > "$XDG_RUNTIME_DIR/attach-app-generation"
    echo "$SESSION_MANAGER" > "$XDG_RUNTIME_DIR/attach-app-xsmp"
    trap 'touch "$XDG_RUNTIME_DIR/attach-app-saved"; exit 0' TERM
    touch "$XDG_RUNTIME_DIR/attach-app-ready"
    while :; do sleep 1; done
  '';
  commandApplication = pkgs.writeShellScript "test-attached-command-app" ''
    set -eu
    trap "" TERM INT
    fifo="$XDG_RUNTIME_DIR/attach-quit-request"
    mkfifo "$fifo"
    touch "$XDG_RUNTIME_DIR/attach-command-ready"
    read -r request < "$fifo"
    test "$request" = quit
    test ! -e "$XDG_RUNTIME_DIR/attach-wm-stopped"
    touch "$XDG_RUNTIME_DIR/attach-command-saved"
    rm "$fifo"
  '';
  quitCommand = pkgs.writeShellScript "test-attached-quit-command" ''
    printf 'quit\n' > "$XDG_RUNTIME_DIR/attach-quit-request"
  '';
in pkgs.testers.runNixOSTest {
  name = "rsdm-session-attach";
  globalTimeout = 120;
  nodes.machine = { ... }: {
    virtualisation.memorySize = 1024;
    users.users.alice = { isNormalUser = true; uid = 1000; group = "alice"; };
    users.groups.alice = {};
    security.polkit.enable = true;
    security.pam.services.login.startSession = true;
    environment.systemPackages = [ rsdm pkgs.coreutils pkgs.util-linux ];
    environment.etc."rsdm.toml".text = "";
  };
  testScript = ''
    import tomllib

    start_all()
    machine.wait_for_unit("multi-user.target")
    machine.succeed("loginctl enable-linger alice")
    machine.wait_for_unit("user@1000.service")
    runtime = "/run/user/1000"
    user = "runuser -u alice -- env XDG_RUNTIME_DIR=" + runtime + " DBUS_SESSION_BUS_ADDRESS=unix:path=" + runtime + "/bus "
    control = "${controller} system rsdm-test-external-session.service "
    machine.succeed("systemd-run --unit=rsdm-test-external-session --collect --uid=alice --property=PAMName=login --property=TTYPath=/dev/tty1 --setenv=XDG_SESSION_TYPE=wayland --setenv=XDG_RUNTIME_DIR=" + runtime + " --setenv=DBUS_SESSION_BUS_ADDRESS=unix:path=" + runtime + "/bus -- ${external}")
    machine.wait_for_file(runtime + "/attach-wm-ready")
    before = machine.succeed(user + "systemctl --user show-environment")
    wm = machine.succeed("systemctl show rsdm-test-external-session -p MainPID -p InvocationID")
    with subtest("attach owns an independent service and no graphical targets"):
        try:
            machine.succeed(control + "rsdm session attach")
        except Exception:
            print(machine.succeed(user + "journalctl --user --no-pager -u rsdm-session-manager -n 60"))
            raise
        status = machine.succeed(control + "rsdm session status")
        assert status.startswith("partial: running "), status
        generation = status.split("generation ")[1].split(")")[0]
        record = tomllib.loads(machine.succeed("cat " + runtime + "/rsdm/sessions/" + generation + "/session.toml"))
        login_session_id = record["identity"]["login_session_id"]
        assert record["provider"] == "partial" and not record["owns_targets"]
        assert "compositor_unit" not in record
        assert record["coordinator_unit"][0] == "rsdm-session-manager.service"
        assert machine.succeed(user + "systemctl --user show-environment") == before
        machine.fail(user + "systemctl --user is-active --quiet graphical-session.target")
        machine.succeed(control + "rsdm session attach")
        assert machine.succeed("systemctl show rsdm-test-external-session -p MainPID -p InvocationID") == wm
    with subtest("registered app preparation leaves the external WM and PAM active"):
        session = control + "RSDM_SESSION_GENERATION=" + generation + " "
        machine.succeed(session + "rsdm app -- ${application}")
        machine.wait_for_file(runtime + "/attach-app-ready")
        assert machine.succeed("cat " + runtime + "/attach-app-generation").strip() == generation
        assert machine.succeed("cat " + runtime + "/attach-app-xsmp").strip()
        output = machine.succeed(session + "rsdm session stop")
        assert "prepared" in output, output
        machine.wait_for_file(runtime + "/attach-app-saved")
        assert machine.succeed("systemctl show rsdm-test-external-session -p MainPID -p InvocationID") == wm
        machine.succeed("loginctl show-session " + record["identity"]["login_session_id"])
        machine.fail("test -e " + runtime + "/attach-wm-stopped")
        assert machine.succeed(user + "systemctl --user show-environment") == before
    with subtest("quit-command helpers use the independent coordinator without a graphical anchor"):
        machine.succeed(session + "rsdm app --quit-command ${quitCommand} -- ${commandApplication}")
        machine.wait_for_file(runtime + "/attach-command-ready")
        output = machine.succeed(session + "rsdm session stop")
        assert "prepared" in output, output
        machine.wait_for_file(runtime + "/attach-command-saved")
        machine.fail(user + "systemctl --user is-active --quiet graphical-session.target")
        assert machine.succeed("systemctl show rsdm-test-external-session -p MainPID -p InvocationID") == wm
        assert machine.succeed(user + "systemctl --user show-environment") == before
    with subtest("hot disable drops only the attached coordinator"):
        machine.succeed("printf '[session_manager]\\nenabled = false\\n' > /etc/rsdm-toggle.toml; mv /etc/rsdm-toggle.toml /etc/rsdm.toml")
        machine.wait_until_fails(user + "systemctl --user is-active --quiet rsdm-session-manager.service")
        machine.wait_until_fails(user + "busctl --user list | grep org.rsdm.Session1")
        assert machine.succeed("systemctl show rsdm-test-external-session -p MainPID -p InvocationID") == wm
        machine.fail("test -e " + runtime + "/attach-wm-stopped")
        assert machine.succeed(user + "systemctl --user show-environment") == before
    ${import ./attach-scenarios.nix { inherit application; }}
    machine.succeed("systemctl stop rsdm-test-external-session")
  '';
}
