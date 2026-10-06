# Idle locking

`rsdm idle` is a small Wayland idle daemon built into rsdm. It uses
[`ext-idle-notify-v1`](https://wayland.app/protocols/ext-idle-notify-v1), so the
compositor decides when the seat is idle; rsdm does not capture global keyboard
or pointer input.

The monitor follows every seat advertised by this Wayland connection, including
hotplugged seats, and starts locking only when all of them are idle. Activity on
any connected seat prevents a different idle seat from locking the session.

```toml
[idle]
enable = true
timeout = 300
ignore_inhibitors = false
lock_command = []
on_lock = ["notify-send 'rsdm lock active'"]
on_unlock = ["notify-send 'rsdm unlocked'"]
```

- `timeout` is measured in seconds.
- With `ignore_inhibitors = false` (the default), visible applications may
  inhibit idle locking during video playback or a presentation. `true` asks
  protocol version 2 to consider input activity only; a version 1 compositor
  logs a warning and continues honoring inhibitors.
- An empty `lock_command` starts the current rsdm binary with `lock` and the
  same config path. This mode has a readiness handshake: `on_lock` runs only
  after the compositor confirms that the session is securely locked.
- A non-empty `lock_command` is an argv array, for example `["swaylock"]`.
  It must stay in the foreground until unlock; daemonizing commands make it
  impossible for rsdm to know when the lock ends. rsdm cannot observe another
  locker's protocol state, so `on_lock` runs after the foreground process starts.
- `on_lock` and `on_unlock` are shell commands, run in order as the desktop
  user. `on_unlock` runs only for a lock cycle whose activation reached the
  configured readiness point and whose locker exited successfully. It also runs
  after a privileged emergency unlock, but never after a crash, signal, nonzero
  exit, or wait failure. Do not put untrusted config text in these fields.

The daemon never unlocks on pointer movement or resume. Activity only resets
the next idle timeout; authentication still belongs to `rsdm lock`.

## Starting the daemon

NixOS enables `rsdm-idle.service` automatically when
`services.rsdm.idle.enable = true`. With a packaged user unit:

```sh
systemctl --user enable --now rsdm-idle.service
systemctl --user status rsdm-idle.service
```

It can also run directly in the foreground for debugging:

```sh
RUST_LOG=rsdm_idle=debug,rsdm_lock=debug rsdm idle
```

The service is tied to `graphical-session.target`, so it stops with the Wayland
session. The built-in locker also requires `[lock].enable = true`; an alternate
`lock_command` does not. The compositor must implement `ext-idle-notify-v1`.
When service-managed, each locker runs in its own transient user scope. Thus
stopping/restarting the idle monitor cannot kill an active session-lock owner
and strand the compositor on an abandoned lock; systemd still tracks both
process lifecycles normally.
