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
      stableVersion = "1.0.0";
      stableReleases = {
        x86_64-linux = {
          target = "x86_64-unknown-linux-gnu";
          hash = "sha256-TiXIoDo/Fd2otUOdXECoUYsKt2bQLD067FwJMiV8ie0=";
        };
        aarch64-linux = {
          target = "aarch64-unknown-linux-gnu";
          hash = "sha256-azXEYmavmyPxzsVIgjjIP10kmdrrRTWY0IJKdwsruGQ=";
        };
      };

      # Built from the exact flake source on the user's machine.
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

      # Installs the matching binary from the latest stable GitHub release.
      # autoPatchelf rewrites the generic GNU/Linux interpreter and library
      # references to their Nix store paths; no Rust compilation is involved.
      mkStable =
        system:
        let
          pkgs = pkgsFor system;
          release = stableReleases.${system};
        in
        pkgs.stdenvNoCC.mkDerivation {
          pname = "rsdm";
          version = stableVersion;
          src = pkgs.fetchurl {
            url = "https://github.com/Frysuni/rsdm/releases/download/v${stableVersion}/rsdm-${stableVersion}-${release.target}.tar.gz";
            inherit (release) hash;
          };
          sourceRoot = ".";
          strictDeps = true;
          nativeBuildInputs = [ pkgs.autoPatchelfHook ];
          buildInputs = [
            pkgs.libxkbcommon
            pkgs.pam
            pkgs.stdenv.cc.cc.lib
            pkgs.wayland
          ];
          dontConfigure = true;
          dontBuild = true;
          installPhase = ''
            runHook preInstall

            install -Dm755 rsdm "$out/bin/rsdm"
            install -Dm644 LICENSE README.md rsdm.toml -t "$out/share/doc/rsdm"
            cp -r docs "$out/share/doc/rsdm/guide"

            runHook postInstall
          '';
          meta = {
            description = "Standalone Rust TTY/TUI Wayland display manager and screen locker";
            homepage = "https://github.com/Frysuni/rsdm";
            license = pkgs.lib.licenses.gpl3Only;
            mainProgram = "rsdm";
            platforms = systems;
          };
        };

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
        rsdm-source = mkUnstable system;
        rsdm-prebuilt = mkStable system;
        rsdm-unstable = rsdm-source;
        rsdm-stable = rsdm-prebuilt;
        default = rsdm-prebuilt;
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
