{ pkgs, rsdm }:

let
  module = import ../../packaging/nix/module.nix {
    packages.${pkgs.stdenv.hostPlatform.system}.rsdm-stable = rsdm;
  };
  compositor = pkgs.writeShellScript "test-dm-sway" ''
    export WLR_RENDERER=pixman
    exec ${pkgs.sway}/bin/sway --config /etc/sway/config
  '';
  application = pkgs.writeShellScript "test-dm-application" ''
    set -eu
    trap 'test -S "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY"; touch /tmp/dm-app-saved; exit 0' TERM
    echo "$$" > "$XDG_RUNTIME_DIR/dm-app-ready"
    while :; do sleep 1; done
  '';
in
pkgs.testers.runNixOSTest {
  name = "rsdm-dm-lifecycle";
  globalTimeout = 180;
  nodes.machine = { ... }: {
    imports = [ module ];
    virtualisation.memorySize = 2048;
    virtualisation.qemu.options = [ "-vga none -device virtio-gpu-pci" ];
    users.users.alice = {
      isNormalUser = true;
      uid = 1000;
      group = "alice";
      password = "test-password";
    };
    users.groups.alice = {};
    programs.sway.enable = true;
    security.polkit.enable = true;
    services.rsdm = {
      enable = true;
      package = rsdm;
      keyring = "none";
      dm.fixedSession = "${compositor}";
      config.security.failure_delay_ms = 0;
    };
    environment.etc."sway/config".text = ''
      output * bg #ff00ff solid_color
      exec ${rsdm}/bin/rsdm session finalize SWAYSOCK
    '';
    specialisation.updated.configuration.environment.etc."rsdm-switch-probe".text = "updated";
  };

  testScript = ''
    import shlex

    start_all()
    machine.wait_for_unit("rsdm.service")
    machine.wait_until_succeeds("journalctl --no-pager -u rsdm | grep 'display manager initialized'")
    machine.wait_for_unit("multi-user.target")
    machine.sleep(2)
    machine.send_key("ctrl-alt-f1")
    machine.send_chars("alice")
    machine.send_key("tab")
    machine.send_chars("test-password")
    machine.send_key("ret")
    user = "runuser -u alice -- env XDG_RUNTIME_DIR=/run/user/1000 DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus "
    try:
        machine.wait_until_succeeds(user + "rsdm session status | grep ': running '", timeout=45)
    except Exception:
        print(machine.succeed("journalctl --no-pager -b -n 100"))
        raise
    environment = machine.succeed(user + "systemctl --user show-environment")
    values = dict(line.split("=", 1) for line in environment.splitlines())
    generation = values["RSDM_SESSION_GENERATION"]
    session_id = values["XDG_SESSION_ID"]
    session = user + "RSDM_SESSION_GENERATION=" + generation + " WAYLAND_DISPLAY=" + values["WAYLAND_DISPLAY"] + " SWAYSOCK=" + values["SWAYSOCK"] + " "
    machine.succeed(session + "swaymsg -t get_outputs --raw | grep '\"active\": true'")
    compositor = machine.succeed(user + "systemctl --user list-units --plain --no-legend 'session-rsdm-*' | awk '{print $1}'").strip()
    compositor_pid = machine.succeed(user + "systemctl --user show --property=MainPID --value " + shlex.quote(compositor)).strip()

    with subtest("user manager reload and reexec preserve the session and applications"):
        machine.succeed(session + "rsdm app -- ${application}")
        machine.wait_for_file("/run/user/1000/dm-app-ready")
        app_pid = machine.succeed("cat /run/user/1000/dm-app-ready").strip()
        dm_pid = machine.succeed("systemctl show rsdm --property=MainPID --value").strip()
        for operation in ["daemon-reload", "daemon-reexec", "daemon-reexec"]:
            machine.succeed(user + "systemctl --user " + operation)
            machine.sleep(2)
            machine.succeed(session + "rsdm session status | grep ': running '")
            assert machine.succeed(user + "systemctl --user show --property=MainPID --value " + shlex.quote(compositor)).strip() == compositor_pid
            assert machine.succeed("systemctl show rsdm --property=MainPID --value").strip() == dm_pid
            machine.fail("test -e /tmp/dm-app-saved")
            machine.succeed(session + "swaymsg -t get_outputs --raw | grep '\"active\": true'")
        machine.fail("journalctl --no-pager -b | grep 'session coordinator failed'")

    with subtest("NixOS switch preserves the live desktop and applications"):
        machine.succeed("/run/current-system/specialisation/updated/bin/switch-to-configuration switch")
        machine.succeed("test \"$(cat /etc/rsdm-switch-probe)\" = updated")
        machine.sleep(2)
        machine.succeed(session + "rsdm session status | grep ': running '")
        assert machine.succeed(user + "systemctl --user show --property=MainPID --value " + shlex.quote(compositor)).strip() == compositor_pid
        assert machine.succeed("systemctl show rsdm --property=MainPID --value").strip() == dm_pid
        machine.fail("test -e /tmp/dm-app-saved")
        machine.succeed(session + "swaymsg -t get_outputs --raw | grep '\"active\": true'")

    with subtest("user bus restarts preserve ownership, applications and session control"):
        for _ in range(2):
            broker_pid = machine.succeed(user + "systemctl --user show dbus.service --property=MainPID --value").strip()
            machine.execute(user + "systemctl --user restart dbus.service")
            machine.wait_until_succeeds(user + "systemctl --user is-active --quiet dbus.service")
            assert machine.succeed(user + "systemctl --user show dbus.service --property=MainPID --value").strip() != broker_pid
            machine.wait_until_succeeds(session + "rsdm session status | grep ': running '")
            assert "RSDM_SESSION_GENERATION=" + generation in machine.succeed(user + "systemctl --user show-environment")
            assert machine.succeed(user + "systemctl --user show --property=MainPID --value " + shlex.quote(compositor)).strip() == compositor_pid
            assert machine.succeed("systemctl show rsdm --property=MainPID --value").strip() == dm_pid
            machine.succeed("kill -0 " + app_pid)
            machine.fail("test -e /tmp/dm-app-saved")
            machine.succeed(session + "swaymsg -t get_outputs --raw | grep '\"active\": true'")
        machine.succeed("journalctl --no-pager -b | grep 'restored user manager connection'")
        machine.succeed("journalctl --no-pager -b | grep 'restored session control endpoint'")
        machine.fail(session + "rsdm session cleanup --generation " + generation)
        machine.succeed(session + "rsdm app -- true")

    with subtest("a service restart preserves the seated session and PAM owner"):
        machine.succeed("systemctl restart rsdm")
        machine.wait_for_unit("rsdm.service")
        machine.wait_until_succeeds("journalctl --no-pager -u rsdm | grep 'VT is owned by a live session; waiting for it to end'")
        machine.succeed("test \"$(systemctl show rsdm --property=NRestarts --value)\" = 0")
        machine.fail("journalctl --no-pager -u rsdm | grep 'another rsdm greeter already owns'")
        machine.succeed("loginctl show-session " + shlex.quote(session_id) + " --property=State")
        assert machine.succeed(user + "systemctl --user show --property=MainPID --value " + shlex.quote(compositor)).strip() == compositor_pid
        machine.succeed(session + "rsdm session status | grep ': running '")

    with subtest("logout saves applications before PAM closes and returns the Greeter"):
        machine.succeed(session + "rsdm session stop")
        machine.wait_for_file("/tmp/dm-app-saved")
        machine.wait_until_succeeds("journalctl --no-pager -b | grep 'session finished.*exit=Success'")
        machine.wait_until_succeeds("journalctl --no-pager -b | grep 'pam_unix(rsdm:session): session closed for user alice'")
        machine.wait_until_fails("loginctl show-session " + shlex.quote(session_id))
        machine.wait_until_succeeds("journalctl --no-pager -u rsdm | grep 'display manager initialized' | test $(wc -l) -ge 2")
        machine.wait_for_unit("rsdm.service")
        machine.fail("journalctl --no-pager -b | grep 'session recovery exited with'")
  '';
}
