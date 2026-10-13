{ pkgs, rsdm }:
let
  controller = import ./controller.nix { inherit pkgs; };
  python = pkgs.python3.withPackages (packages: [ packages.dbus-python packages.pygobject3 ]);
  native = pkgs.writeShellScript "test-attached-native" ''
    exec ${python}/bin/python3 ${./native-manager.py} "$@" defer
  '';
  application = pkgs.writeShellScript "test-attached-native-app" ''
    set -eu
    trap 'test -e "$XDG_RUNTIME_DIR/display-alive"; echo saved > "$XDG_RUNTIME_DIR/app-attached"; exit 0' TERM
    touch "$XDG_RUNTIME_DIR/app-attached-ready"
    while :; do sleep 1; done
  '';
  autoApplication = pkgs.writeShellScript "test-attached-native-auto-app" ''
    set -eu
    trap 'touch "$XDG_RUNTIME_DIR/native-auto-term"; exit 0' TERM
    printf '%s\n' "$$" > "$XDG_RUNTIME_DIR/native-auto-pid"
    touch "$XDG_RUNTIME_DIR/native-auto-ready"
    while :; do sleep 1; done
  '';
in pkgs.testers.runNixOSTest {
  name = "rsdm-native-attached";
  globalTimeout = 120;
  nodes.machine = { ... }: {
    virtualisation.memorySize = 1024;
    users.users.alice = { isNormalUser = true; uid = 1000; group = "alice"; };
    users.groups.alice = {};
    security.polkit.enable = true;
    security.pam.services.login.startSession = true;
    environment.systemPackages = [ rsdm pkgs.coreutils pkgs.util-linux pkgs.dbus ];
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
    control = "${controller} system rsdm-test-native-attached.service "
    foreign = "${controller} system rsdm-test-foreign-login.service "
    settings = " --collect --uid=alice --property=Type=exec --property=PAMName=login --setenv=XDG_SESSION_TYPE=wayland --setenv=XDG_RUNTIME_DIR=" + runtime + " --setenv=DBUS_SESSION_BUS_ADDRESS=unix:path=" + runtime + "/bus "

    for desktop in ["GNOME", "KDE"]:
        with subtest(desktop + " attached logout prepares explicit apps and leaves auto apps to native dialogs"):
            machine.succeed("rm -f " + runtime + "/app-attached* " + runtime + "/native-* " + runtime + "/display-alive")
            machine.succeed("systemd-run --unit=rsdm-test-native-attached" + settings + " --property=TTYPath=/dev/tty1 -- ${native} " + desktop)
            machine.wait_for_file(runtime + "/native-manager-ready")
            wm = machine.succeed("systemctl show rsdm-test-native-attached -p MainPID -p InvocationID")
            environment = machine.succeed(user + "systemctl --user show-environment")
            machine.succeed(control + "rsdm session attach")
            status = machine.succeed(control + "rsdm session status")
            assert status.startswith("partial: running "), status
            assert "capability: native-logout: available" in status, status
            machine.fail("test -e " + runtime + "/native-requests")
            machine.fail(user + "systemctl --user is-active --quiet graphical-session.target")
            machine.succeed(control + "rsdm app --shutdown-method term -- ${application}")
            machine.wait_for_file(runtime + "/app-attached-ready")
            machine.succeed(control + "rsdm app --shutdown-timeout 1 -- ${autoApplication}")
            machine.wait_for_file(runtime + "/native-auto-ready")
            auto_pid = machine.succeed("cat " + runtime + "/native-auto-pid").strip()
            machine.succeed("echo app-attached > " + runtime + "/native-require-saved")
            result = machine.succeed(control + "rsdm session stop")
            assert "delegated" in result, result
            assert machine.succeed("cat " + runtime + "/app-attached").strip() == "saved"
            assert machine.succeed("cat " + runtime + "/native-requests") == "logout\n"
            machine.fail("test -e " + runtime + "/native-auto-term")
            machine.succeed("kill -0 " + auto_pid)
            assert machine.succeed("systemctl show rsdm-test-native-attached -p MainPID -p InvocationID") == wm
            assert machine.succeed(user + "systemctl --user show-environment") == environment
            machine.succeed("test -e " + runtime + "/display-alive")
            machine.succeed(control + "rsdm session status | grep '^partial: running '")
            machine.succeed(user + "systemctl --user stop rsdm-session-manager.service")

        with subtest(desktop + " peers in another login of the same UID remain untouched"):
            machine.succeed("systemd-run --unit=rsdm-test-foreign-login" + settings + " --property=TTYPath=/dev/tty2 -- ${pkgs.coreutils}/bin/sleep infinity")
            machine.wait_for_unit("rsdm-test-foreign-login.service")
            machine.succeed(foreign + "rsdm session attach")
            status = machine.succeed(foreign + "rsdm session status")
            assert "capability: native-logout: unavailable" in status, status
            result = machine.succeed(foreign + "rsdm session stop")
            assert "prepared" in result, result
            assert machine.succeed("cat " + runtime + "/native-requests") == "logout\n"
            assert machine.succeed("systemctl show rsdm-test-native-attached -p MainPID -p InvocationID") == wm
            machine.succeed("test -e " + runtime + "/display-alive")
            machine.succeed(user + "systemctl --user stop rsdm-session-manager.service")
            machine.succeed("systemctl stop rsdm-test-foreign-login rsdm-test-native-attached")
            machine.wait_until_succeeds("test \"$(systemctl show --property=LoadState --value rsdm-test-native-attached.service)\" = not-found")

    with subtest("a user-service native peer without a logind session cannot receive attached logout"):
        machine.succeed("rm -f " + runtime + "/native-* " + runtime + "/display-alive")
        machine.succeed(user + "systemd-run --user --unit=rsdm-test-unscoped-native --collect --setenv=XDG_RUNTIME_DIR=" + runtime + " --setenv=DBUS_SESSION_BUS_ADDRESS=unix:path=" + runtime + "/bus -- ${native} GNOME")
        machine.wait_for_file(runtime + "/native-manager-ready")
        machine.succeed("systemd-run --unit=rsdm-test-foreign-login" + settings + " --property=TTYPath=/dev/tty2 -- ${pkgs.coreutils}/bin/sleep infinity")
        machine.wait_for_unit("rsdm-test-foreign-login.service")
        machine.succeed(foreign + "rsdm session attach")
        status = machine.succeed(foreign + "rsdm session status")
        assert "capability: native-logout: unavailable" in status, status
        assert "prepared" in machine.succeed(foreign + "rsdm session stop")
        machine.fail("test -e " + runtime + "/native-requests")
        machine.succeed(user + "systemctl --user is-active --quiet rsdm-test-unscoped-native.service")
        machine.succeed("test -e " + runtime + "/display-alive")
        machine.succeed(user + "systemctl --user stop rsdm-session-manager rsdm-test-unscoped-native")
        machine.succeed("systemctl stop rsdm-test-foreign-login")

    with subtest("logind during a native reply prepares remaining auto apps and preserves the external WM"):
        machine.succeed("rm -f " + runtime + "/native-* " + runtime + "/display-alive")
        machine.succeed(user + "dbus-daemon --session --fork --nopidfile --address=unix:path=" + runtime + "/native-power-bus")
        machine.succeed(user + "systemd-run --user --unit=rsdm-test-native-power --setenv=DBUS_SESSION_BUS_ADDRESS=unix:path=" + runtime + "/native-power-bus -- ${python}/bin/python3 ${./power-manager.py}")
        machine.wait_for_file(runtime + "/power-manager-ready")
        power = user + "busctl --address=unix:path=" + runtime + "/native-power-bus call org.freedesktop.login1 /org/freedesktop/login1 org.rsdm.TestPower "
        native_control = control + "DBUS_SYSTEM_BUS_ADDRESS=unix:path=" + runtime + "/native-power-bus "
        machine.succeed("systemd-run --unit=rsdm-test-native-attached" + settings + " --property=TTYPath=/dev/tty1 --setenv=DBUS_SYSTEM_BUS_ADDRESS=unix:path=" + runtime + "/native-power-bus -- ${native} GNOME")
        machine.wait_for_file(runtime + "/native-manager-ready")
        wm = machine.succeed("systemctl show rsdm-test-native-attached -p MainPID -p InvocationID")
        environment = machine.succeed(user + "systemctl --user show-environment")
        machine.succeed(native_control + "rsdm session attach")
        assert "DBUS_SYSTEM_BUS_ADDRESS=unix:path=" + runtime + "/native-power-bus" in machine.succeed(user + "systemctl --user show rsdm-session-manager.service -p Environment --value")
        generation = machine.succeed(native_control + "rsdm session status").split("generation ")[1].split(")")[0]
        record_path = runtime + "/rsdm/sessions/" + generation + "/session.toml"
        machine.succeed(native_control + "rsdm app --shutdown-timeout 1 -- ${autoApplication}")
        machine.wait_for_file(runtime + "/native-auto-ready")
        machine.succeed("touch " + runtime + "/native-reply-hold")
        machine.succeed(native_control + "sh -c 'rsdm session stop > /run/user/1000/native-stop-result 2>&1 &'")
        machine.wait_for_file(runtime + "/native-requests")
        original_budget = tomllib.loads(machine.succeed("cat " + record_path))["shutdown_deadline_usec"]
        machine.succeed(power + "Prepare b true")
        machine.wait_until_succeeds("test \"$(sed -n 's/^shutdown_deadline_usec = //p' " + record_path + ")\" -lt " + str(original_budget))
        budget = tomllib.loads(machine.succeed("cat " + record_path))["shutdown_deadline_usec"]
        machine.succeed("touch " + runtime + "/native-reply-release")
        machine.wait_for_file(runtime + "/native-auto-term")
        machine.wait_until_fails(user + "busctl --user list | grep org.rsdm.Session1")
        machine.wait_until_succeeds(power + "ReleasedGuards | grep '^u 1$'")
        closed = tomllib.loads(machine.succeed("cat " + record_path))
        assert closed["phase"] == "closed" and closed["shutdown_transaction"]["result"] == "prepared", closed
        assert closed["shutdown_deadline_usec"] == budget and closed["shutdown_transaction"]["deadline_usec"] == budget
        assert machine.succeed("cat " + runtime + "/native-requests") == "logout\n"
        assert machine.succeed("systemctl show rsdm-test-native-attached -p MainPID -p InvocationID") == wm
        assert machine.succeed(user + "systemctl --user show-environment") == environment
        machine.succeed("test -e " + runtime + "/display-alive")
        machine.fail("test -e " + runtime + "/power-requests")
        machine.succeed("systemctl stop rsdm-test-native-attached")
  '';
}
