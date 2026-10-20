# niri

niri ships a `niri-session` wayland session and brings up
`graphical-session.target` itself via `niri.service`. rsdm detects this and lets
niri own and tear down its own session (see
[session-manager.md](../session-manager.md)).

## Launch it from the greeter

Fixed session (always launch niri, hide the picker):

```toml
[dm]
fixed_session = "niri-session"
```

NixOS:

```nix
services.rsdm.dm.fixedSession = "niri-session";
```

Or leave `fixed_session` unset and pick niri from the discovered
`/usr/share/wayland-sessions/niri.desktop`.

## Lock binding

```kdl
// ~/.config/niri/config.kdl
binds {
    Super+L { spawn "rsdm" "lock"; }
}
```

Choose the output with the unlock UI and the secondary-output policy in rsdm:

```toml
[lock]
enable = true
primary_output = "DP-1"       # `niri msg outputs`
secondary_output = "background" # background | black | off
# size = 2                       # omit for TTY-like auto sizing
```

`off` currently uses the opaque black fallback and keeps niri's outputs enabled.
niri's fractional output scales, including `1.5`, are handled through Wayland
fractional-scale/viewporter buffers.

## Idle locking

No separate swayidle/hypridle process is required:

```toml
[idle]
enable = true
timeout = 300
ignore_inhibitors = false
```

When installed through the NixOS module the user service starts with niri's
`graphical-session.target`. Otherwise enable `rsdm-idle.service` with
`systemctl --user enable --now rsdm-idle.service`.

## Apps as part of the session

```kdl
spawn-at-startup "rsdm" "app" "--" "waybar"
```

## Finalizing (optional)

`niri-session` raises the session target itself, so it does not need this.
Direct bare niri runs under the managed provider and needs to publish its display
and IPC endpoints with finalize. It can do that through its startup command:

```sh
rsdm session start -- niri -- rsdm session finalize
```

This leaves the niri configuration unchanged. A bare setup can also finalize
from its existing startup configuration:

```kdl
spawn-at-startup "rsdm" "session" "finalize"
```
