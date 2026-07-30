# Hyprland

Hyprland installs a `hyprland.desktop` wayland session. Whether it raises
`graphical-session.target` on its own depends on your setup; rsdm auto-detects
and, for a bare compositor, anchors the target itself once the Wayland
environment is published (see [session-manager.md](../session-manager.md)).

## Launch it from the greeter

Fixed session:

```toml
[dm]
fixed_session = "Hyprland"     # or the .desktop id "hyprland"
```

NixOS:

```nix
services.rsdm.dm.fixedSession = "Hyprland";
```

Or leave `fixed_session` unset and pick Hyprland from
`/usr/share/wayland-sessions/hyprland.desktop`.

## Lock binding

```ini
# ~/.config/hypr/hyprland.conf
bind = SUPER, L, exec, rsdm lock
```

For idle locking, either keep hypridle and run `rsdm lock` from a listener, or
enable rsdm's own `[idle]` block and `rsdm-idle.service`; do not enable both.

## Apps as part of the session

```ini
exec-once = rsdm app -- waybar
```

## Finalizing (optional)

If your Hyprland does not bring up the graphical session target itself, finalize
it explicitly after startup:

```ini
exec-once = rsdm session finalize
```
