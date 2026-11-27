---
description: Complete RSDM TOML reference with defaults, allowed values and NixOS equivalents.
---

# Configuration

RSDM reads `/etc/rsdm.toml` by default. Every field is optional. Unknown keys and invalid values are rejected. Changes are read at the next component launch; Session Manager also monitors its live configuration.

```sh
rsdm validate-config
rsdm --config ./rsdm.toml validate-config
```

Use the [complete annotated example](https://github.com/Frysuni/rsdm/blob/main/rsdm.toml) as a starting point. On NixOS, configure `services.rsdm` in your system configuration; the module generates the file. The NixOS equivalents below are relative to that option.

## Components

::: code-group

```toml [TOML]
[dm]
enable = true

[lock]
enable = true

[idle]
enable = true
timeout = 300

[session_manager]
enabled = true
```

```nix [NixOS]
services.rsdm = {
  enable = true;
  dm.enable = true;
  lock.enable = true;
  idle.enable = true;
  idle.timeout = 300;
  sessionManager = true;
};
```

:::

## DM and Greeter

| TOML field | Default | NixOS |
| --- | --- | --- |
| `dm.enable` | `true` | `dm.enable` |
| `dm.pam_service` | `"rsdm"` | raw config |
| `dm.fixed_session` | unset | `dm.fixedSession` |
| `dm.session_dirs` | system paths (below) | `dm.sessionDirs` |
| `dm.tty.path` | `"/dev/tty1"` | `dm.tty` (e.g. `"tty1"`) |
| `dm.tty.seat` | `"seat0"` | `dm.seat` |
| `dm.remember.username` | `true` | `dm.remember.username` |
| `dm.remember.session` | `true` | `dm.remember.session` |
| `dm.fallback.enabled` | `true` | `dm.fallback.enable` |
| `dm.fallback.command` | `[]` | `dm.fallback.command` |

::: details Session discovery and fallback
Session directories are searched in order, defaulting to /usr/local/share/wayland-sessions and /usr/share/wayland-sessions. NixOS prepends /run/current-system/sw/share/wayland-sessions by default; set `dm.sessionDirs` = null for generic defaults. Paths must be absolute. Fixed session accepts a desktop ID, unique name/Exec, or literal command. Ambiguous names/Exec values require an exact desktop ID.

Use a Linux virtual terminal path, such as /dev/tty1. Configure matching service wiring. Remember options prefill the last successful login and picker selection. Empty fallback argv auto-detects system getty. Explicit argv is literal. Fallback requires deny_root = false, allowed_groups = [], a usable configuration and a free VT.
:::

See [DM and Greeter](display-manager.md).

## Lock

| TOML field | Default | NixOS |
| --- | --- | --- |
| `lock.enable` | `false` | `lock.enable` |
| `lock.pam_service` | `"rsdm-lock"` | raw config |
| `lock.primary_output` | unset (largest pixel area) | `lock.primaryOutput` |
| `lock.secondary_output` | `"background"` | `lock.secondaryOutput` |
| `lock.size` | unset (native size) | `lock.size` |

Secondary output values are background, black and off (currently opaque black with outputs powered). Size is an integer from 1 to 12, where 1 is native glyph size. See [Lock](lock.md).

## Idle

| TOML field | Default | NixOS |
| --- | --- | --- |
| `idle.enable` | `false` | `idle.enable` |
| `idle.timeout` | `300` seconds | `idle.timeout` |
| `idle.ignore_inhibitors` | `false` | `idle.ignoreInhibitors` |
| `idle.lock_command` | `[]` | `idle.lockCommand` |
| `idle.on_lock` | `[]` | `idle.onLock` |
| `idle.on_unlock` | `[]` | `idle.onUnlock` |

Timeout accepts 1–4294967 seconds. Empty lock_command selects built-in Lock, which must be enabled. Hooks are arrays of argv arrays, e.g. `[["notify-send", "Unlocked"]]`. See [Idle](idle.md) for readiness and inhibitor behavior.

## Appearance

Both `dm.design` and `lock.design` have independent defaults. NixOS uses the same prefixes with camelCase field names.

| Field | Default | NixOS field |
| --- | --- | --- |
| `theme` | `"monochrome"` | `theme` |
| `border_style` | `"classic"` | `borderStyle` |
| `background` | `"none"` | `background` |
| `background_speed` | `5` (0–10) | `backgroundSpeed` |
| `title_mode` | `"custom"` | `titleMode` |
| `title_text` | unset (RSDM) | `titleText` |
| `title_preset` | `"rsdm"` | `titlePreset` |
| `title_font` | `"ANSI Shadow"` | `titleFont` |
| `show_clock` | `true` | `showClock` |
| `show_hostname` | `true` | `showHostname` |
| `password_mode` | `"hidden"` | `passwordMode` (default asterisks) |
| `menu` | `true` | `menu` |

::: details All appearance choices
| Setting | Values |
| --- | --- |
| Themes | minimal, monochrome, cyberpunk, catppuccin, gruvbox, nord, dracula, tokyo-night, material, solarized, eldritch, rama, dark, trans-is-hard-job |
| Borders | none, classic, modern, minimal, ascii1, ascii2, ascii3, ascii4, wave, pulse |
| Backgrounds | none, matrix, fire, rain, plasma, starfield |
| Title modes | custom, hostname, os-release, session-name, preset-logo |
| Preset logos | minimal, block, rsdm, rustty, waytty, niri, hyprland, kde, gnome, nixos, arch, tux |
| Fonts | figrs/FIGlet font names |
| Password | hidden, asterisks |

Speed 0 freezes animation. Custom title mode uses title_text; preset-logo uses title_preset. Menu enables F1.
:::

### Lock wallpaper

| Field in `lock.design` | Default | NixOS field |
| --- | --- | --- |
| `wallpaper` | unset | `wallpaper` |
| `wallpaper_dim` | `6` | `wallpaperDim` |
| `background_opacity` | `8` | `backgroundOpacity` |

Wallpaper is an absolute PNG/JPEG path. Dim ranges from 0 (black) to 10 (original). Background opacity ranges from 0 (wallpaper only) to 10 (opaque effect) and is ignored without a wallpaper. The TOML schema accepts wallpaper fields in the Greeter design, where they have no visual effect.

### Personal Lock settings

F1 changes the current design live. Lock's **Save settings** writes a personal overlay to `$XDG_CONFIG_HOME/rsdm/lock.toml` (or `~/.config/rsdm/lock.toml` if `XDG_CONFIG_HOME` is unset or relative). The overlay is loaded after system configuration.

::: details Exactly what can be saved
Lock size, size reset, secondary_output and the design's theme, border_style, background, background_speed, title_font, wallpaper_dim and background_opacity. Authentication, PAM, wallpaper path, primary output and other administrative settings stay in the system file. Greeter changes last only for the current instance.
:::

To reset saved preferences, rename the overlay while unlocked. An invalid overlay prevents Lock from starting, so use another VT if necessary.

```sh
mv "${XDG_CONFIG_HOME:-$HOME/.config}/rsdm/lock.toml" \
   "${XDG_CONFIG_HOME:-$HOME/.config}/rsdm/lock.toml.bak"
```

If `XDG_CONFIG_HOME` is relative, use the fallback path explicitly.

## Session Manager

| TOML field | Default | NixOS |
| --- | --- | --- |
| `session_manager.enabled` | `true` | `sessionManager` |
| `session_manager.extra_env` | `[]` | raw config |
| `session_manager.ready_timeout_secs` | `10` | raw config |

Extra_env names additional variables to publish at readiness. Timeout is nonnegative, in seconds, and must fit a monotonic deadline. A managed readiness timeout leaves targets inactive until finalize. See [Session Manager](session-manager.md).

## Security and paths

| TOML field | Default | Values |
| --- | --- | --- |
| `security.deny_root` | `true` | Boolean |
| `security.allowed_groups` | `[]` | Local group names (any one suffices) |
| `security.max_failed_attempts` | `5` | 1–255 |
| `security.failure_delay_ms` | `1200` | Nonnegative milliseconds |
| `paths.cache_dir` | `"/var/cache/rsdm"` | Absolute directory |

The root/group settings restrict DM logins. Both Greeter and Lock use the configured attempt limit and delay. These are raw settings on NixOS. See [security](SECURITY.md) for PAM identity, throttling and trust boundaries.

## Logging

| TOML field | Default | NixOS |
| --- | --- | --- |
| `logging.level` | `"info"` | `logging.level` |
| `logging.file` | unset | `logging.file` |

Levels are error, warn, info, debug and trace. The optional file path must be absolute, in trusted directories, and a regular file owned by the process user with no shared writes or symlinks. Journald remains enabled. `RUST_LOG` overrides verbosity for individual launches.

## NixOS module options

These options control packaging and service wiring rather than runtime TOML.

| Option | Default | Purpose |
| --- | --- | --- |
| `enable` | `false` | Install and configure RSDM |
| `channel` | `"stable"` | stable or unstable |
| `package` | channel package | Override selected package |
| `keyring` | `"auto"` | auto, gnome, kwallet or none |
| `disableGetty` | `true` | Free the configured VT |
| `displayManagerAlias` | `null` | Automatic on tty1, explicit boolean override |
| `extraConfig` | `{}` | Raw values merged recursively last |
| `config` | `{}` | Deprecated raw alias, merged before extraConfig |

For example:

```nix
services.rsdm.extraConfig = {
  security.max_failed_attempts = 3;
  session_manager.extra_env = [ "MY_VARIABLE" ];
};
```

See the [NixOS installation guide](installation/nixos.md) and [keyrings](keyrings.md).
