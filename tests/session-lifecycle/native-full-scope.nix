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
    environment.systemPackages = [ rsdm pkgs.coreutils pkgs.util-linux ];
    environment.etc."rsdm.toml".text = "";
  };
  testScript = ''
    start_all()
    machine.wait_for_unit("multi-user.target")
    machine.succeed("loginctl enable-linger alice")
    machine.wait_for_unit("user@1000.service")
    runtime = "/run/user/1000"
    user = "runuser -u alice -- env XDG_RUNTIME_DIR=" + runtime + " DBUS_SESSION_BUS_ADDRESS=unix:path=" + runtime + "/bus "
    control = "${controller} system rsdm-test-own-login.service "
    settings = " --collect --uid=alice --property=Type=exec --property=PAMName=login --setenv=XDG_SESSION_TYPE=wayland --setenv=XDG_RUNTIME_DIR=" + runtime + " --setenv=DBUS_SESSION_BUS_ADDRESS=unix:path=" + runtime + "/bus "
    for desktop, provider in [("GNOME", "gnome"), ("KDE", "plasma")]:
        for scope in ["another-login", "user-service"]:
            with subtest(provider + " full shutdown refuses " + scope + " native peers"):
                machine.succeed("rm -f " + runtime + "/native-* " + runtime + "/own-launcher-ready " + runtime + "/display-alive")
                if scope == "another-login":
                    machine.succeed("systemd-run --unit=rsdm-test-foreign-native" + settings + " --property=TTYPath=/dev/tty1 -- ${foreign} " + desktop)
                    inspect = "systemctl show rsdm-test-foreign-native -p MainPID -p InvocationID"
                else:
                    machine.succeed(user + "systemd-run --user --unit=rsdm-test-foreign-native --collect --setenv=XDG_RUNTIME_DIR=" + runtime + " --setenv=DBUS_SESSION_BUS_ADDRESS=unix:path=" + runtime + "/bus -- ${foreign} " + desktop)
                    inspect = user + "systemctl --user show rsdm-test-foreign-native -p MainPID -p InvocationID"
                machine.wait_for_file(runtime + "/native-manager-ready")
                foreign_before = machine.succeed(inspect)
                machine.succeed("systemd-run --unit=rsdm-test-own-login" + settings + " --property=TTYPath=/dev/tty2 --setenv=XDG_CURRENT_DESKTOP=" + desktop + " -- rsdm session start -- ${launcher}")
                machine.wait_for_file(runtime + "/own-launcher-ready")
                machine.wait_until_succeeds(control + "rsdm session status | grep '^" + provider + ": running '", timeout=30)
                for args in ["session stop", "power reboot", "power poweroff"]:
                    code, output = machine.execute(control + "rsdm " + args)
                    if machine.execute("test ! -e " + runtime + "/native-requests")[0] != 0:
                        print("foreign native requests", machine.succeed("cat " + runtime + "/native-requests"))
                    machine.succeed("test ! -e " + runtime + "/native-requests")
                    assert code != 0 and "delegated" not in output, output
                    assert machine.succeed(inspect) == foreign_before
                    machine.succeed("test -e " + runtime + "/display-alive")
                    machine.succeed("systemctl is-active --quiet rsdm-test-own-login")
                status = machine.succeed(control + "rsdm session status")
                assert "capability: native-logout: unavailable" in status, status
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
