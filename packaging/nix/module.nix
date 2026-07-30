self:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.services.rsdm;
  toml = pkgs.formats.toml { };
  defaultPackage = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
  ttyName = lib.removePrefix "/dev/" cfg.dm.tty;
  ttyPath = if lib.hasPrefix "/dev/" cfg.dm.tty then cfg.dm.tty else "/dev/${cfg.dm.tty}";
  displayManagerAlias =
    if cfg.displayManagerAlias == null then ttyName == "tty1" else cfg.displayManagerAlias;

  # Decide which keyring rsdm's greeter PAM stack should drive, without any
  # rsdm-specific or desktop-specific assumptions. "auto" mirrors the whole
  # system: if the keyring is unlocked for ANY login path on this machine, rsdm
  # unlocks it too. We therefore scan every PAM service for the keyring toggle
  # (not just `login` - the user may drive it from greetd, sddm, a custom
  # service, ...) and also honour the standalone gnome-keyring service. Explicit
  # "gnome"/"kwallet"/"none" force the choice. A fully custom or non-standard
  # keyring is handled by overriding security.pam.services.rsdm directly. We
  # scan only the bool toggles, and we exclude rsdm's own services so computing
  # rsdm's own toggle below cannot recurse back into this scan.
  scannablePamServices = lib.attrValues (
    removeAttrs config.security.pam.services [
      "rsdm"
      "rsdm-lock"
    ]
  );
  anyServiceEnablesGnomeKeyring = lib.any (
    service: service.enableGnomeKeyring or false
  ) scannablePamServices;
  anyServiceEnablesKwallet = lib.any (service: service.kwallet.enable or false) scannablePamServices;
  gnomeKeyringService = config.services.gnome.gnome-keyring.enable or false;
  useGnomeKeyring =
    if cfg.keyring == "gnome" then
      true
    else if cfg.keyring == "auto" then
      (anyServiceEnablesGnomeKeyring || gnomeKeyringService)
    else
      false;
  useKwallet =
    if cfg.keyring == "kwallet" then
      true
    else if cfg.keyring == "auto" then
      anyServiceEnablesKwallet
    else
      false;
  design = import ./design.nix { inherit lib; };
  inherit (design) designToToml mkDesignOptions;
  baseConfig = {
    session_manager.enabled = cfg.sessionManager;
    paths.cache_dir = "/var/cache/rsdm";
    logging = {
      level = cfg.logging.level;
    }
    // lib.optionalAttrs (cfg.logging.file != null) { file = cfg.logging.file; };
    dm = {
      enable = cfg.dm.enable;
      pam_service = "rsdm";
      tty = {
        path = ttyPath;
        seat = cfg.dm.seat;
      };
      fallback = {
        enabled = cfg.dm.fallback.enable;
        command = cfg.dm.fallback.command;
      };
      remember = {
        username = cfg.dm.remember.username;
        session = cfg.dm.remember.session;
      };
      design = designToToml cfg.dm.design;
    }
    // lib.optionalAttrs (cfg.dm.fixedSession != null) { fixed_session = cfg.dm.fixedSession; }
    // lib.optionalAttrs (cfg.dm.sessionDirs != null) { session_dirs = cfg.dm.sessionDirs; };
    lock = {
      enable = cfg.lock.enable;
      pam_service = "rsdm-lock";
      secondary_output = cfg.lock.secondaryOutput;
      design = designToToml cfg.lock.design;
    }
    // lib.optionalAttrs (cfg.lock.size != null) { size = cfg.lock.size; }
    // lib.optionalAttrs (cfg.lock.primaryOutput != null) {
      primary_output = cfg.lock.primaryOutput;
    };
    idle = {
      enable = cfg.idle.enable;
      timeout = cfg.idle.timeout;
      ignore_inhibitors = cfg.idle.ignoreInhibitors;
      lock_command = cfg.idle.lockCommand;
      on_lock = cfg.idle.onLock;
      on_unlock = cfg.idle.onUnlock;
    };
  };
in
{
  options.services.rsdm = import ./options.nix {
    inherit
      defaultPackage
      lib
      mkDesignOptions
      pkgs
      toml
      ttyName
      ;
  };

  config = import ./system.nix {
    inherit
      baseConfig
      cfg
      config
      displayManagerAlias
      lib
      pkgs
      toml
      ttyName
      ttyPath
      useGnomeKeyring
      useKwallet
      ;
  };
}
