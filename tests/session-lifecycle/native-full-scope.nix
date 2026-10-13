{ pkgs, rsdm }:

let
  controller = import ./controller.nix { inherit pkgs; };
  python = pkgs.python3.withPackages (packages: [ packages.dbus-python packages.pygobject3 ]);
  foreign = pkgs.writeShellScript "test-full-foreign-desktop" ''
    exec ${python}/bin/python3 ${./native-manager.py} "$1" defer
  '';
  launcher = pkgs.writeShellScript "test-full-own-launcher" ''
    systemctl --user start test-native.target
    printf '%s\n' "$$" > "$XDG_RUNTIME_DIR/own-launcher-pid"
    touch "$XDG_RUNTIME_DIR/own-launcher-ready"
    exec ${pkgs.coreutils}/bin/sleep infinity
  '';
  application = pkgs.writeShellScript "test-native-partial-application" ''
    trap 'touch "$XDG_RUNTIME_DIR/partial-app-saved"; exit 0' TERM
    printf '%s\n' "$$" > "$XDG_RUNTIME_DIR/partial-app-pid"
    touch "$XDG_RUNTIME_DIR/partial-app-ready"
    while :; do sleep 1; done
  '';
  quit = pkgs.writeShellScript "test-native-partial-quit" ''
    kill -TERM "$(cat "$XDG_RUNTIME_DIR/partial-app-pid")"
    touch "$XDG_RUNTIME_DIR/partial-quit-sent"
  '';
in pkgs.testers.runNixOSTest {
  name = "rsdm-native-full-scope";
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
    machine.succeed(user + "dbus-daemon --session --fork --nopidfile --address=unix:path=" + runtime + "/power-bus")
    private = user + "DBUS_SESSION_BUS_ADDRESS=unix:path=" + runtime + "/power-bus "
    machine.succeed(user + "systemd-run --user --unit=rsdm-test-power --setenv=DBUS_SESSION_BUS_ADDRESS=unix:path=" + runtime + "/power-bus -- ${python}/bin/python3 ${./power-manager.py}")
    machine.wait_for_file(runtime + "/power-manager-ready")
    prepare = private + "busctl --address=unix:path=" + runtime + "/power-bus call org.freedesktop.login1 /org/freedesktop/login1 org.rsdm.TestPower Prepare b "
    released = private + "busctl --address=unix:path=" + runtime + "/power-bus call org.freedesktop.login1 /org/freedesktop/login1 org.rsdm.TestPower ReleasedGuards"
    control = "${controller} system rsdm-test-own-login.service "
    settings = " --collect --uid=alice --property=Type=exec --property=PAMName=login --setenv=XDG_SESSION_TYPE=wayland --setenv=XDG_RUNTIME_DIR=" + runtime + " --setenv=DBUS_SESSION_BUS_ADDRESS=unix:path=" + runtime + "/bus "
    for desktop, provider in [("GNOME", "gnome"), ("KDE", "plasma")]:
        for scope in ["another-login", "user-service"]:
            with subtest(provider + " full shutdown refuses " + scope + " native peers"):
                machine.succeed("rm -f " + runtime + "/native-* " + runtime + "/partial-app-* " + runtime + "/partial-quit-sent " + runtime + "/own-launcher-ready " + runtime + "/display-alive")
                machine.succeed("touch " + runtime + "/power-deny-check")
                if scope == "another-login":
                    machine.succeed("systemd-run --unit=rsdm-test-foreign-native" + settings + " --property=TTYPath=/dev/tty1 -- ${foreign} " + desktop)
                    inspect = "systemctl show rsdm-test-foreign-native -p MainPID -p InvocationID"
                else:
                    machine.succeed(user + "systemd-run --user --unit=rsdm-test-foreign-native --collect --setenv=XDG_RUNTIME_DIR=" + runtime + " --setenv=DBUS_SESSION_BUS_ADDRESS=unix:path=" + runtime + "/bus -- ${foreign} " + desktop)
                    inspect = user + "systemctl --user show rsdm-test-foreign-native -p MainPID -p InvocationID"
                machine.wait_for_file(runtime + "/native-manager-ready")
                foreign_before = machine.succeed(inspect)
                machine.succeed("systemd-run --unit=rsdm-test-own-login" + settings + " --property=TTYPath=/dev/tty2 --setenv=DBUS_SYSTEM_BUS_ADDRESS=unix:path=" + runtime + "/power-bus --setenv=XDG_CURRENT_DESKTOP=" + desktop + " -- rsdm session start -- ${launcher}")
                machine.wait_for_file(runtime + "/own-launcher-ready")
                machine.wait_until_succeeds(control + "rsdm session status | grep '^" + provider + ": running '", timeout=30)
                machine.succeed(control + "rsdm app --shutdown-timeout 2 --on-timeout cancel -- ${application}")
                machine.wait_for_file(runtime + "/partial-app-ready")
                for args in ["session stop", "power reboot", "power poweroff"]:
                    code, output = machine.execute(control + "rsdm " + args)
                    if machine.execute("test ! -e " + runtime + "/native-requests")[0] != 0:
                        print("foreign native requests", machine.succeed("cat " + runtime + "/native-requests"))
                    machine.succeed("test ! -e " + runtime + "/native-requests")
                    assert "delegated" not in output, output
                    if args == "session stop":
                        assert code == 0 and "prepared" in output, output
                        machine.wait_for_file(runtime + "/partial-app-saved")
                    else:
                        assert code != 0, output
                    assert machine.succeed(inspect) == foreign_before
                    machine.succeed("test -e " + runtime + "/display-alive")
                    machine.succeed("systemctl is-active --quiet rsdm-test-own-login")
                status = machine.succeed(control + "rsdm session status")
                assert "capability: native-logout: unavailable" in status, status
                assert "manager: partial\n" in status, status
                assert "capability: compositor-lifecycle: unavailable" in status, status
                assert "Native desktop lifecycle control could not be verified" in status, status
                generation = machine.succeed(user + "systemctl --user show-environment | sed -n 's/^RSDM_SESSION_GENERATION=//p'").strip()
                directory = runtime + "/rsdm/sessions/" + generation
                record = tomllib.loads(machine.succeed("cat " + directory + "/session.toml"))
                environment_before = machine.succeed(user + "systemctl --user show-environment")
                login_before = machine.succeed("systemctl show rsdm-test-own-login -p MainPID -p InvocationID")
                released_before = int(machine.succeed(released).split()[1])
                requests_before = machine.succeed("cat " + runtime + "/power-requests 2>/dev/null || true").splitlines()
                quit_case = provider == "gnome" and scope == "user-service"
                if quit_case:
                    machine.succeed("rm -f " + runtime + "/partial-app-*")
                    machine.succeed(control + "rsdm app --shutdown-timeout 2 --on-timeout cancel --quit-command ${quit} -- ${application}")
                    machine.wait_for_file(runtime + "/partial-app-ready")
                    machine.succeed(user + "systemctl --user stop " + record["anchor_unit"])
                    machine.wait_for_file(runtime + "/partial-quit-sent")
                    machine.wait_for_file(runtime + "/partial-app-saved")
                else:
                    machine.succeed("rm -f " + runtime + "/power-deny-check")
                    action = "reboot" if scope == "another-login" else "poweroff"
                    output = machine.succeed(control + "rsdm power " + action)
                    assert "prepared" in output and "delegated" not in output, output
                machine.wait_until_fails(user + "busctl --user list | grep org.rsdm.Session1")
                machine.wait_until_succeeds(released + " | grep '^u " + str(released_before + 1) + "$'", timeout=10)
                requests = machine.succeed("cat " + runtime + "/power-requests 2>/dev/null || true").splitlines()
                assert requests == requests_before + ([] if quit_case else ["1000"])
                closed = tomllib.loads(machine.succeed("cat " + directory + "/session.toml"))
                assert closed["phase"] == "closed" and closed["shutdown_transaction"]["result"] == "prepared", closed
                assert machine.succeed(inspect) == foreign_before
                assert machine.succeed("systemctl show rsdm-test-own-login -p MainPID -p InvocationID") == login_before
                assert machine.succeed(user + "systemctl --user show-environment") == environment_before
                machine.succeed(user + "systemctl --user is-active --quiet graphical-session.target")
                if quit_case:
                    machine.fail(user + "systemctl --user is-active --quiet " + record["anchor_unit"])
                else:
                    machine.succeed(user + "systemctl --user is-active --quiet " + record["anchor_unit"])
                machine.succeed("test -e " + runtime + "/display-alive; kill -0 $(cat " + runtime + "/own-launcher-pid)")
                machine.succeed("test ! -e " + runtime + "/native-requests")
                machine.succeed(prepare + "false")
                machine.succeed("kill -TERM $(cat " + runtime + "/own-launcher-pid)")
                machine.wait_until_succeeds("test \"$(systemctl show --property=LoadState --value rsdm-test-own-login.service)\" = not-found")
                if scope == "another-login":
                    machine.succeed("systemctl stop rsdm-test-foreign-native")
                else:
                    machine.succeed(user + "systemctl --user stop rsdm-test-foreign-native")
                machine.succeed(user + "systemctl --user stop test-native.target graphical-session.target")
                machine.wait_until_fails(user + "busctl --user list | grep org.rsdm.Session1")
  '';
}
