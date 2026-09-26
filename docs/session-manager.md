# Session manager

When `session_manager.enabled = true` (the default), the greeter does not exec
the compositor directly. It wraps it in:

```sh
rsdm session start -- <compositor>
```

This gives you a uwsm-style systemd `--user` graphical session without pulling in
uwsm. It is what makes user services, panels, portals and `rsdm app` launches
behave like a normal desktop session. (The keyring is unlocked earlier, by the
greeter login itself - see [keyrings.md](keyrings.md) - not here.)

## What `rsdm session start` does

- Exports to `systemctl --user` and the D-Bus activation environment only what it
  actually owns: `PATH`, `XDG_RUNTIME_DIR`, `XDG_SEAT`/`XDG_VTNR`, plus the
  keyring agent variables the login published (`SSH_AUTH_SOCK`, ...) which it
  inherited from the session environment. rsdm never invents
  `WAYLAND_DISPLAY`/`DISPLAY` - the compositor publishes those.
- Decides who owns the graphical session:
  - A compositor that brings up `graphical-session.target` itself (niri does this
    via `niri.service`; sway is similar) is left to manage and tear down its own
    session. rsdm does NOT touch its targets - fighting its shutdown
    (`niri-shutdown.target`) would stop user services
    (`WantedBy=graphical-session.target`: panels, keyrings, launchers) from
    starting on the next login.
  - A bare compositor that does not is given `graphical-session.target` and
    `xdg-desktop-autostart.target`, anchored by rsdm - but only after the
    environment is published (once `WAYLAND_DISPLAY` shows up in
    `systemctl --user show-environment`), so panels and portals start with a
    ready display.
- Holds the graphical session exactly as long as the compositor lives. When it
  exits, the targets rsdm raised are stopped, related transient units get a clean
  systemd stop, the exported variables are unset, and the user is dropped back to
  the greeter. The anchor unit is also `BindsTo=` the compositor's transient
  unit, so the same cascade (anchor -> `graphical-session.target` ->
  `rsdm app` units) fires inside systemd even if the supervisor process is
  killed before it can tear anything down.

## Launching apps into the session

Programs you want tied to the session (tracked as transient units, stopped with
`graphical-session.target`) should go through `rsdm app`:

```sh
# in your compositor config, instead of `exec foot`:
rsdm app -- foot
```

Each launch becomes a transient unit in `app-graphical.slice`,
named `app-rsdm-<application>@<instance>.service`. The application identity stays
stable between launches so desktop portals can reuse saved permissions; only
the instance changes.
The unit is
`PartOf=graphical-session.target`: when the compositor exits (or the session is
otherwise torn down) systemd stops every one of them. Because that target *is*
the cleanup story, `rsdm app` refuses to launch when no graphical session is
active; during session startup (a `spawn-at-startup` racing the session
manager) it waits up to 10 seconds for `graphical-session.target` to come up
before giving up.

## Finalizing from inside a compositor

Compositors that can run a helper after startup (Hyprland `exec-once`, niri
`spawn-at-startup`) can finalize the session explicitly instead of relying on
auto-detection:

```sh
rsdm session finalize          # export the live env and raise the targets
```

Pass extra environment variable names to also export them:

```sh
rsdm session finalize MY_VAR ANOTHER_VAR
```

## Tuning and disabling

`[session_manager]`:

```toml
[session_manager]
enabled = true
extra_env = []           # extra env var NAMES to export once the compositor is up
ready_timeout_secs = 10  # how long to wait for the Wayland socket before activating anyway
```

Disable the wrapper entirely with `enabled = false` (or
`services.rsdm.sessionManager = false` on NixOS). The compositor then launches
directly. Keyring unlock is unaffected - it happens in the greeter login, not in
the wrapper (see [keyrings.md](keyrings.md)).

## Notes

- Do not `nixos-rebuild switch` from inside an rsdm-managed session if it would
  restart user services mid-session; a clean re-login is safer.
- If user services do not start, check they are `WantedBy=graphical-session.target`
  and enabled in `systemctl --user`, and read `journalctl --user -b`. When the
  compositor manages its own session (niri/sway), rsdm deliberately keeps its
  hands off the targets.
