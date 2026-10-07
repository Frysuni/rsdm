# Session manager

RSDM coordinates application shutdown with the graphical session. During an
orderly logout, it asks registered apps to quit and keeps the display available
while they save and exit. This can reduce unclean-shutdown warnings on the next
launch. GNOME and Plasma retain their native session manager and shutdown dialogs.

With `session_manager.enabled = true` (the default), DM wraps the selected
session's original command in `rsdm session start`, supplying the config path
and desktop metadata. The coordinator runs as the desktop user. Greeter owns
login UI; Lock owns the Wayland session lock.

Coordination requires Linux pidfd support, systemd 250+, logind and a user bus.
Only one graphical session per UID is supported because the bus, activation
environment and graphical targets are shared. A second coordinator is rejected
before shared state changes. Disabling the wrapper preserves PAM/keyring login.

## Quick start

Log in through RSDM with session management enabled. There is no need to run
`rsdm session start` again inside the desktop. From a terminal in that session:

```sh
rsdm app -- foot
rsdm session status
```

`rsdm app` registers and launches the program, then returns after startup; it
does not wait for the program to exit. Status should show the session as
`running` and list the registered app. To register more applications, replace
their launch commands in WM bindings or autostart with `rsdm app -- <program>`.
See the [niri](compositors/niri.md) and [Hyprland](compositors/hyprland.md) examples.

Use `rsdm session stop` for logout, and `rsdm power reboot` or
`rsdm power poweroff` for power actions. For an app that needs more time in a
managed WM or niri session, launch it with a policy such as:

```sh
rsdm app --shutdown-timeout 60 --on-timeout cancel -- foot
```

If that app is still running after 60 seconds, RSDM cancels logout and keeps
the desktop alive. Choose the app's documented quit command when available;
SIGTERM alone does not guarantee that it saves. Native DE policy differences
are described [below](#native-wm-and-de-ownership).

## Orderly logout

Use these commands from the graphical session:

```sh
rsdm session stop
rsdm session cancel
rsdm session status
rsdm power reboot
rsdm power poweroff
```

Stop closes application admission, drains accepted launches, requests quit in
parallel and waits for actual application processes to exit. It then stops the
owned graphical services and compositor. Status shows provider, phase, logind
identity, generation, XSMP availability and registered application policies.

An application timeout or `cancel` can cancel preparation while the compositor
and services are alive. Closed apps are not restarted, and accepted quit requests
cannot be recalled. Cancellation is unavailable once infrastructure teardown
starts. Cancelled/failed shutdown returns a nonzero CLI status; forced apps are
reported separately. Successful teardown does not prove every app saved.

Bind logout to `rsdm session stop`, rather than directly quitting the compositor.
For example, in Hyprland:

```ini
bind = SUPER SHIFT, E, exec, rsdm session stop
```

For niri:

```kdl
binds {
    Super+Shift+E { spawn "rsdm" "session" "stop"; }
}
```

RSDM does not rewrite WM configs. Direct compositor quit, compositor crash,
SIGKILL and power loss can remove the display before applications finish saving.

## Registering applications

```sh
rsdm app -- foot
rsdm app --shutdown-timeout 60 --on-timeout cancel -- editor document.txt
rsdm app --quit-command 'examplectl "quit now"' -- example
rsdm app --shutdown-method term -- example
rsdm app --shutdown-method xsmp -- example
```

| Option | Default | Effect |
| --- | --- | --- |
| `--shutdown-timeout <seconds>` | `30` | Positive number of seconds to wait for the app to exit after requesting quit |
| `--on-timeout force\|cancel` | `force` | Force remaining processes to exit, or cancel preparation while the desktop is alive |
| `--shutdown-method auto\|term\|xsmp` | `auto` | Select the available quit method, use SIGTERM, or require an admitted XSMP connection at shutdown |
| `--quit-command '<command>'` | Unset | Run the app's quit command; requires `--shutdown-method auto` |

In a managed WM or native niri session, auto chooses an explicit quit command,
a connected XSMP client, then SIGTERM to the main process. Remaining children
are tracked through the service cgroup. Force timeout gives them up to five
seconds of SIGTERM grace, then SIGKILL. That grace is additional to the app
timeout. Apps prepare in parallel, so logout does not wait one full timeout per
app in sequence. Policies apply to that launch; relaunch an app to change them.

Quit commands use argv parsing without a shell or variable expansion. Use the
application's documented quit method where available. Helper success,
`SaveYourselfDone`, window disappearance and main PID exit do not prove app exit.
RSDM waits for the app cgroup. Arguments after `--` are passed through unchanged;
any shell expansion happens before RSDM receives them. Quote `'$HOME'` in a shell
to pass those characters literally.

`example` and `examplectl` above are placeholders. Replace them with the app and
its documented quit command. The quit helper runs with that app's recorded
environment and working directory.

Each launch creates a transient service in `app-graphical.slice`, named
`app-rsdm-<application>@<generation-and-instance>.service`. Stable application
identity supports portal permissions. Registration precedes unit creation, so
shutdown includes accepted launches with pending start jobs. Startup launches
wait up to ten seconds for readiness. Shutdown rejects new launch/finalize
requests; stale generation tokens fail rather than targeting a later login.

Only `rsdm app` launches enter this registry. Menus, autostart entries and arbitrary
UID processes are not automatically wrapped. Without an RSDM token, plain
`rsdm app -- program` retains the basic graphical-target launch path, but shutdown
policy options are unavailable.

Launch the first instance through `rsdm app`. A launcher that forwards to an
already running instance or a separately activated service does not move that
process into the registered unit. Programs outside that unit are not covered
by its shutdown policy.

## Native WM and DE ownership

| Session | Ownership |
| --- | --- |
| Bare WM, including bare `niri` | RSDM transient compositor service; app preparation before compositor stop |
| Official `niri-session` | Original wrapper and real notify `niri.service`; apps prepared before stopping its verified invocation |
| GNOME | Original Exec; native SessionManager owns confirmation, inhibitors and shutdown |
| Plasma | Original Exec; native LogoutPrompt owns confirmation and subsequent shutdown |
| Explicit external manager | Original Exec; observed native unit or existing graphical target, with optional logout command |

DesktopNames selects a GNOME/Plasma candidate; readiness requires its native
manager and an active native graphical target. RSDM does not start that target
on the manager's behalf. It preserves the DE's session-management and XSMP
server. Ordinary native-DE policies use native shutdown without a preliminary
SIGTERM; the native manager controls their save prompts and timeouts. Explicit
term/quit-command policies run an RSDM preflight first. Native `auto` with
`--on-timeout cancel` is rejected because RSDM cannot enforce cancellation inside
native dialogs.

`delegated` means the native manager accepted the request, not that logout
completed: it may still show a dialog, cancel or wait for an application. RSDM
keeps observing the lifecycle. Native API rejection leaves the compositor alive
and returns an error; normal native logout has no forced generic fallback.

Explicit startup overrides:

```sh
rsdm session start --mode managed -- compositor
rsdm session start --mode external --native-unit custom-session.service -- original-wrapper
rsdm session start --mode external --logout-command 'sessionctl logout' -- original-wrapper
```

Managed mode cannot observe a native unit. External mode requires its manager
to publish readiness and own the graphical lifecycle; RSDM invents no notify
readiness or compositor PID. A successfully detached launcher is observed through
its native unit or graphical target. A desktop entry already explicitly calling
`rsdm session start` gets no second wrapper. Prefer official `niri-session` when
using niri's systemd integration.

## XSMP

Official builds enable the `xsmp` Cargo feature. Managed WM and native niri
sessions publish `SESSION_MANAGER` and `ICEAUTHORITY` before app launch. The
server uses local Unix ICE connections and private cookie authentication.
Clients must belong to a registered app's verified unit invocation; a claimed
`SmProcessID` is not ownership evidence.

It implements save, serialized interaction, the shared phase-2 barrier,
cancellation and `Die`, while still waiting for actual process exit. Explicit
XSMP fails if the build/provider lacks support or the app has no admitted
connection. Auto falls back for unconnected apps. Client restart commands are
never executed, and native DE servers are not replaced.

Participation depends on the toolkit and display backend. RSDM does not force
Wayland apps into XWayland or change crash markers. There is no universal quit
protocol, so it cannot guarantee clean saves for every Chromium/Mumble backend
or every other application.

Custom builds can omit XSMP:

```sh
cargo build --release --locked -p rsdm --no-default-features
```

Status reports that limitation. Default builds require libSM/libICE development
packages; release binaries require their runtime libraries. See the
[installation guides](README.md).

## Readiness and recovery

RSDM publishes existing `PATH`, `LANG`, `XDG_RUNTIME_DIR`, validated session
identity, seat/VT data and its session-management endpoint. Other PAM values
reach the compositor; export extra values with finalize or `extra_env`. Keyrings
are unlocked by DM's PAM login before the coordinator; see [keyrings.md](keyrings.md).

After the compositor's display becomes available:

```sh
rsdm session finalize
rsdm session finalize MY_VAR ANOTHER_VAR
```

Finalize exports live `WAYLAND_DISPLAY`, `DISPLAY` and `XAUTHORITY` when present
and activates the anchor once the provider is ready. Automatic managed readiness
observes a newly published display or an already active graphical target;
native providers must also confirm their own readiness. Stale display
assignments are cleared before launch without inventing addresses. For managed
sessions, a readiness timeout keeps owned targets inactive; a late explicit
finalize can still activate them.

For an RSDM-owned lifecycle, the anchor activates `graphical-session.target` and
`xdg-desktop-autostart.target`. Ownership is recorded before activation. Native
targets stay under native control. Apps have a synchronous ExecStop backstop for
external unit/target stops, using the saved deadline and excluding the hook's
own process. A committed systemd stop cannot be cancelled.

Cleanup clears only matching exported values and retains `PATH`, `LANG` and
`XDG_RUNTIME_DIR` for unrelated services. Recovery uses recorded generation and
InvocationID, never KillUser or a UID process sweep. Root runs recovery after
changing to the session UID; user records and quit commands are not interpreted
with elevated privileges.

User-manager reload and reexec, including the reexec performed by a NixOS
configuration switch, do not request logout. Temporary D-Bus read failures defer
observation and readiness checks. If a start or stop reply is lost, RSDM checks
the resulting unit state without sending the command again.

After a user-bus restart, the coordinator reconnects its manager and control
connections, restores references to matching owned invocations and republishes
matching activation-environment values. A private runtime lock prevents another
coordinator or recovery helper from replacing the live session while its D-Bus
name is unavailable. Applications and native desktop services still need to
handle their own disconnected D-Bus connections; RSDM cannot reconnect them.

Each unit start/stop call has one timeout covering its preflight reads, signal
subscription, method reply, and completion checks. A lost reply or job signal is
reconciled against owned unit state inside the same budget, without repeating
the mutation or adding another verification timeout. An expired start budget
does not imply that an accepted service start was rolled back; recovery retains
its ownership record.

Shutdown D-Bus work shares the coordinator's monotonic hard deadline across
manager clones, including reads/reconnect and cleanup mutations. A revised
logind budget wakes pending calls to recompute their remaining time; revocation
removes the shared limit while preparation is still cancellable. After expiry,
new calls fail without sending a mutation. Already accepted systemd operations
can continue and remain represented by recovery records.

Per-app metadata leases normally wait at most 250 ms for contention. During
shutdown/recovery their retries and sleeps also obey the shared deadline;
revisions or revocation affect an already waiting lease. The lease is released
before application exit waits or systemd jobs.

Stopping a native/external launcher pins the original child with a pidfd.
Its TERM grace and post-KILL exit check share the shutdown deadline. Expiry
interrupts either wait and still permits immediate KILL of that owned child;
it does not start another blocking wait. An exit that cannot be confirmed is
reported as cleanup failure rather than successful completion.

Installing an updated package does not replace a running coordinator. When
upgrading from an affected version, end the old session before applying a live
configuration switch, then start the updated DM and a new session. A reboot into
the updated configuration also starts the new coordinator.

## Power and PAM lifetime

Managed reboot/poweroff checks logind authorization, prepares apps, then requests
power as the user. Rejection keeps the compositor/services alive; closed apps
cannot be restored. Authorization and the single power request share the
coordinator's shutdown deadline, including system-bus setup and pending replies;
a timeout does not replay the request. Native DE power delegates once to its
manager. Suspend and
hibernate do not prepare logout. Greeter power actions call logind directly.

The coordinator holds a delay inhibitor and listens for `PrepareForShutdown`.
External shutdown cannot be cancelled by user requests, but logind can revoke it
while preparation is running. Its published delay budget limits app timeouts,
with 250 ms reserved. RSDM does not increase global timeouts or polkit privileges.
If a user inhibitor is denied, notifications remain active and RSDM warns that
it depends on the PAM owner's separate guard. Finite OS delay cannot guarantee
saving through hung applications, storage or an unresponsive bus.

DM's detached root owner keeps its delay FD through user cleanup, PAM close and
PAM end. Restarting DM does not terminate that leader's graphical session.
Recovery installs its short shared budget before connecting to the user bus;
authentication, validation, the coordinator-name lease, app preparation, and
unit teardown use the same deadline. A saved or active deadline is never
extended by recovery. A failed coordinator also drains its accepted workers
within that budget; unconfirmed jobs keep their recovery ownership records.
Root bounds its helper wait separately. Lock handles
power requests asynchronously and retains opaque surfaces through rejection or
cancellation; only authentication or privileged emergency unlock releases them.

## Control request limits

The user-bus API accepts at most 4096 application arguments, 1024 environment
entries, and 64 KiB per string. Quit commands allow up to 256 arguments and
64 KiB of encoded data. Each complete Launch/Finalize/XSMP request body is
limited to 256 KiB, including D-Bus encoding overhead. Oversized or NUL-containing
payloads are rejected before coordinator admission or recovery-record changes.
The CLI checks launch/finalize payloads before sending them.

Admission allows 64 unfinished Launch/Finalize/Status requests. Stop, Cancel,
and XSMP preparation have independent limits of 4, 2, and 2 so launch traffic
cannot consume their slots. Excess calls receive D-Bus `LimitsExceeded` and
may be retried after outstanding work finishes. Slots remain held through
queued/worker work and replies, including when a caller disconnects. Stop
acknowledgements retain their slot until the coordinator receives them.

## Configuration and diagnostics

No WM/DE tables or app profiles are needed in `rsdm.toml`:

```toml
[session_manager]
enabled = true
extra_env = []
ready_timeout_secs = 10
```

The DM audit records `session finished` with the session child's exit status
before recovery starts. `session cleanup completed` or `session cleanup failed`
then reports recovery and PAM closure separately. The shutdown inhibitor remains
held until PAM is closed/dropped on all these paths.

`ready_timeout_secs` accepts zero or a positive number of seconds that fits a
monotonic deadline on the host platform. Configuration validation rejects
unrepresentable values before the session starts.

Disable coordination with `enabled = false`, or
`services.rsdm.sessionManager = false` on NixOS. Inspect `rsdm session status`,
`journalctl --user -b` and the DM journal for readiness/recovery. Enable graphical
services under `WantedBy=graphical-session.target`.

| Symptom | What to check |
| --- | --- |
| `this command requires an RSDM-coordinated session` | Run the command from the current graphical login with session management enabled, as that user |
| Status stays `starting` | Check the compositor's environment publication and provider readiness; use the finalize hook described above if needed |
| The app is absent from status | Start it through `rsdm app`; registration does not adopt an existing process outside its unit |
| `forced shutdown: <unit>` | The app did not exit in time; check its quit support, increase its timeout or choose `--on-timeout cancel` where supported |
| `delegated` after stop or power | The native DE owns completion; check its dialog for confirmation, cancellation or an app waiting to save |

Adapters follow upstream [GNOME SessionManager](https://github.com/GNOME/gnome-session/blob/master/gnome-session/org.gnome.SessionManager.xml),
[Plasma LogoutPrompt](https://github.com/KDE/plasma-workspace/blob/master/logout-greeter/org.kde.LogoutPrompt.xml)
and [XSMP](https://www.x.org/releases/current/doc/libSM/SMlib.html).
