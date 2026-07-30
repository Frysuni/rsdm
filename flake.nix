{
  description = "rsdm standalone Rust TTY/TUI display manager and screen locker";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    crane.url = "github:ipetkov/crane";
  };

  outputs =
    {
      self,
      nixpkgs,
      crane,
    }:
    let
      version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
      pkgsFor = system: import nixpkgs { inherit system; };

      # Built from the flake source on the user's machine (tracks whatever this
      # flake input points at). This is the `default`.
      mkUnstable =
        system:
        let
          pkgs = pkgsFor system;
          craneLib = crane.mkLib pkgs;
          commonArgs = {
            pname = "rsdm";
            inherit version;
            src = craneLib.cleanCargoSource ./.;
            strictDeps = true;
            nativeBuildInputs = [ pkgs.pkg-config ];
            buildInputs = [
              pkgs.pam
              pkgs.wayland
              pkgs.libxkbcommon
            ];
            cargoExtraArgs = "-p rsdm";
            doCheck = false;
          };
        in
        craneLib.buildPackage (commonArgs // { cargoArtifacts = craneLib.buildDepsOnly commonArgs; });

      # Evaluate the module in both supported deployment shapes. In particular,
      # lock/idle-only installs must not create a restart-looping greeter service
      # or take over getty/defaultUnit.
      mkModuleCheck =
        system:
        let
          pkgs = pkgsFor system;
          evaluate =
            dmEnabled:
            (nixpkgs.lib.nixosSystem {
              inherit system;
              modules = [
                (import ./packaging/nix/module.nix self)
                {
                  boot.loader.grub.enable = false;
                  fileSystems."/" = {
                    device = "none";
                    fsType = "tmpfs";
                  };
                  system.stateVersion = "26.05";
                  services.rsdm = {
                    enable = true;
                    dm.enable = dmEnabled;
                    lock.enable = true;
                    idle.enable = true;
                  };
                }
              ];
            }).config;
          full = evaluate true;
          idleOnly = evaluate false;
        in
        assert full.systemd.services ? rsdm;
        assert full.systemd.defaultUnit == "graphical.target";
        assert !(idleOnly.systemd.services ? rsdm);
        assert idleOnly.systemd.defaultUnit == "multi-user.target";
        assert idleOnly.systemd.services."getty@tty1".enable;
        assert idleOnly.systemd.user.services ? rsdm-idle;
        pkgs.runCommand "rsdm-module-evaluation" { } "touch $out";

    in
    {
      packages = forAllSystems (system: rec {
        rsdm-unstable = mkUnstable system;
        # Keep the public name usable before the first release assets exist.
        # Pinning the flake input to a tag/revision provides stable source; a
        # fake-output-hash prebuilt derivation would make flake checks pass but
        # fail every real user build.
        rsdm-stable = rsdm-unstable;
        default = rsdm-unstable;
      });

      apps = forAllSystems (system: rec {
        rsdm = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/rsdm";
          meta.description = "Run the rsdm display manager and screen-locker CLI";
        };
        default = rsdm;
      });

      checks = forAllSystems (system: {
        module = mkModuleCheck system;
      });

      devShells = forAllSystems (
        system:
        let
          pkgs = pkgsFor system;
        in
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              cargo
              rustc
              rustfmt
              clippy
              pkg-config
              pam
              wayland
              libxkbcommon
              gcc
              clang
            ];
            shellHook = ''
              export PKG_CONFIG_PATH="${pkgs.pam}/lib/pkgconfig:${pkgs.wayland.dev}/lib/pkgconfig:${pkgs.libxkbcommon.dev}/lib/pkgconfig''${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
              export LD_LIBRARY_PATH="${pkgs.wayland}/lib:${pkgs.libxkbcommon}/lib''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
            '';
          };
        }
      );

      overlays.default = final: _prev: {
        rsdm = self.packages.${final.stdenv.hostPlatform.system}.default;
      };

      nixosModules.default = import ./packaging/nix/module.nix self;
      nixosModules.rsdm = self.nixosModules.default;
    };
}
