# Architecture

The production tree is intentionally narrow:

- `rsdm-core`: domain types, ports, config validation, login/runtime use cases
- `rsdm-infra`: PAM, Unix user resolution, session discovery, state storage,
  privilege dropping, session launch, and the systemd `--user` session manager
- `rsdm-ui`: the one, medium-independent design - theme palette, border styles,
  animated backgrounds, banner/clock, the runtime design switcher, and the whole
  screen composition. Everything draws against an abstract `Surface` (a grid of
  styled glyph cells); it depends only on `rsdm-core` (no ratatui, no Wayland)
- `rsdm-tui`: a thin greeter backend - a `Surface` over ratatui's cell buffer
  plus the input loop; the look comes from `rsdm-ui`
- `rsdm-lock`: a thin `ext-session-lock-v1` Wayland locker - a `Surface` over a
  software framebuffer (the Greeter's console bitmap font and Unicode map, so it
  draws the same frames and backgrounds as the TTY) plus PAM verification
- `rsdm-idle`: an `ext-idle-notify-v1` client that supervises one lock process,
  waits for its compositor-confirmed runtime state, and runs configured hooks
- `rsdm-cli`: binary wiring for `dm`, `lock`, `idle`, emergency `unlock`, logs,
  status, session commands, apps, and config validation

Core does not depend on PAM, libc, ratatui, systemd, Wayland, or other OS APIs.
There is no "skin" concept: a single composition in `rsdm-ui` renders on both
fronts, so a theme/border/background/title means the same on the VT and on
the pixel lock screen. Each backend only adapts cells to its medium and degrades
truecolor as needed (the console quantizes; the framebuffer is exact). The
runtime switcher (F1) mutates the live design in memory. The locker can
explicitly persist its exposed fields through a comment-preserving, validated,
atomic config update; the TTY greeter remains in-memory only.

Greeter (`dm`) runtime path:

1. Load and validate `/etc/rsdm.toml`
2. Discover or resolve the selected Wayland session
3. Acquire the configured VT: refuse a second rsdm outright; when a live
   session still owns the VT (a compositor in graphics mode, a console login -
   the restart-during-a-session case), wait for it to end rather than drawing
   over it, so the fallback is never stacked onto a busy VT either
4. Render the TUI on the configured TTY
5. On submit, fork the session-leader child and show "Authenticating..."
   while it runs PAM in the background; a failure comes back inline (the
   terminal is never torn down between attempts)
6. In the child, exactly like login(1): authenticate, resolve the account,
   set `PAM_TTY`/`XDG_VTNR`/`XDG_SEAT`/`XDG_SESSION_TYPE`/
   `XDG_SESSION_DESKTOP`, then open the PAM session after `setsid()`: logind
   binds the session to the right seat/VT and relocates the child into its own
   `session-N.scope`, and a keyring module in the stack (if any) unlocks the
   keyring with the just-typed password and publishes its agent variables into
   the PAM environment
7. The child parks once authorized; the greeter tears down and clears the
   terminal, then releases it
8. In the child: fork again; set environment, initialize groups, drop UID/GID,
   chdir HOME, and exec the configured Wayland session; wait for it and close
   the PAM session. Stopping `rsdm.service` cannot reach this child, so a
   restart never logs the live session out
9. The greeter waits for the child's final report, reclaims the VT foreground
   and redraws; on a fatal error it hands the TTY to the fallback login (the
   configured command first, then a built-in `agetty`/`login` chain) instead
   of dying

When `session_manager.enabled` is set, step 9 execs the compositor wrapped in
`rsdm session start -- <compositor>` instead of directly. That supervisor
exports the Wayland/XDG environment (including the keyring agent variables
inherited from step 7) to `systemctl --user` and D-Bus, and then watches who owns
the graphical session: a
compositor that brings up `graphical-session.target` itself (niri's
`niri.service`, sway, ...) is left to manage and tear down its own session, while
a bare compositor that does not gets `graphical-session.target` and
`xdg-desktop-autostart.target` anchored by rsdm. Either way the compositor and
`rsdm app` launches run as transient user units, and rsdm tears down only what it
brought up when the compositor exits.

Locker (`lock`) runtime path:

1. Load config and resolve the seated user, theme, ASCII title, and wallpaper
2. Bind `ext-session-lock-v1`, `fractional-scale-v1`, and `viewporter`; select
   the configured primary output (or the largest current mode), and create one
   lock surface per active output
3. Render a physical-pixel shm buffer per output. The primary receives the full
   interactive scene; secondary outputs receive background-only or black. niri
   `off` outputs are disabled through IPC and restored by the locker guard
4. Verify typed passwords through the `rsdm-lock` PAM service (auth only),
   rate limited
5. On success send `unlock`, round-trip so the compositor acknowledges it, then
   exit (an unflushed unlock leaves the compositor stuck on an abandoned lock)

Idle (`idle`) runtime path:

1. Bind the compositor's idle notifier to the first seat with the configured
   timeout and inhibitor policy
2. On `idled`, refuse duplicate cycles or an already-active rsdm lock, then
   spawn the current executable as `rsdm lock`
3. Observe the verified runtime state written only after compositor lock
   confirmation, then run `on_lock`
4. Wait for authentication or emergency unlock, run `on_unlock`, and re-arm for
   the next compositor idle event
