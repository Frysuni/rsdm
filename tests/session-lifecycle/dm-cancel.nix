{ pkgs, rsdm }:

let
  module = import ../../packaging/nix/module.nix {
    packages.${pkgs.stdenv.hostPlatform.system}.rsdm-stable = rsdm;
  };
  session = pkgs.writeShellScript "cancelled-desktop" ''
    ${pkgs.coreutils}/bin/touch /tmp/desktop-started
    exec ${pkgs.coreutils}/bin/sleep 60
  '';
  pamHook = pkgs.writeShellScript "cancelled-login-pam" ''
    case "$PAM_TYPE" in
      open_session)
        ${pkgs.systemd}/bin/systemctl kill --kill-whom=main --signal=STOP rsdm.service
        printf "%s" "$PPID" > /tmp/pam-owner
        ${pkgs.coreutils}/bin/touch /tmp/pam-opened
        ;;
      close_session)
        owner=$PPID
        ${pkgs.coreutils}/bin/sleep 0.1
        # An orphaned hook must not mark a killed PAM owner as cleanly closed.
        ${pkgs.gnugrep}/bin/grep -Eq "^State:[[:space:]]+[SRD]" /proc/"$owner"/status
        ${pkgs.coreutils}/bin/touch /tmp/pam-closed
        ;;
    esac
  '';
in
pkgs.testers.runNixOSTest {
  name = "rsdm-dm-cancelled-login";
  globalTimeout = 90;
  nodes.machine = { lib, ... }: {
    imports = [ module ];
    virtualisation.memorySize = 2048;
    virtualisation.qemu.options = [ "-vga none -device virtio-gpu-pci" ];
    users.users.alice = { isNormalUser = true; uid = 1000; password = "test-password"; };
    services.rsdm = { enable = true; package = rsdm; keyring = "none"; };
    security.pam.services.rsdm.text = lib.mkForce ''
      auth required ${pkgs.linux-pam}/lib/security/pam_unix.so
      account required ${pkgs.linux-pam}/lib/security/pam_unix.so
      session required ${pkgs.linux-pam}/lib/security/pam_unix.so
      session optional ${pkgs.systemd}/lib/security/pam_systemd.so
      session required ${pkgs.linux-pam}/lib/security/pam_exec.so ${pamHook}
    '';
    environment.etc."rsdm.toml".source = lib.mkForce (pkgs.writeText "cancelled-login.toml" ''
      [dm]
      fixed_session = "${session}"
      [dm.remember]
      username = false
      [session_manager]
      enabled = false
      [security]
      failure_delay_ms = 0
    '');
  };
  testScript = ''
    start_all()
    machine.wait_for_unit("rsdm.service")
    machine.wait_for_unit("multi-user.target")
    machine.wait_until_succeeds("journalctl --no-pager -u rsdm | grep 'display manager initialized'")
    machine.wait_until_tty_matches("1", "Password")
    machine.send_key("ctrl-alt-f1")
    machine.send_chars("alice")
    machine.send_key("tab")
    machine.send_chars("test-password")
    machine.send_key("ret")
    machine.wait_for_file("/tmp/pam-opened")
    machine.succeed("grep -E '^State:.*T' /proc/$(systemctl show rsdm -p MainPID --value)/status")
    machine.succeed("systemctl stop rsdm.service")
    machine.succeed("test -e /tmp/pam-closed")
    machine.fail("test -e /tmp/desktop-started")
    machine.fail("kill -0 $(cat /tmp/pam-owner)")
  '';
}
