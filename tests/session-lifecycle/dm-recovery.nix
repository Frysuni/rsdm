{ pkgs, rsdm }:

let
  module = import ../../packaging/nix/module.nix {
    packages.${pkgs.stdenv.hostPlatform.system}.rsdm-stable = rsdm;
  };
  controller = import ./controller.nix { inherit pkgs; };
  compositor = pkgs.writeShellScript "custom-config-desktop" ''
    export WLR_RENDERER=pixman
    exec ${pkgs.sway}/bin/sway --config /etc/sway/custom-config
  '';
  application = pkgs.writeShellScript "custom-config-app" ''
    trap 'touch /tmp/custom-app-saved; exit 0' TERM
    touch /tmp/custom-app-ready
    while :; do sleep 1; done
  '';
in
pkgs.testers.runNixOSTest {
  name = "rsdm-custom-config-recovery";
  globalTimeout = 120;
  nodes.machine = { lib, ... }: {
    imports = [ module ];
    virtualisation.memorySize = 2048;
    virtualisation.qemu.options = [ "-vga none -device virtio-gpu-pci" ];
    users.users.alice = {
      isNormalUser = true;
      uid = 1000;
      group = "alice";
      extraGroups = [ "session-policy" ];
      password = "test-password";
    };
    users.groups.alice = {};
    users.groups.session-policy.gid = 1234;
    programs.sway.enable = true;
    security.polkit.enable = true;
    services.rsdm = { enable = true; package = rsdm; keyring = "none"; };
    systemd.services.rsdm.serviceConfig = {
      ExecStartPre = [ "${pkgs.coreutils}/bin/install -Dm755 ${rsdm}/bin/rsdm /run/test-dm/bin/rsdm" ];
      ExecStart = lib.mkForce "/run/test-dm/bin/rsdm --config /etc/rsdm-custom.toml dm";
    };
    environment.etc."rsdm.toml".source = lib.mkForce (pkgs.writeText "default-disabled.toml" ''
      [session_manager]
      enabled = false
    '');
    environment.etc."rsdm-custom.toml".text = ''
      [dm]
      fixed_session = "${compositor}"
      [dm.remember]
      username = false
      [security]
      failure_delay_ms = 0
    '';
    environment.etc."rsdm-explicit.toml".text = ''
      [dm]
      fixed_session = "${rsdm}/bin/rsdm --config /etc/rsdm-session.toml session start -- ${compositor}"
      [dm.remember]
      username = false
      [session_manager]
      enabled = false
      [security]
      failure_delay_ms = 0
    '';
    environment.etc."rsdm-session.toml" = {
      text = "";
      mode = "0640";
      group = "session-policy";
    };
    environment.etc."sway/custom-config".text = ''
      output * bg #ff00ff solid_color
      exec ${rsdm}/bin/rsdm --config /etc/rsdm-session.toml session finalize SWAYSOCK
    '';
  };

  testScript = ''
    start_all()
    machine.wait_for_unit("rsdm.service")
    machine.wait_until_succeeds("journalctl --no-pager -u rsdm | grep 'display manager initialized'")
    machine.succeed("loginctl enable-linger alice")
    machine.succeed("cp /etc/rsdm-custom.toml /run/rsdm-wrapped.toml")
    user = "runuser -u alice -- env XDG_RUNTIME_DIR=/run/user/1000 DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus "
    client = "${controller} published alice ${rsdm}/bin/rsdm --config /etc/rsdm-session.toml "
    for count, scenario in enumerate(["wrapped desktop", "explicit session start", "replaced executable"], 1):
        with subtest(scenario):
            if count > 1:
                source = "/etc/rsdm-explicit.toml" if scenario == "explicit session start" else "/run/rsdm-wrapped.toml"
                machine.succeed("systemctl stop rsdm; cp --remove-destination " + source + " /etc/rsdm-custom.toml; systemctl start rsdm")
                machine.wait_until_succeeds("journalctl --no-pager -u rsdm | grep 'display manager initialized' | test $(wc -l) -ge " + str(count))
            machine.succeed("rm -f /tmp/custom-app-ready /tmp/custom-app-saved")
            machine.send_key("ctrl-alt-f1")
            machine.send_chars("alice")
            machine.send_key("tab")
            machine.send_chars("test-password")
            machine.send_key("ret")
            machine.wait_until_succeeds(client + "session status | grep ': running '", timeout=40)
            machine.succeed(client + "app --shutdown-timeout 2 --shutdown-method term -- ${application}")
            machine.wait_for_file("/tmp/custom-app-ready")
            compositor = machine.succeed(user + "systemctl --user list-units --plain --no-legend 'session-rsdm-*' | awk '{print $1}'").strip()
            pid = machine.succeed(user + "systemctl --user show --property=MainPID --value " + compositor).strip()
            coordinator = machine.succeed("pgrep -u alice -f 'rsdm --config /etc/rsdm-(custom|session).toml session start'").strip()
            assert coordinator.isdigit(), coordinator
            if scenario == "replaced executable":
                machine.succeed("install -m755 ${rsdm}/bin/rsdm /run/test-dm/bin/rsdm.next; mv -f /run/test-dm/bin/rsdm.next /run/test-dm/bin/rsdm")
                machine.succeed("readlink /proc/$(systemctl show rsdm -p MainPID --value)/exe | grep -F '(deleted)'")
            machine.succeed("kill -KILL " + coordinator)
            machine.wait_until_succeeds("journalctl --no-pager -b | grep 'pam_unix(rsdm:session): session closed for user alice' | test $(wc -l) -ge " + str(count))
            machine.fail("kill -0 " + pid)
            machine.succeed("test -e /tmp/custom-app-saved")
            machine.wait_until_tty_matches("1", "Password", timeout=15)
            machine.fail("journalctl --no-pager -b | grep -E 'session recovery (failed|exited with)'")
  '';
}
