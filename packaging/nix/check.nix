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
assert builtins.all (entry: entry.assertion) stable.assertions;
assert
  builtins.all (entry: entry.assertion) incompatibleStable.assertions
  == nixpkgs.lib.versionAtLeast stable.services.rsdm.package.version "2.0.0";
assert
  builtins.all (entry: entry.assertion) incompatibleExtraConfig.assertions
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
pkgs.runCommand "rsdm-module-evaluation" { } "touch $out"
