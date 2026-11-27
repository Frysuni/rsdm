---
description: Enable and customize the Wayland screen locker, including multi-monitor behavior.
---

# Lock

Lock is a Wayland client using ext-session-lock-v1. It can run with or without RSDM DM.

## Enable and try

::: code-group

```toml [TOML]
[lock]
enable = true
```

```nix [NixOS]
services.rsdm.lock.enable = true;
```

:::

Inside your graphical session as your normal user:

```sh
rsdm lock
```

Enter your account password to unlock. An empty field can start a PAM conversation (for example fingerprint or token authentication). Later prompts request their own credentials. Your compositor must support and permit the session-lock protocol; see [compatibility](compatibility.md).

## Keyboard shortcuts

| Key | Action |
| --- | --- |
| Enter | Submit credentials |
| Esc | Clear input or cancel PAM conversation |
| F1 | Appearance menu, if enabled |
| F9 / F10 | Hibernate / sleep, if available |
| F11 / F12 | Reboot / power off |

Power actions require two presses. A different key cancels the confirmation. The screen remains locked on rejection or cancellation.

Add rsdm lock to a key binding in [niri](compositors/niri.md), [Hyprland](compositors/hyprland.md) or [Sway](compositors/sway.md).

## Multiple displays and scaling

```toml
[lock]
primary_output = "DP-1"
secondary_output = "black"
size = 2
```

The largest connected output by pixel area is selected if `primary_output` is omitted. The unlock form, menu, clock and status appear only on the primary output. Secondary outputs show the background or opaque black. The `off` setting currently behaves like `black` and keeps displays powered.

Omit `size` (or set it to 1) to use the Greeter's native physical console glyph dimensions. Values 2–12 enlarge the font. Fractional desktop scaling is handled separately.

::: details Matching the Greeter's font
DM publishes the console font to `/run/rsdm/console-font-<tty>.bin`. Lock reads it and falls back to 8×16 if it is unavailable. Restart DM at the next safe login after changing the console font.
:::

## Appearance and personal settings

```toml
[lock.design]
theme = "catppuccin"
wallpaper = "/etc/rsdm-wallpaper.png"
wallpaper_dim = 6
background = "rain"
background_opacity = 8
```

Use an absolute PNG or JPEG path readable by the desktop user. The wallpaper is cover-fit and center-cropped. Its dim level is applied first, followed by the animation and login card. The F1 menu previews supported visual options. Select **Save settings** to store personal overrides in the user's configuration directory. Wait for the save confirmation before unlocking. These override the corresponding system visual settings on the next launch; see [configuration](configuration.md#personal-lock-settings) for the exact fields and reset procedure.

::: details Rendering and background behavior
Full-field effects (plasma, fire) remain visible behind the card and text. Sparse effects (matrix, rain, starfield) retain the theme background behind the card. With wallpaper and no animation the card is transparent. Lock caches scaled wallpaper per output and reuses up to two shared-memory buffers per output. Without fractional-scale and viewporter support, it uses an integer-scale fallback.
:::

::: details Render a preview without locking
From a source checkout with the [build dependencies](development.md):

```sh
cargo run --locked -p rsdm-lock --example preview -- \
  /tmp/rsdm-lock-preview.png nord rsdm modern rain 15 none 8
xdg-open /tmp/rsdm-lock-preview.png
```

The positional arguments are output, theme, preset logo, border, background, frame number, optional wallpaper (or none), and background opacity. The example uses representative credentials and does not lock the desktop.
:::

## Emergency unlock

If the locker is still running but authentication is unavailable, switch to another VT, authenticate as an administrator and run:

```sh
sudo rsdm unlock
# For multiple users:
# sudo rsdm unlock --user alice
```

You can also run `rsdm unlock` directly; it requests authorization through sudo, or pkexec if sudo is unavailable. A crashed locker may require ending the affected compositor session. Never rely on killing a lock process to unlock. See [security](SECURITY.md#lock-screen) and [troubleshooting](troubleshooting.md).
