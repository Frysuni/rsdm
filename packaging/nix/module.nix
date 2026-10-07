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
  defaultPackage = self.packages.${pkgs.stdenv.hostPlatform.system}."rsdm-${cfg.channel}";
  ttyName = lib.removePrefix "/dev/" cfg.dm.tty;
  ttyPath = if lib.hasPrefix "/dev/" cfg.dm.tty then cfg.dm.tty else "/dev/${cfg.dm.tty}";

  # Auto selects one PAM keyring: prefer detected GNOME Keyring, otherwise
  # use detected KWallet. Explicit choices override detection. Exclude our own
  # PAM services so deriving their toggles cannot recurse into this scan.
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
      (!useGnomeKeyring && anyServiceEnablesKwallet)
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
      lib
      pkgs
      toml
      useGnomeKeyring
      useKwallet
      ;
  };
}
