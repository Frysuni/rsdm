# niri

Choose niri in the Greeter or set `dm.fixed_session = "niri-session"` (NixOS: `services.rsdm.dm.fixedSession`). The upstream desktop entry normally starts niri-session, whose real `niri.service` owns its graphical targets.

## Bind keys and launch apps

Add these entries to your existing `~/.config/niri/config.kdl`:

```kdl
binds {
    Super+L { spawn "rsdm" "lock"; }
    Super+Shift+E { spawn "rsdm" "session" "stop"; }
}

spawn-at-startup "rsdm" "app" "--" "waybar"
```

Merge the bindings into your existing binds block and replace conflicting keys. Enable [Lock](../lock.md) first. Use `niri msg outputs` for the output connector name.

## Readiness

The official niri-session wrapper needs no explicit finalize. For a bare niri session, publish its environment using either its startup command:

```sh
rsdm session start -- niri -- rsdm session finalize
```

or its configuration:

```kdl
spawn-at-startup "rsdm" "session" "finalize"
```

## Idle

Enable [Idle](../idle.md); the packaged user service follows graphical-session.target. Use a single idle monitor. On bare niri, finalize before relying on target-based services.

See the [upstream niri documentation](https://github.com/niri-wm/niri/wiki) and [session backends](../reference/session-backends.md#niri-window-close).
