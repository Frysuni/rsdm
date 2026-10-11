{ pkgs, rsdm }:
let
  compositor = pkgs.writeShellScript "test-bootstrap-compositor" ''
    set -eu
    report="$XDG_RUNTIME_DIR/bootstrap"
    echo $$ >> "$report-starts"
    pwd > "$report-directory"
    printf '<%s>\n' "$@" > "$report-arguments"
    printf '%s\n' "$BOOTSTRAP_TEST" > "$report-environment"
    printf '%s\n' "$RSDM_SESSION_GENERATION" > "$report-generation"
    trap 'touch "$report-terminated"; exit 17' TERM INT
    while ! test -e "$report-ready"; do sleep 0.05; done
    ${rsdm}/bin/rsdm session finalize
    touch "$report-finalized"
    while ! test -e "$report-exit"; do sleep 0.05; done
    exit 7
  '';
in
''
    def launch_bootstrap(generation):
        machine.succeed("rm -f " + runtime + "/bootstrap-*")
        command = [
            "systemd-run", "--unit=rsdm-test-bootstrap", "--uid=alice", "--expand-environment=no",
            "--property=PAMName=login", "--property=TTYPath=/dev/tty1",
            "--property=WorkingDirectory=/home/alice", "--setenv=XDG_SESSION_TYPE=wayland",
            "--setenv=XDG_RUNTIME_DIR=" + runtime,
            "--setenv=DBUS_SESSION_BUS_ADDRESS=unix:path=" + runtime + "/bus",
            "--setenv=RSDM_SESSION_GENERATION=" + generation, "--setenv=BOOTSTRAP_TEST=two words",
            "rsdm", "session", "start", "--mode", "managed", "--",
            "${compositor}", "$HOME", "", "--flag", "two words",
        ]
        machine.succeed(shlex.join(command))

    with subtest("disable before WM dispatch launches the original command once and holds PAM"):
        generation = "ffffffffffffffffffffffffffffff10"
        before = machine.succeed(user + "systemctl --user show-environment")
        machine.succeed("systemctl kill --kill-whom=main --signal=STOP user@1000.service")
        machine.sleep(1)
        launch_bootstrap(generation)
        machine.wait_until_succeeds(user + "busctl --user list | grep org.rsdm.Session1")
        machine.fail("test -e " + runtime + "/bootstrap-starts")
        machine.succeed("printf '[session_manager]\\nenabled = false\\n' > /etc/rsdm-toggle.toml; mv /etc/rsdm-toggle.toml /etc/rsdm.toml")
        machine.wait_for_file(runtime + "/bootstrap-starts")
        machine.wait_until_fails(user + "busctl --user list | grep org.rsdm.Session1")
        machine.succeed("systemctl kill --kill-whom=main --signal=CONT user@1000.service")
        assert machine.succeed(user + "systemctl --user show-environment") == before
        machine.fail("test -e " + runtime + "/rsdm/sessions/" + generation)
        machine.fail(user + "systemctl --user list-units --all --no-legend 'rsdm-session-*' | grep " + generation)
        machine.fail("systemd-inhibit --list --no-pager | grep 'Saving registered graphical applications'")
        machine.succeed("touch " + runtime + "/bootstrap-ready")
        machine.wait_for_file(runtime + "/bootstrap-finalized")
        machine.succeed("systemctl is-active --quiet rsdm-test-bootstrap")
        arguments = machine.succeed("cat " + runtime + "/bootstrap-arguments")
        assert arguments == "<$HOME>\n<>\n<--flag>\n<two words>\n", arguments
        assert machine.succeed("cat " + runtime + "/bootstrap-environment") == "two words\n"
        assert machine.succeed("cat " + runtime + "/bootstrap-directory") == "/home/alice\n"
        assert machine.succeed("cat " + runtime + "/bootstrap-generation").strip() == generation
        assert len(machine.succeed("cat " + runtime + "/bootstrap-starts").splitlines()) == 1
        machine.fail("test -e " + runtime + "/bootstrap-terminated")
        machine.succeed("touch " + runtime + "/bootstrap-exit")
        machine.wait_until_fails("systemctl is-active --quiet rsdm-test-bootstrap")
        assert machine.succeed("systemctl show rsdm-test-bootstrap -p ExecMainStatus --value").strip() == "7"
        machine.succeed("systemctl reset-failed rsdm-test-bootstrap")
        machine.succeed("printf '[session_manager]\\nenabled = true\\n' > /etc/rsdm-toggle.toml; mv /etc/rsdm-toggle.toml /etc/rsdm.toml")

    with subtest("disable after WM dispatch preserves the starting invocation until its real exit"):
        generation = "ffffffffffffffffffffffffffffff11"
        launch_bootstrap(generation)
        machine.wait_for_file(runtime + "/bootstrap-starts")
        directory = runtime + "/rsdm/sessions/" + generation
        record = tomllib.loads(machine.succeed("cat " + directory + "/session.toml"))
        machine.succeed(user + "rsdm session status | grep ': starting '")
        wm = machine.succeed(user + "systemctl --user show " + record["compositor_unit"] + " -p MainPID -p InvocationID")
        launcher = machine.succeed("systemctl show rsdm-test-bootstrap -p MainPID -p InvocationID")
        machine.succeed("printf '[session_manager]\\nenabled = false\\n' > /etc/rsdm-toggle.toml; mv /etc/rsdm-toggle.toml /etc/rsdm.toml")
        machine.wait_until_succeeds("grep -q '^disabled = true$' " + directory + "/session.toml")
        machine.wait_until_fails(user + "busctl --user list | grep org.rsdm.Session1")
        machine.wait_until_fails("systemd-inhibit --list --no-pager | grep 'Saving registered graphical applications'")
        machine.wait_until_fails("test -e " + shlex.quote(dict(record["exported_environment"])["ICEAUTHORITY"]))
        machine.succeed("touch " + runtime + "/bootstrap-ready")
        machine.wait_for_file(runtime + "/bootstrap-finalized")
        machine.succeed("printf '[session_manager]\\nenabled = true\\n' > /etc/rsdm-toggle.toml; mv /etc/rsdm-toggle.toml /etc/rsdm.toml")
        machine.succeed(user + "RSDM_SESSION_GENERATION=" + generation + " rsdm session cleanup --generation " + generation)
        assert machine.succeed(user + "systemctl --user show " + record["compositor_unit"] + " -p MainPID -p InvocationID") == wm
        assert machine.succeed("systemctl show rsdm-test-bootstrap -p MainPID -p InvocationID") == launcher
        machine.succeed("loginctl show-session " + record["identity"]["login_session_id"])
        machine.fail(user + "systemctl --user is-active --quiet graphical-session.target")
        machine.fail("test -e " + runtime + "/bootstrap-terminated")
        assert len(machine.succeed("cat " + runtime + "/bootstrap-starts").splitlines()) == 1
        machine.succeed("touch " + runtime + "/bootstrap-exit")
        machine.wait_until_fails("systemctl is-active --quiet rsdm-test-bootstrap")
        assert machine.succeed("systemctl show rsdm-test-bootstrap -p ExecMainStatus --value").strip() == "7"
        machine.succeed("systemctl reset-failed rsdm-test-bootstrap")
''
