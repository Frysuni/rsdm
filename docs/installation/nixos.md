# Installation: NixOS

`rsdm` provides two channels for x86_64-linux and aarch64-linux:

| channel | behavior |
|---------|----------|
| `stable` (default) | downloads a stable GitHub Release binary; no Rust compilation |
| `unstable` | builds the source revision selected by your flake input locally |

Both use the same input. You do not need to specify commits or prerelease tags.
The lock file records the revision automatically when you update the input.

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
            channel = "stable"; # or "unstable" to build the development version

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

## Updating and switching channels

Keep `inputs.rsdm.url = "github:Frysuni/rsdm"` and choose either:

```nix
services.rsdm.channel = "stable";
# services.rsdm.channel = "unstable";
```

Update from your system flake directory, then rebuild (replace `host` with your
NixOS configuration name):

```sh
nix flake update rsdm
sudo nixos-rebuild switch --flake .#host
```

Stable uses the release version and archive hashes recorded in the updated
flake. The release workflow updates these after publishing a stable release.
Unstable compiles the source selected by the updated lock file, including
unreleased changes on the main branch. Prereleases do not change stable.

The module does not add caches, keys, or global Nix download settings. Both
channels use your existing substituters, normally including `cache.nixos.org`,
for available dependencies. Unstable builds RSDM locally; dependencies missing
from those caches are also built locally. Project-specific Rust dependencies
may need compilation on the first build. Subsequent builds reuse matching
local store paths. There is no additional cache setup or trust prompt.

For direct package use, the flake exposes `rsdm-stable` and `rsdm-unstable`;
`default` selects stable. `rsdm-prebuilt` and `rsdm-source` remain compatibility
aliases. An explicit `services.rsdm.package` overrides the channel selection.

The module rejects `dm.design.borderStyle = "none"` and
`lock.design.borderStyle = "none"` when the selected package is older than 2.0.0,
including values supplied through `config` or `extraConfig`. Use a supported
border such as `classic`, or select the unstable channel for this style.

### Migrating from the GitHub Pages cache

Remove `services.rsdm.binaryCache.enable` from your configuration; this option
has been removed. Remove an explicit `services.rsdm.package` if you want the
new `channel` setting to select the package. Also remove any manually added
RSDM Pages substituter and signing key from your Nix settings.

Until the first successful switch, the running daemon may still have the old
Pages cache configured. Bypass it for that rebuild:

```sh
sudo nixos-rebuild switch --flake .#host \
  --option substituters https://cache.nixos.org \
  --option extra-substituters ""
```

This command temporarily excludes all additional caches; include any other
caches you need in the `substituters` argument. Later rebuilds need no override.
Do not disable substitution globally: that would also prevent downloads of
ordinary dependencies from the public NixOS cache.

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
| `channel`                             | `stable` (default) or `unstable`                   |
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
