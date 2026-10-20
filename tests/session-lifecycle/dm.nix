{ pkgs, rsdm }:

let
  controller = import ./controller.nix { inherit pkgs; };
  module = import ../../packaging/nix/module.nix {
    packages.${pkgs.stdenv.hostPlatform.system}.rsdm-stable = rsdm;
  };
  compositor = pkgs.writeShellScript "test-dm-sway" ''
    set -eu
    ${pkgs.systemd}/bin/busctl --system --json=short call org.freedesktop.login1 /org/freedesktop/login1 org.freedesktop.login1.Manager ListInhibitors |
      ${pkgs.python3}/bin/python3 -c 'import json, sys; rows = json.load(sys.stdin)["data"][0]; assert any(row[:4] == ["shutdown", "RSDM", "Closing the graphical PAM session", "delay"] and row[4] == 0 for row in rows)'
    touch /tmp/dm-root-guard-ready
    export WLR_RENDERER=pixman
    exec ${pkgs.sway}/bin/sway --config /etc/sway/config
  '';
  application = pkgs.writeShellScript "test-dm-application" ''
    set -eu
    trap 'test -S "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY"; touch /tmp/dm-app-saved; exit 0' TERM
    echo "$$" > "$XDG_RUNTIME_DIR/dm-app-ready"
    while :; do sleep 1; done
  '';
  fallbackGetty = pkgs.runCommand "test-fallback-getty" { nativeBuildInputs = [ pkgs.stdenv.cc ]; } ''
    $CC -x c -o $out - <<'EOF'
    #include <stdio.h>
    int main(int argc, char **argv) {
      FILE *file = fopen("/tmp/rsdm-fallback-arguments", "w");
      if (!file) return 1;
      for (int i = 0; i < argc; i++) if (fprintf(file, "%s\n", argv[i]) < 0) return 1;
      return fclose(file) != 0;
    }
    EOF
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
    systemd.settings.Manager.ShowStatus = pkgs.lib.mkForce true;
    services.rsdm = {
      enable = true;
      package = rsdm;
      keyring = "none";
      dm.fixedSession = "${compositor}";
      config.security.failure_delay_ms = 0;
      config.dm.session_dirs = [ "/etc/rsdm-test-sessions" ];
    };
    systemd.services.rsdm.preStart = ''
      ${pkgs.coreutils}/bin/mkdir -p /run/rsdm_noexec
      if ! ${pkgs.util-linux}/bin/mountpoint -q /run/rsdm_noexec; then
        ${pkgs.util-linux}/bin/mount -t tmpfs -o noexec,mode=0755 tmpfs /run/rsdm_noexec
      fi
      ${pkgs.coreutils}/bin/install -m755 ${pkgs.coreutils}/bin/true /run/rsdm_noexec/true
    '';
    environment.etc."rsdm-test-sessions/available.desktop".text = ''
      [Desktop Entry]
      Name=Test desktop
      Exec=${compositor}
      TryExec=${pkgs.coreutils}/bin/true
    '';
    environment.etc."rsdm-test-sessions/unavailable.desktop".text = ''
      [Desktop Entry]
      Name=Unavailable desktop
      Exec=/run/rsdm_noexec/true
      TryExec=/run/rsdm_noexec/true
    '';
    environment.etc."rsdm-test-sessions/unavailable-spaced.desktop".text = ''
      [Desktop Entry]
      Type=Application
      Name=Unavailable spaced filename
      Exec=${pkgs.coreutils}/bin/true
      TryExec=${pkgs.coreutils}/bin/true\s
    '';
    environment.etc."sway/config".text = ''
      output * bg #ff00ff solid_color
      exec ${rsdm}/bin/rsdm session finalize SWAYSOCK
    '';
    systemd.services."getty@tty63" = {
      overrideStrategy = "asDropin";
      serviceConfig = {
        Type = "oneshot";
        Restart = "no";
        RemainAfterExit = true;
        ExecStart = [ "" "@${fallbackGetty} rsdm-custom-getty \"quoted value\" \"\" %%I %%%% %I" ];
      };
    };
    environment.etc."rsdm-fallback.toml".text = ''
      [session_manager]
      enabled = false
      [dm]
      session_dirs = ["/run/rsdm-empty-sessions"]
      [dm.tty]
      path = "/dev/tty63"
      [dm.fallback]
      enabled = true
      command = []
      [security]
      deny_root = false
    '';
    specialisation.updated.configuration.environment.etc."rsdm-switch-probe".text = "updated";
  };

  testScript = ''
    import shlex

    start_all()
    machine.wait_for_unit("rsdm.service")
    machine.wait_until_succeeds("journalctl --no-pager -u rsdm | grep 'display manager initialized'")
    machine.wait_for_unit("multi-user.target")
    with subtest("Greeter excludes unavailable TryExec programs"):
        machine.succeed("${pkgs.coreutils}/bin/true")
        machine.fail("test -e '${pkgs.coreutils}/bin/true '")
        status, diagnostic = machine.execute("/run/rsdm_noexec/true")
        assert status == 126, diagnostic
        machine.succeed("journalctl --no-pager -u rsdm | grep 'desktop session discovery complete count=1'")
    machine.sleep(2)
    with subtest("Greeter suppresses PID 1 status output while preserving journal logs"):
        machine.succeed("test $(systemctl show -p ShowStatus --value) = no")
        machine.succeed("systemd-run --unit=rsdm-journal-probe --wait --collect -- /bin/sh -c 'echo rsdm-journal-probe-message'")
        machine.succeed("journalctl --no-pager -u rsdm-journal-probe | grep rsdm-journal-probe-message")
    machine.send_key("ctrl-alt-f1")
    machine.send_chars("alice")
    machine.send_key("tab")
    machine.send_chars("test-password")
    machine.send_key("ret")
    user = "runuser -u alice -- env XDG_RUNTIME_DIR=/run/user/1000 DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus "
    control = "${controller} published alice "
    try:
        machine.wait_until_succeeds(control + "rsdm session status | grep ': running '", timeout=45)
    except Exception:
        print(machine.succeed("journalctl --no-pager -b -n 100"))
        raise
    machine.wait_for_file("/tmp/dm-root-guard-ready")
    machine.succeed("test $(systemctl show -p ShowStatus --value) = yes")
    environment = machine.succeed(user + "systemctl --user show-environment")
    values = dict(line.split("=", 1) for line in environment.splitlines())
    generation = values["RSDM_SESSION_GENERATION"]
    session_id = values["XDG_SESSION_ID"]
    assert generation in machine.succeed(control + "busctl --user call org.rsdm.Session1 /org/rsdm/Session1 org.rsdm.Session1 Handshake")
    session = control + "RSDM_SESSION_GENERATION=" + generation + " WAYLAND_DISPLAY=" + values["WAYLAND_DISPLAY"] + " SWAYSOCK=" + values["SWAYSOCK"] + " "
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
        machine.wait_until_succeeds("busctl --system --json=short call org.freedesktop.login1 /org/freedesktop/login1 org.freedesktop.login1.Manager ListInhibitors | ${pkgs.python3}/bin/python3 -c 'import json, sys; assert all(row[1] != \"RSDM\" for row in json.load(sys.stdin)[\"data\"][0])'")
        machine.wait_until_succeeds("journalctl --no-pager -u rsdm | grep 'display manager initialized' | test $(wc -l) -ge 2")
        machine.wait_for_unit("rsdm.service")
        machine.fail("journalctl --no-pager -b | grep 'session recovery exited with'")

    with subtest("TTY fallback preserves the arguments executed by systemd"):
        machine.succeed("mkdir /run/rsdm-empty-sessions")
        machine.succeed("systemctl start getty@tty63.service")
        expected = machine.succeed("cat /tmp/rsdm-fallback-arguments")
        assert expected == "rsdm-custom-getty\nquoted value\n\n%I\n%%\ntty63\n"
        machine.succeed("rm /tmp/rsdm-fallback-arguments")
        machine.succeed("systemd-run --unit=rsdm-test-fallback --collect -- ${rsdm}/bin/rsdm --config /etc/rsdm-fallback.toml dm")
        machine.wait_for_file("/tmp/rsdm-fallback-arguments")
        machine.wait_until_fails("systemctl is-active --quiet rsdm-test-fallback.service")
        assert machine.succeed("cat /tmp/rsdm-fallback-arguments") == expected

    with subtest("closing Greeter restores a configured disabled status policy"):
        machine.succeed("mkdir -p /run/systemd/system.conf.d")
        machine.succeed("printf '[Manager]\\nShowStatus=no\\n' > /run/systemd/system.conf.d/rsdm-test.conf")
        machine.succeed("systemctl daemon-reexec")
        machine.succeed("systemctl stop rsdm.service")
        machine.succeed("test $(systemctl show -p ShowStatus --value) = no")
  '';
}
