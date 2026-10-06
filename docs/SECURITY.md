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

Greeter and Lock support password-style PAM stacks: one username and one secret
response per transaction. Echoed username prompts receive the submitted account
name. The password is never reused for a second secret prompt, even in a later
callback; additional credential prompts fail with a PAM conversation error.
Password-plus-OTP, interactive MFA, and challenge-response stacks require an
interactive conversation UI and are currently unsupported. Configure separate
`rsdm` and `rsdm-lock` PAM stacks compatible with this single-password model.

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
  target UID, executable name, `lock` argv and `/proc` start time before sending
  SIGUSR1, preventing stale PID reuse or signaling an unrelated process. The
  signal handler also checks the kernel-supplied sender UID and ignores every
  non-root signal, so `kill -USR1` cannot bypass PAM. The live locker then sends
  the ordinary Wayland `unlock` request; killing a locker is intentionally never
  used because the compositor must fail securely.

## Idle hooks

`idle.on_lock` and `idle.on_unlock` are intentionally shell commands executed
as the desktop user. The system configuration is therefore trusted input. With
the built-in locker, `on_lock` is delayed until compositor lock confirmation;
user activity never causes an automatic unlock.
