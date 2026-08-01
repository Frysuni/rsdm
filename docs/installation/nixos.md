# Installation: NixOS

`rsdm` ships a flake with a NixOS module and two installation modes:

| package          | what it is                                                     |
|------------------|----------------------------------------------------------------|
| `rsdm-prebuilt`  | downloads the release binary and patches it for the Nix store  |
| `rsdm-source`    | builds from the exact flake source with Crane                  |
| `rsdm-stable`    | compatibility alias for `rsdm-prebuilt`                       |
| `rsdm-unstable`  | compatibility alias for `rsdm-source`                         |
| `default`        | alias for `rsdm-prebuilt`                                     |

The prebuilt package is the default and does not compile Rust locally. Select
`rsdm-source` when you explicitly want to build and run the input revision from
source. Pin the flake input to a tag or commit for a stable deployment.

## Flake module

```nix
{
  inputs.rsdm.url = "github:Frysuni/rsdm";

  outputs = { nixpkgs, rsdm, ... }: {
    nixosConfigurations.host = nixpkgs.lib.nixosSystem {
      modules = [
        rsdm.nixosModules.default
        {
          services.rsdm = {
            enable = true;

            # Default: release binary, no Rust compilation.
            package = rsdm.packages.x86_64-linux.rsdm-prebuilt;

            # Build the selected flake revision from source instead:
            # package = rsdm.packages.x86_64-linux.rsdm-source;

            dm = {
              tty = "tty1";

              # always launch one session, hide the picker:
              fixedSession = "niri-session";

              design.theme = "catppuccin";
            };

            # lock screen: enabled, with its own look + wallpaper:
            lock = {
              enable = true;
              primaryOutput = "DP-1";
              secondaryOutput = "background";
              size = null; # TTY-like auto size; or 1..12
              design = {
                theme = "catppuccin";
                wallpaper = ./wall.png;
                wallpaperDim = 6;
                backgroundOpacity = 8;
              };
            };

            idle = {
              enable = true;
              timeout = 300;
              ignoreInhibitors = false;
              onLock = [ "notify-send 'screen locked'" ];
              onUnlock = [ ];
            };
          };
        }
      ];
    };
  };
}
```

```sh
sudo nixos-rebuild switch
```

The module:

- writes `/etc/rsdm.toml` from the options below (escape hatch:
  `services.rsdm.extraConfig` for raw extra TOML, or `services.rsdm.config` to
  override the whole thing),
- defines the `rsdm` (greeter login, unlocks the keyring) and `rsdm-lock` PAM
  services,
- disables `getty` on the greeter VT,
- creates `rsdm.service` with journald output and a `display-manager.service`
  alias on tty1 when `dm.enable = true`,
- creates `rsdm-idle.service` in each graphical user session when `idle.enable`
  is true,
- sets `systemd.defaultUnit = "graphical.target"` so the greeter is reached at
  boot when `dm.enable = true`. A lock/idle-only configuration leaves the
  system's getty, default boot target, and display-manager role untouched.

## Common options

The options mirror the config shape: greeter under `dm.*`, locker under `lock.*`,
each look under its own `.design`.

| option                                | meaning                                            |
|---------------------------------------|----------------------------------------------------|
| `enable`                              | turn rsdm on                                       |
| `package`                             | rsdm package to install                            |
| `dm.enable`                           | run the greeter (default `true`)                   |
| `dm.tty` / `dm.seat`                  | VT the greeter owns (e.g. `"tty1"`) and logind seat |
| `dm.fixedSession`                     | always launch one session, hide the picker (or `null`) |
| `dm.sessionDirs`                      | dirs scanned for `.desktop` sessions (or `null` = defaults) |
| `dm.remember.username` / `.session`   | pre-fill last login                                |
| `dm.fallback.enable` / `.command`     | login program to hand the VT to if the greeter cannot run |
| `dm.design.*`                         | greeter look: `theme`, `borderStyle`, `background`, `backgroundSpeed`, `titleMode`, `titleText`, `titlePreset`, `titleFont`, `passwordMode`, `menu`, `showClock`, `showHostname` |
| `lock.enable`                         | allow locking (default `false`)                    |
| `lock.primaryOutput`                  | output with the unlock UI (`null` = largest output) |
| `lock.secondaryOutput`                | `background`, `black`, or `off`                    |
| `lock.size`                           | `null` for TTY-like auto sizing, or integer `1..12` |
| `lock.design.*`                       | locker look: same fields as `dm.design`, plus `wallpaper`, `wallpaperDim`, `backgroundOpacity` |
| `idle.enable` / `idle.timeout`        | enable compositor-driven idle lock; inactivity seconds |
| `idle.ignoreInhibitors`               | consider input only, ignoring application inhibitors |
| `idle.lockCommand`                    | optional alternate locker argv (`[]` = built-in)   |
| `idle.onLock` / `idle.onUnlock`       | shell hooks around an idle-started lock             |
| `keyring`                             | `auto` / `gnome` / `kwallet` / `none`              |
| `sessionManager`                      | wrap the compositor in the systemd `--user` session manager (bool) |
| `disableGetty`                        | free the greeter VT from getty                     |
| `displayManagerAlias`                 | alias the unit as `display-manager.service`        |
| `logging.level` / `logging.file`      | log verbosity; optional extra log file (journal is always on) |

`dm.design` and `lock.design` are each a complete design with its own defaults -
there are no shared defaults and no overrides. The greeter and locker take the
same design fields, except the wallpaper fields, which exist on the locker only.

The runtime F1 menu can preview lock size and secondary-output changes. Its Save
settings action cannot modify this module-generated file because it points into
the immutable Nix store; set the corresponding options above and run
`sudo nixos-rebuild switch` to persist them.

## Keyrings

`services.rsdm.keyring` selects which keyring module the `rsdm` login stack runs,
so the keyring is unlocked by the same login that checks your password (as
login/gdm/sddm do). `auto` (the default) mirrors the rest of your system
(gnome-keyring if any PAM service or `services.gnome.gnome-keyring` enables it;
kwallet if `kwallet.enable` is on). See [keyrings.md](../keyrings.md).
