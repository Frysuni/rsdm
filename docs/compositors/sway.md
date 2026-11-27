# Sway

Choose Sway in the Greeter, or set `dm.fixed_session = "sway"` (NixOS: `services.rsdm.dm.fixedSession`).

## Bind keys and publish readiness

Add to your existing `~/.config/sway/config` (assuming $mod is already defined):

```ini
exec rsdm session finalize
bindsym $mod+l exec rsdm lock
bindsym $mod+Shift+e exec rsdm session stop
exec rsdm app -- waybar
```

Replace conflicting bindings and autostart entries. Enable [Lock](../lock.md) first. Finalize publishes the compositor environment, including `SWAYSOCK`, for a bare session.

```sh
sway --validate
swaymsg -t get_outputs
rsdm session status
```

## Idle

Use [RSDM Idle](../idle.md) or your existing swayidle integration. If you keep swayidle, use a foreground locker and a readiness-aware suspend integration. Avoid running two automatic lockers.

For owned Sway sessions, verified native Wayland application windows can receive close requests. XWayland windows use other applicable application policies.

See [sway(5)](https://github.com/swaywm/sway/blob/master/sway/sway.5.scd) and [session backends](../reference/session-backends.md#sway-window-close).
