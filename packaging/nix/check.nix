{
  self,
  nixpkgs,
  system,
}:
let
  pkgs = import nixpkgs { inherit system; };
  evaluate = settings:
    (nixpkgs.lib.nixosSystem {
      inherit system;
      modules = [
        self.nixosModules.default
        {
          boot.loader.grub.enable = false;
          fileSystems."/" = {
            device = "none";
            fsType = "tmpfs";
          };
          system.stateVersion = "26.05";
          services.rsdm = {
            enable = true;
            lock.enable = true;
            idle.enable = true;
          } // settings;
          nix.settings.extra-substituters = [ "https://cache.example.org" ];
          nix.settings.extra-trusted-public-keys = [ "example-key" ];
        }
      ];
    }).config;
  stable = evaluate { };
  stableWithOptions = evaluate {
    dm = {
      tty = "tty2";
      seat = "seat1";
      fixedSession = "niri-session";
      sessionDirs = [ "/usr/share/wayland-sessions" ];
      fallback.enable = false;
      design = {
        theme = "catppuccin";
        borderStyle = "minimal";
        background = "matrix";
        titleMode = "hostname";
        titleText = "Greeter";
      };
    };
    lock = {
      enable = true;
      primaryOutput = "DP-1";
      secondaryOutput = "off";
      size = 2;
      design = {
        theme = "nord";
        titleMode = "preset-logo";
        titlePreset = "nixos";
        wallpaper = "/etc/rsdm-wallpaper.png";
        wallpaperDim = 4;
        backgroundOpacity = 7;
      };
    };
    logging.file = "/var/log/rsdm/rsdm.log";
    extraConfig = {
      security.allowed_groups = [ "users" ];
      session_manager.extra_env = [ "XCURSOR_THEME" ];
    };
  };
  rsdmAssertionsHold = cfg:
    builtins.all (entry: entry.assertion) (builtins.filter (entry:
      nixpkgs.lib.hasPrefix "services.rsdm" entry.message
      || nixpkgs.lib.hasPrefix "RSDM" entry.message
    ) cfg.assertions);
  incompatibleStable = evaluate {
    dm.design.borderStyle = "none";
    lock.design.borderStyle = "none";
  };
  incompatibleExtraConfig = evaluate {
    extraConfig.lock.design.border_style = "none";
  };
  unstable = evaluate {
    channel = "unstable";
    dm.design.borderStyle = "none";
    lock.design.borderStyle = "none";
  };
  idleOnly = evaluate { dm.enable = false; };
  override = evaluate {
    channel = "unstable";
    package = pkgs.hello;
  };
in
assert stable.services.rsdm.channel == "stable";
assert stable.services.rsdm.package == self.packages.${system}.rsdm-stable;
assert rsdmAssertionsHold stable;
assert rsdmAssertionsHold stableWithOptions;
assert
  rsdmAssertionsHold incompatibleStable
  == nixpkgs.lib.versionAtLeast stable.services.rsdm.package.version "2.0.0";
assert
  rsdmAssertionsHold incompatibleExtraConfig
  == nixpkgs.lib.versionAtLeast stable.services.rsdm.package.version "2.0.0";
assert unstable.services.rsdm.package == self.packages.${system}.rsdm-unstable;
assert !unstable.services.rsdm.package.allowSubstitutes;
assert unstable.services.rsdm.dm.design.borderStyle == "none";
assert unstable.services.rsdm.lock.design.borderStyle == "none";
assert override.services.rsdm.package == pkgs.hello;
assert builtins.all (cfg:
  cfg.nix.settings.extra-substituters == [ "https://cache.example.org" ]
  && cfg.nix.settings.extra-trusted-public-keys == [ "example-key" ]
) [ stable unstable idleOnly ];
assert stable.systemd.services ? rsdm;
assert stable.systemd.defaultUnit == "graphical.target";
assert !(idleOnly.systemd.services ? rsdm);
assert idleOnly.systemd.defaultUnit == "multi-user.target";
assert idleOnly.systemd.services."getty@tty1".enable;
assert idleOnly.systemd.user.services ? rsdm-idle;
pkgs.runCommand "rsdm-module-configuration" { } ''
  ${stable.services.rsdm.package}/bin/rsdm \
    --config ${stable.environment.etc."rsdm.toml".source} validate-config
  ${stableWithOptions.services.rsdm.package}/bin/rsdm \
    --config ${stableWithOptions.environment.etc."rsdm.toml".source} validate-config
  touch "$out"
''
