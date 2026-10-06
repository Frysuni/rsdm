# Display manager (greeter)

`rsdm dm` is the greeter: a TTY/TUI login that owns one virtual terminal, asks
for a username and password, authenticates through PAM, and launches the chosen
Wayland session as that user. It is normally started by `rsdm.service`, not by
hand.

```sh
rsdm dm --config /etc/rsdm.toml
```

## How it runs

1. Load and validate the config. If the config is unusable, exit with an error
   without touching a VT or guessing the fallback policy.
2. Resolve or discover the Wayland session (fixed or picker).
3. Acquire the configured VT. A second rsdm is refused; a VT still owned by a
   live session (a restarted greeter while the compositor runs, a console
   login) is waited out rather than drawn over.
4. Render the TUI and read username/password.
5. On submit, fork the session-leader child; the form stays up showing
   "Authenticating..." and a failure is shown inline, with no flash of the
   raw console.
6. The child authenticates through the `rsdm` PAM service, resolves the
   account, sets `PAM_TTY`/`XDG_VTNR`/`XDG_SEAT`/`XDG_SESSION_TYPE`/
   `XDG_SESSION_DESKTOP`, and opens the PAM session (after `setsid`) so logind
   binds it to the right seat and VT - the session lives in `session-N.scope`,
   not in `rsdm.service`, so stopping the greeter never logs it out.
7. Once the greeter has torn down and cleared the terminal, the child forks
   the session process: set env, init groups, drop uid/gid, `chdir` `$HOME`,
   and exec the session.
8. The greeter waits for the session, reclaims the VT, and redraws.

When `session_manager.enabled = true` the compositor is launched wrapped in
`rsdm session start -- <compositor>`. See [session-manager.md](session-manager.md).

## Choosing the session

`[dm] fixed_session`:

- set it - always launch that session and hide the choice. The value is a
  `.desktop` id (file stem), a session `Name`, or a literal command, e.g.
  `niri-session`, `Hyprland`, `startplasma-wayland`.
- omit it - discover `.desktop` sessions under `[dm] session_dirs` and let the
  user pick.

`[dm.remember]` can pre-fill the last username and pre-select the last session.

## Look and feel

The greeter look lives in `[dm.design]`: theme, border, animated background and
speed, the generated FIGlet ASCII title (font), and password masking. With
`menu = true`, `F1` opens a runtime switcher to preview these live (never
persisted). See [configuration.md](configuration.md).

## The service

The shipped unit:

```ini
ExecStart=/usr/bin/rsdm dm --config /etc/rsdm.toml
Conflicts=getty@tty1.service
StandardInput=tty
StandardOutput=journal
StandardError=journal
Alias=display-manager.service
WantedBy=graphical.target
```

`StandardOutput`/`StandardError` must be `journal`: rsdm logs to stderr and the
unit routes it to the journal, so logs never corrupt the TTY. `rsdm dm` refuses
to start if stdout/stderr is the active terminal.

The unit deliberately does not set `TTYReset`/`TTYVHangup`/`TTYVTDisallocate`:
systemd applies those both before and *after* the unit runs, and after a login
the VT belongs to the user's compositor, whose logind session outlives
`rsdm.service` - the stop-time teardown would vhangup the live session's VT.
The greeter claims and resets the VT itself.

VT ownership checks include both controlling terminals and open VT descriptors
held by detached session processes. DM restores a graphics VT to text mode only
when no foreign holder remains. Unreadable process state is treated as busy.

## Multiple monitors

The greeter is a Linux VT/fbcon application. A VT has one global character grid
which the kernel clones to every connector; unlike the Wayland locker, rsdm does
not receive a separate drawable surface or size for each monitor. Consequently
the greeter cannot independently place a login card or a black background on a
secondary monitor from TOML.

For a single-monitor greeter, disable the unwanted connector while fbcon is
active with the kernel `video=` parameter (connector names are visible under
`/sys/class/drm`, for example `video=HDMI-A-1:d`). On NixOS put it in
`boot.kernelParams`; on GRUB-based Arch/Fedora/Ubuntu put it in the kernel command
line and regenerate the GRUB config. The graphical compositor can enable and
configure that connector again when the user session starts. Do not use a
userspace DRM modeset helper from the greeter: it would compete with logind/DRM
master ownership immediately before session launch.

```sh
journalctl -u rsdm.service -b
```

## Fallback

`[dm.fallback]` is the safety net. If the greeter cannot run (no
sessions, broken PAM) or the user confirms "exit to console", rsdm execs the
fallback login on the VT:

```toml
[dm.fallback]
enabled = true
command = ["agetty", "--noclear", "tty1", "linux"]
```

`agetty` is preferred: it opens and configures the VT itself and then runs
`login`, which is far more robust than exec'ing `login` directly. The TTY name
must match `[dm.tty]`. rsdm will not run the fallback onto a VT that is already
contended.

Fallback requires a successfully loaded configuration. A missing, invalid, or
incompatible config causes DM to exit; it never substitutes a default VT or
enables fallback in place of the administrator's policy.

DM also refuses fallback when `security.deny_root = true` or
`security.allowed_groups` is non-empty, because console login cannot enforce
these restrictions. This applies both to Greeter errors and to user-requested
exit; the Greeter hides the exit action when fallback is unavailable. With
`deny_root = false`, empty `allowed_groups`, and fallback enabled, console access
uses the system login PAM stack and its own rate limits.

## Security

The greeter never checks passwords itself - PAM does. Password buffers are
zeroized; the session child drops privileges (supplementary groups, setgid,
setuid, chdir HOME) before exec. See [SECURITY.md](SECURITY.md).
