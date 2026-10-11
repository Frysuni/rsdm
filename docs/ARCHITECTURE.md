# Architecture

The production tree is intentionally narrow:

- `rsdm-core`: domain types, ports, config validation, login/runtime use cases
- `rsdm-infra`: PAM, Unix user resolution, session discovery, state storage,
  privilege dropping, and the DM session launcher
- `rsdm-system`: shared Linux process handles, monotonic clocks, generation
  identifiers, and logind transport; it contains no Session Manager policy
- `rsdm-session-manager`: independent systemd `--user` session coordination,
  application registration, shutdown, recovery, and optional libSM/libICE XSMP;
  it depends on Core and System, without PAM, DM, Greeter, or Lock dependencies
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
  status, session commands, apps, and config validation; static terminal reports
  live in its `output` module

CLI reports use ratatui widgets to lay out an in-memory buffer, then write rows
and SGR styles to the current terminal. They never enter an alternate screen,
change input mode or run an event loop. Each destination checks its own terminal
width and selects plain output for redirects, `NO_COLOR`, `TERM=dumb` or very
narrow terminals. The Greeter's ratatui backend and Lock's Wayland rendering are
separate. CLI diagnostic events remain in file/journal logs; their terminal
reports replace duplicate tracing lines while styling is active.

The CLI's `logs` module retains journalctl filters and access checks. Styled
terminals request newline-delimited JSON, decode text/binary fields and render
each record incrementally through `output/journal`. Runtime tracing events use
`output/events` with the same layout; span context and event fields are retained.
The CLI does not restyle file logs. Plain journal output delegates directly to
journalctl as before.

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
   over it, so the fallback is never stacked onto a busy VT either. Only after
   ownership is confirmed, restore a cooked line discipline before raw mode
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
   chdir HOME, and exec the configured Wayland session. The root leader keeps
   a running-session handle and shutdown delay FD while waiting, recovering
   recorded units as the desktop UID, and closing/ending PAM. Stopping
   `rsdm.service` cannot reach this child, so a
   restart never logs the live session out
9. The greeter waits for the child's final report, confirms the VT is free,
   reclaims its foreground and cooked baseline, and redraws; on a fatal error
   it hands the TTY to the fallback login (the
   configured command first, then a built-in `agetty`/`login` chain) instead
   of dying

Greeter power actions execute `rsdm logind-request` in a separate process.
Its bus executor exits with the helper, so a denied request leaves no runtime
threads in the Greeter before another PAM owner is forked.
On terminal handover, the CLI creates a runtime after the Greeter has returned
to query systemd's getty command; the fallback then replaces the process by exec.

Session, app, and power commands own their Tokio runtime at the CLI boundary.
The PAM owner creates its inhibitor runtime after fork; Lock power workers
create their own runtime while the Wayland event loop keeps running. Logind
capability queries, request timeouts, and shutdown notifications use Tokio.
Coordinator startup resolves its own PID through logind and checks the session
owner UID. An `XDG_SESSION_ID` hint must match that verified session; it cannot
select another login of the same user. Connection setup and session queries
share one five-second budget.
GNOME and Plasma shutdown delegation runs in an asynchronous worker under the
shared deadline; a failed or lost reply does not trigger a repeated request.
Session Manager clients, stop hooks, and recovery await typed D-Bus requests.
User-manager operations stay on the caller's runtime; compositor startup,
application launches, preparation, observation, and teardown use Tokio tasks.
Control endpoint registration and health checks run as Tokio tasks; dropping
the server aborts its pending reconnection task.
The workspace uses zbus's Tokio backend with its blocking API disabled.
Private bus fixtures use Tokio sockets and asynchronous setup. User-manager
jobs, reconnects, recovery leases, and shared deadlines use Tokio timers;
Session Manager has no direct async-io, async-channel, or futures-lite dependency.
Native session launchers use Tokio child processes. Cancelling an exit wait
retains the owned child and its pidfd for escalation inside the shared deadline.
XSMP keeps libSM/libICE on one dedicated thread with a current-thread runtime.
Tokio channels carry its commands, startup and completion. Socket readiness uses
AsyncFd registrations that restore blocking mode before any FFI call. Shutdown
interrupts owned socket duplicates and awaits completion inside the shared
deadline; dropping a handle never joins a possibly blocked thread.

When `session_manager.enabled` is set, step 8 wraps the original command in
`rsdm session start`, with the selected config and desktop metadata. Its user
coordinator owns one generation, separate from logind and desktop-entry IDs.
Typed D-Bus handlers feed a serial lifecycle actor. Start/stop jobs, app
preparation, control-bus maintenance and serialized finalize publication run in
workers. The actor uses Tokio channels and `select!` for requests, completions,
logind notices, termination signals, and periodic observation. SIGTERM/SIGINT use
Tokio signal streams after session exec. Reply delivery and recovery worker
draining share their original budgets through asynchronous waits.
Deadline revisions use Tokio watch so every pending operation observes both
shortening and revocation without retaining watchers after completion.
The actor, startup, recovery and app workers await recovery record reads and writes
on blocking filesystem tasks. A record lease, validation and mutation stay within
one task; D-Bus waits do not retain that lease. Target ownership reads run in
workers; the actor checks the current lifecycle and records ownership before
allowing anchor activation. Pending reads leave status, stop and cancel available.
Registration and recovery records precede side effects. InvocationID and pidfd
checks keep cleanup/signals attached to the recorded processes.
Executable lookup, procfs membership checks and XSMP record/credential files also
use blocking filesystem tasks. Cookie registration stays on the libICE thread;
unclaimed credential files are removed when a cancelled startup job completes.
Cloned user-manager connections share four slots for short requests, including
reference restoration. Job completion and application-exit waits release those
slots; preparation starts for every application before collecting worker results.

Bare compositors use a transient service and a per-generation anchor. The
official `niri-session` runs unchanged outside that service and owns its real
notify `niri.service`. GNOME and Plasma retain their native session managers.
RSDM publishes selected environment values, preserves native graphical-target
ownership and creates graphical/autostart targets only for its managed lifecycle.

Public Session Manager commands receive their configuration explicitly. When
disabled at command entry, app and session start run the original command with
its inherited environment and exit status. Finalize, stop hooks and recovery do
nothing; status, stop and cancel report disabled without consulting a bus or
runtime record. CLI configuration reads run on blocking workers. DM power uses
logind directly; Lock reads the current policy on its power worker and also uses
logind when SM is disabled, even with an old generation in its environment.

The configuration loader provides `watch_session_config`, a typed Tokio watch
receiver for session policy. It reopens the path every 250 ms on blocking
workers, so in-place edits, atomic replacement and recreation are observed.
Invalid or missing updates retain the last validated policy. Unchanged session
policy does not notify receivers; dropping all receivers ends monitoring.
Updates retain a disable counter, so a receiver that skips intermediate values
still observes a validated false-to-true transition as disabling its generation.

CLI start, stop hooks and recovery consume these updates. A disabled generation
closes control admission and cancels pending manager calls and preparation. It
persists its disabled marker, releases the shutdown inhibitor and interrupts the
private XSMP worker. Already accepted external requests cannot be recalled.
It leaves application units, WM, anchor and exported environment running.
The launcher retains its child and compositor pidfd independently of coordinator
workers and passively waits for the actual WM exit before returning to DM.
App stop commands preserve the coordinator's configuration path in their argv.
Before closing XSMP peers, disabling cancels their pending saves and interactions.
Worker shutdown allows up to 100 ms for this cleanup, then interrupts stalled
sockets within the same 500 ms completion budget; the caller's deadline can
shorten either wait. It never sends a new SaveYourself, phase 2 or Die after disable.

Each generation reserves a private one-byte retirement file before publishing
its session record. Disabling updates that file independently of atomic record
replacement, so denied directory writes cannot revive the old generation.
Record errors still allow worker draining, inhibitor release and XSMP shutdown.
Hooks check retirement before dispatch and while waiting, even if their own
configuration watcher missed the disable transition. Retirement is checked
before parsing a damaged or missing session record.

Recovery records can permanently mark a generation disabled. Once marked, app
stop hooks and recovery leave its applications, compositor, anchor and exported
environment untouched, even when the current configuration enables SM again.
An attempt to save that generation as enabled is rejected. Older records without
the retirement file retain their original recovery behavior and disabled field.

Orderly logout closes the launch gate, drains accepted jobs and prepares
registered `rsdm app` processes in parallel before infrastructure teardown.
Each app also has a synchronous ExecStop backstop sharing the saved deadline.
Optional libSM/libICE support implements authenticated local XSMP, phase-2
barriers, interaction, cancel and Die; native DE XSMP is preserved. Save replies
and helper success never replace actual cgroup exit. See
[session-manager.md](session-manager.md) for provider/policy boundaries.

Locker (`lock`) runtime path:

1. Load config and resolve the seated user, theme, ASCII title, and wallpaper
2. Bind `ext-session-lock-v1`, `fractional-scale-v1`, and `viewporter`; select
   the configured primary output (or the largest current mode), and create one
   lock surface per active output
3. Reuse one physical-pixel software canvas and at most two shm buffers per
   output, writing only released buffers and retaining redraw requests while
   both are busy. The primary receives the full
   interactive scene; secondary outputs receive background-only or black. niri
   `off` outputs use one bounded IPC worker, which retains connector restoration
   obligations even when the corresponding Wayland globals disappear
4. Verify typed passwords through the `rsdm-lock` PAM service (auth only),
   rate limited
5. On success send `unlock`, round-trip so the compositor acknowledges it, then
   exit (an unflushed unlock leaves the compositor stuck on an abandoned lock)

Lock power requests run in a separate worker while the Wayland event loop keeps
servicing opaque surfaces. Cancellation and request failure never invoke unlock.

Idle (`idle`) runtime path:

1. Bind the compositor's idle notifier to every advertised seat with the
   configured timeout and inhibitor policy, following seat additions/removals
2. Track `idled`/`resumed` per seat. When all connected seats are idle, refuse
   duplicate cycles or an already-active rsdm lock, then spawn the current
   executable as `rsdm lock`
3. Observe the verified runtime state written only after compositor lock
   confirmation, then run `on_lock`
4. Wait for authentication or emergency unlock, run `on_unlock`, and re-arm for
   the next compositor idle event
