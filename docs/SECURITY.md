# Security

`rsdm` authenticates users through PAM and never implements password checking
itself.

Security boundaries:

- PAM service: `rsdm`
- Password buffers use `zeroize`
- Passwords must not be logged
- The session child drops privileges with supplementary groups, `setgid`,
  `setuid`, and `chdir HOME` before `exec`
- `XDG_SESSION_TYPE=wayland` is set for launched sessions
- PAM environment is applied after validation
- stdout/stderr must be connected to journald or another log sink, not the
  active TTY

The shipped systemd service uses:

```ini
StandardInput=tty
StandardOutput=journal
StandardError=journal
```

(and deliberately no `TTYReset`/`TTYVHangup`/`TTYVTDisallocate` - systemd
applies those after the unit stops too, onto the VT that by then belongs to
the user's live session; see [display-manager.md](display-manager.md)).

## PAM conversation

Greeter and Lock send the entered password to the first hidden PAM prompt.
Later hidden prompts request separate credentials, such as an OTP; echoed
prompts accept visible input. PAM information and error messages are displayed
alongside the next prompt. Enter submits a response and Esc cancels the
transaction. Secret responses use zeroizing buffers and are never reused for
another prompt. Lock keeps processing Wayland events during authentication.

Greeter applies account policy to the final username returned by PAM, including
stacks that map a submitted identity to a local account. Lock requires that
PAM's final identity still matches the seated user. An unavailable conversation,
unsupported PAM message type, or cancelled response fails authentication.

## Attempt limits

Greeter keeps counters in its parent process across PAM attempts. It checks
both submitted identities and the directory/PAM account names reported by the
helper, including failed attempts; aliases that map to one account share that
account's budget. Mapping is checked before opening a PAM session. Unknown
aliases can require PAM work to discover their account; an exhausted mapped
account still cannot authorize a session.

DM and Lock track at most 1024 account names per process. Input is limited to
256 bytes for a username and 4096 bytes for each credential. Partial failure
counters expire after 15 minutes without a failure, or after
`failure_delay_ms` if that is longer. Blocked counters expire when their
configured delay ends. When the table is full, new names are rate limited
until an entry expires; existing blocked names are never evicted by a flood
of other names.

## Lock screen

`rsdm lock` is an `ext-session-lock-v1` Wayland client. Its security profile:

- Authenticates the seated user through the `rsdm-lock` PAM service, which runs
  only the `auth`/`account` stacks - it never opens a session or changes tokens
- Password buffers use `zeroize` and are taken (not copied) for verification
- Failed attempts are rate limited with the same limiter as the greeter
- Only the current user can unlock; the username is resolved from the process's
  effective UID. Caller-controlled environment variables such as `USER` and
  interactive input cannot substitute the PAM identity
- The compositor keeps the lock surface on top until `unlock` is sent, so the
  protocol - not the client - guarantees the screen stays covered
- Secondary-output presentation never exposes session contents: background and
  black are opaque lock surfaces, while `off` is used only after the compositor
  has confirmed the session lock and falls back to opaque black if unsupported
- Emergency `rsdm unlock` is root-only. It verifies the runtime file owner,
  target UID, executable name, `lock` argv and `/proc` start time, then pins the
  process with a pidfd and repeats process validation. It verifies the pinned
  process survived those checks and sends SIGUSR1 through that descriptor, so
  numeric PID reuse cannot redirect the signal to a replacement process. The
  signal handler also checks the kernel-supplied sender UID and ignores every
  non-root signal, so `kill -USR1` cannot bypass PAM. The live locker then sends
  the ordinary Wayland `unlock` request; killing a locker is intentionally never
  used because the compositor must fail securely.

## Idle hooks

`idle.on_lock` and `idle.on_unlock` are argv commands executed as the desktop
user. They never invoke a shell. With the built-in locker, `on_lock` is delayed
until compositor lock confirmation; user activity never causes an automatic
unlock.

## Session coordination

The user coordinator authenticates D-Bus callers by UID and checks generation
on lifecycle requests. It acquires its non-replaceable bus name before touching
shared manager state. Recovery files are private, owner-checked, bounded regular
files opened without following symlinks. Root never parses them: it launches the
same executable's cleanup command after dropping UID/GID and supplementary groups,
and bounds the helper wait. Cleanup verifies recorded unit InvocationID; signals
use pidfds and verified cgroup membership, rather than PID guesses or KillUser.

XSMP uses local Unix ICE listeners, private cookie authentication, no host-based
authentication, and kernel peer credentials. A client must be in a registered
application's verified invocation. Its properties, including SmProcessID and
restart commands, cannot grant ownership or execute commands. The server does
not replace native DE session-management endpoints.

Quit commands execute as the desktop user with the application's environment.
Power requests remain subject to logind/polkit; RSDM never retries them as root.
Delay inhibitors live through cleanup and PAM end but cannot extend the OS's
finite shutdown budget. Lock retains its surfaces on power rejection/cancellation.
