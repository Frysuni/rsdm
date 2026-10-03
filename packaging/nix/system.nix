{
  baseConfig,
  cfg,
  config,
  displayManagerAlias,
  lib,
  pkgs,
  toml,
  ttyName,
  ttyPath,
  useGnomeKeyring,
  useKwallet,
}:
lib.mkIf cfg.enable {
  assertions = [
    {
      assertion = !cfg.idle.enable || cfg.lock.enable || cfg.idle.lockCommand != [ ];
      message = "services.rsdm.idle.enable with the built-in locker requires services.rsdm.lock.enable";
    }
  ];

  # Mask BOTH the static getty and logind's autovt alias for the greeter's VT.
  # getty@ttyN is the unit our Conflicts= evicts; masking it (enable=false ->
  # /dev/null symlink) keeps the boot-time getty off the VT. But autovt@ttyN is
  # the SAME getty@.service template under a different instance name, started
  # on demand by logind whenever that VT is activated and has no session it
  # recognises - e.g. right after a `nixos-rebuild switch` reactivates
  # getty.target. Our Conflicts= names getty@ttyN, not autovt@ttyN, so it does
  # NOT evict the autovt one: logind would spawn a getty directly onto the live
  # compositor's VT, whose console TTY setup forces an out-of-band modeset that
  # corrupts amdgpu's present path (desktop drops to ~20-30fps until a reboot).
  # Disabling autovt is not enough - logind StartUnit's it by name regardless
  # of WantedBy symlinks - so it must be masked to be refused.
  systemd.services."getty@${ttyName}".enable = lib.mkIf (cfg.dm.enable && cfg.disableGetty) false;
  systemd.services."autovt@${ttyName}".enable = lib.mkIf (cfg.dm.enable && cfg.disableGetty) false;

  # rsdm is the display manager, so it owns the boot target. Without this the
  # system keeps systemd's default of multi-user.target, graphical.target is
  # never reached, and rsdm.service (WantedBy=graphical.target) never starts at
  # boot - the user lands on a bare console and has to launch the session by
  # hand. Every NixOS DM module promotes the default unit the same way. Use
  # mkDefault so a host that drives the boot target itself can override it.
  systemd.defaultUnit = lib.mkIf cfg.dm.enable (lib.mkDefault "graphical.target");

  environment.etc."rsdm.toml".source = toml.generate "rsdm.toml" (
    lib.recursiveUpdate (lib.recursiveUpdate baseConfig cfg.config) cfg.extraConfig
  );
  environment.systemPackages = [ cfg.package ];

  services.logrotate.settings.rsdm = lib.mkIf (cfg.logging.file != null) {
    files = [ cfg.logging.file ];
    frequency = "weekly";
    rotate = 8;
    compress = true;
    missingok = true;
    notifempty = true;
    # copytruncate keeps the original file (rsdm holds it open); `create` is
    # mutually exclusive with it and would only make logrotate warn.
    copytruncate = true;
  };

  # Greeter PAM. It is the login: it authenticates the user, opens a logind
  # session (startSession => pam_systemd binds the seat/VT so the compositor
  # can take DRM master), and - in the very same open_session - runs the
  # keyring module so the keyring is unlocked with the password the user just
  # typed. This is the standard login/gdm/sddm shape; the keyring module itself
  # drops to the user and starts its agent, and the agent's environment rides
  # into the launched session via pam_getenvlist. Driving the keyring through
  # the structured enableGnomeKeyring/kwallet.enable toggles is what orders the
  # keyring `auth` module BEFORE the `sufficient pam_unix` short-circuit; a raw
  # `.text` append would land it after, where it never runs. With both toggles
  # off (keyring = "none") this is a plain login stack with no keyring.
  security.pam.services.rsdm = {
    startSession = true;
    enableGnomeKeyring = useGnomeKeyring;
    kwallet.enable = useKwallet;
  };

  # The locker only verifies the seated user: auth/account, no session. Uses
  # mkDefault so a system with an unusual auth setup can override the stack.
  security.pam.services.rsdm-lock.text = lib.mkDefault ''
    auth      include   login
    account   include   login
  '';

  systemd.services.rsdm = lib.mkIf cfg.dm.enable {
    description = "rsdm display manager";
    conflicts = [ "getty@${ttyName}.service" ];
    after = [
      "systemd-user-sessions.service"
      "getty@${ttyName}.service"
    ];
    wantedBy = [ "graphical.target" ];
    aliases = lib.optional displayManagerAlias "display-manager.service";
    # Do NOT restart rsdm on `nixos-rebuild switch`. rsdm.service owns the VT
    # and (with the session manager) is the ancestor of the running compositor,
    # so a switch-triggered restart re-execs the greeter and disturbs the live
    # graphical session - the user sees the desktop hitch / drop frames on every
    # rebuild, even when nothing about rsdm changed (the unit's ExecStart embeds
    # cfg.package's store path, so an unrelated rebuild still counts it as
    # "changed"). Every NixOS display manager sets this for the same reason: a
    # new rsdm is picked up on the next logout or reboot, never by yanking the
    # session out from under you. mkDefault leaves it overridable.
    restartIfChanged = lib.mkDefault false;
    # util-linux puts `agetty` on PATH for the fallback handover; `systemd`
    # puts `systemctl` there so the fallback can discover the VT's own getty.
    # systemd supplies the rest of the greeter's runtime PATH.
    path = [
      pkgs.util-linux
      pkgs.systemd
    ];
    serviceConfig = {
      ExecStart = "${cfg.package}/bin/rsdm dm --config /etc/rsdm.toml";
      Restart = "always";
      RestartSec = 1;
      TTYPath = ttyPath;
      StandardInput = "tty";
      StandardOutput = "journal";
      StandardError = "journal";
      # Deliberately NOT setting TTYReset / TTYVHangup / TTYVTDisallocate.
      # systemd applies those VT-teardown ops both BEFORE and AFTER the unit
      # runs (see systemd.exec(5): "before and after the service is started").
      # rsdm hands the VT off to the user's compositor, which keeps running on
      # the SAME VT in its own logind session after the greeter is gone. When
      # the unit later stops - and a `nixos-rebuild switch` stops it once, via
      # a dependency of the first-after-boot sysinit reactivation, even with
      # restartIfChanged=false - the "after" teardown would vhangup/disallocate
      # the live compositor's VT, forcing an out-of-band console modeset that
      # corrupts amdgpu's present path: the desktop drops to ~20-30fps until a
      # fresh modeset (reboot). The greeter does not need them: it claims the
      # VT itself (open + flock single-instance + tcsetpgrp foreground) and
      # Conflicts= already evicts the getty, so there is no leftover text state
      # to reset. Stopping rsdm must be inert to whoever now owns the VT.
      CacheDirectory = "rsdm";
      CacheDirectoryMode = "0700";
      LogsDirectory = "rsdm";
      LogsDirectoryMode = "0750";
    };
  };

  systemd.user.services.rsdm-idle = lib.mkIf cfg.idle.enable {
    description = "rsdm Wayland idle monitor";
    documentation = [ "https://github.com/Frysuni/rsdm/blob/main/docs/idle.md" ];
    wantedBy = [ "graphical-session.target" ];
    partOf = [ "graphical-session.target" ];
    after = [ "graphical-session-pre.target" ];
    environment.RSDM_SYSTEMD_RUN = "${pkgs.systemd}/bin/systemd-run";
    serviceConfig = {
      ExecStart = "${cfg.package}/bin/rsdm idle --config /etc/rsdm.toml";
      Restart = "on-failure";
      RestartSec = 2;
    };
  };
}
