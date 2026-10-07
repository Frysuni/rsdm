{ pkgs, rsdm }:

let
  application = pkgs.writeShellScript "test-niri-application" ''
    set -eu
    trap 'systemctl --user is-active --quiet niri.service; test -S "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY"; touch "$XDG_RUNTIME_DIR/niri-app-saved"; exit 0' TERM
    touch "$XDG_RUNTIME_DIR/niri-app-ready"
    while :; do sleep 1; done
  '';
  stubborn = pkgs.writeShellScript "test-niri-stubborn" ''
    trap "" TERM INT
    touch "$XDG_RUNTIME_DIR/niri-stubborn-ready"
    while :; do sleep 1; done
  '';
in
pkgs.testers.runNixOSTest {
  name = "rsdm-native-niri";
  globalTimeout = 180;
  nodes.machine = { ... }: {
    virtualisation.memorySize = 2048;
    virtualisation.qemu.options = [ "-vga none -device virtio-gpu-pci" ];
    hardware.graphics.enable = true;
    security.polkit.enable = true;
    security.polkit.extraConfig = ''
      polkit.addRule(function(action, subject) {
        if (subject.user == "alice" && action.id.indexOf("org.freedesktop.login1.power-off") == 0) {
          return polkit.Result.NO;
        }
      });
    '';
    users.users.alice = { isNormalUser = true; uid = 1000; group = "alice"; };
    users.groups.alice = {};
    services.getty.autologinUser = "alice";
    systemd.packages = [ pkgs.niri ];
    environment.systemPackages = [ rsdm pkgs.niri pkgs.procps ];
    environment.etc."rsdm.toml".text = "";
    environment.etc."xdg/niri/config.kdl".text = ''
      prefer-no-csd
      animations { off; }
    '';
    environment.variables.LIBGL_ALWAYS_SOFTWARE = "1";
    programs.bash.loginShellInit = ''
      if [ "$(tty)" = /dev/tty1 ] && [ -z "''${RSDM_SESSION_GENERATION:-}" ]; then
        ${rsdm}/bin/rsdm session start -- ${pkgs.niri}/bin/niri-session > /tmp/rsdm-niri-coordinator.log 2>&1
        touch /tmp/rsdm-niri-exited
      fi
    '';
  };
  testScript = ''
    start_all()
    machine.wait_for_unit("multi-user.target")
    user = "runuser -u alice -- env XDG_RUNTIME_DIR=/run/user/1000 DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus "
    try:
        machine.wait_until_succeeds(user + "rsdm session status | grep '^niri: running '", timeout=60)
    except Exception:
        print(machine.succeed("journalctl --no-pager -n 150"))
        raise
    environment = machine.succeed(user + "systemctl --user show-environment")
    values = dict(line.split("=", 1) for line in environment.splitlines())
    session = user + "RSDM_SESSION_GENERATION=" + values["RSDM_SESSION_GENERATION"] + " WAYLAND_DISPLAY=" + values["WAYLAND_DISPLAY"] + " NIRI_SOCKET=" + values["NIRI_SOCKET"] + " "

    with subtest("the original wrapper owns a real notify niri.service"):
        assert machine.succeed(user + "systemctl --user show niri.service --property=Type --value").strip() == "notify"
        machine.succeed(user + "systemctl --user is-active --quiet niri.service")
        machine.succeed(session + "niri msg version")
        machine.fail(user + "systemctl --user list-units --all --plain --no-legend 'session-rsdm-*' | grep session-rsdm")
        machine.succeed(session + "rsdm session status | grep 'RSDM XSMP: available'")

    with subtest("denied power preserves native niri and registered apps"):
        machine.succeed(session + "rsdm app -- ${application}")
        machine.wait_for_file("/run/user/1000/niri-app-ready")
        machine.fail(session + "rsdm power poweroff")
        machine.fail("test -e /run/user/1000/niri-app-saved")
        machine.succeed(user + "systemctl --user is-active --quiet niri.service")

    with subtest("cancel preserves the native compositor and then apps save before native stop"):
        machine.succeed(session + "rsdm app --shutdown-timeout 1 --on-timeout cancel -- ${stubborn}")
        machine.wait_for_file("/run/user/1000/niri-stubborn-ready")
        machine.fail(session + "rsdm session stop")
        machine.succeed(session + "niri msg version")
        machine.succeed(user + "systemctl --user is-active --quiet niri.service")
        machine.succeed(user + "systemctl --user kill --signal=KILL app-rsdm-*service")
        machine.succeed("rm -f /run/user/1000/niri-app-ready")
        machine.succeed(session + "rsdm app -- ${application}")
        machine.wait_for_file("/run/user/1000/niri-app-ready")
        machine.succeed(session + "rsdm session stop")
        machine.succeed("test -e /run/user/1000/niri-app-saved")
        machine.wait_until_fails(user + "systemctl --user is-active --quiet niri.service")
        machine.wait_for_file("/tmp/rsdm-niri-exited")
  '';
}
