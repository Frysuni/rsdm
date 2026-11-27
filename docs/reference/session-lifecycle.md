# Session lifecycle internals

Technical reference for ownership, shutdown transactions, recovery and diagnostic behavior.

See the [user guide](../session-manager.md) for setup and commands.

## External graphical sessions

Attach creates a partial coordinator within the existing login. Its environment and registered apps are private; the original desktop retains its lifecycle.

::: details Implementation and constraints

Use `rsdm session attach` inside an external graphical logind login to coordinate
newly registered applications while retaining its WM and PAM lifecycle. The first
`rsdm app` or `rsdm session finalize` also attaches when no coordinator exists.
The independent `rsdm-session-manager.service` runs the internal `session serve`
command; a private inherited socket authenticates the originating CLI process.
Concurrent callers join one coordinator, and a caller from another login is
rejected even when its environment contains the current generation.

Attached status uses provider `partial`. Application units do not require
`graphical-session.target` or an external anchor. Environment and a private XSMP
endpoint are scoped to new managed applications; the user's global environment
remains untouched. Quit-command helpers use the verified live coordinator or the
registered app unit after it stops, with the app's environment and working directory.
Before `session stop` prepares apps, RSDM probes scoped native logout. An available
peer keeps ordinary `auto` applications under the desktop's native dialogs;
explicit term/quit-command policies run preflight first. Logout then delegates
to live GNOME or Plasma peers verified in this logind session using the broker's
ProcessFD credentials. Numeric PID credentials alone cannot prove this ownership.
The desktop retains confirmation dialogs and completion. Discovery remains
cancellable; before the native call, the actor saves its dispatch phase and the
worker rechecks the transaction, peer owners, process handles, login membership,
gate and deadline.
Without an authenticated native peer, stop returns `prepared` and retains the
external WM, with the limitation in status and the stop result. Native peers in
user services without a provable logind session remain unavailable, including an
unstarted on-demand Plasma prompt. Attached lookup does not activate it.
If the peer disappears after native preparation was selected, stop reports a
failure and retains the external WM. It does not claim generic preparation or
start a second close path after uncertain native dispatch. An attached `auto`
policy with cancellation remains enforceable when generic preparation is selected;
available native dialogs require an explicit preflight policy instead.
Power requests prepare applications before requesting logind; disabling releases
only the attached coordinator. Window-close also requires verified backend ownership.

:::

## Orderly logout

Shutdown prepares registered apps before ending the verified owned session. Read-only status queries report capabilities, ownership and results.

::: details Implementation and constraints

`rsdm session status` reads the current coordinator without activating it. It
reports `inactive` when no coordinator is running and `disabled` when session
management is off. A manually started session can report `full` even when DM is
disabled; attaching to an external WM reports `partial`, with the specific
lifecycle limitations.

The authorized `org.rsdm.Session1.StatusDetails` method returns `enabled`,
`state`, the session summary in `status`, `capabilities`, `limitations`
and `preparations`. Capabilities have a name, state (`available`, `unavailable`,
`not-applicable` or `unknown`) and reason. A missing optional window-close
backend does not change a full session into partial. Window-close capability
probes authenticate the current IPC instance and read its window/event APIs;
they do not close windows. Probes run outside the coordinator actor with a
bounded timeout.
Native logout probes check the service's unique owner, UID, broker-provided
ProcessFD and membership in this generation's logind session.
For Plasma they also verify KSMServer and recognize an activatable LogoutPrompt
without starting it. Status queries do not request shutdown or show native dialogs;
an ownership failure is unavailable and a transport failure is unknown.

GNOME and Plasma report `partial` when this necessary lifecycle capability is
unavailable or unknown, with the probe's reason in the limitations. Optional window backends do not affect the manager's scope.

For a recorded compositor unit, status verifies that the captured InvocationID
is still active and remains unchanged through the query. A missing, stopped or
reused invocation also reports `partial`; a failed or timed-out query reports
unknown lifecycle capability with its reason. These probes only read state.
When a compositor unit is recorded, logout and recovery verify its invocation
before stopping shared graphical targets or the session anchor, and recheck it
inside each request slot before sending teardown commands. This also guards
reference release and environment cleanup. An unverified or replaced invocation
leaves that infrastructure untouched. Application cleanup likewise rechecks
its own invocation before sending StopUnit or releasing its reference.

Before logout stops RSDM infrastructure, the coordinator uses the same bounded
lifecycle probe as StatusDetails. Unavailable or unknown control returns
`prepared`, restores the running phase and retains the launcher and session
infrastructure. This probe remains cancellable; teardown begins only after the
actor accepts its result.

New application units also probe current lifecycle control. If it is unavailable
or unknown, they have no graphical target or anchor lifetime dependencies and
receive the generation's private environment, while retaining the original
provider identity. Existing units keep their passive dependencies.
Quit-command helpers also use the current lifecycle probe. When control is lost,
their lifetime depends on the registered app unit instead of the session anchor,
so an inactive or foreign graphical target does not block preparation. Attached
helpers use their verified live coordinator, or the registered app unit when
that coordinator is stopped or collected. A replaced or unverified coordinator
still rejects dispatch. The app invocation is
rechecked inside the request slot before starting any quit command.

Preparation results include the application InvocationID, transaction, selected
method, dispatch state, `clean`/`forced`/`cancelled`/`unknown` result and escalation
flag. Retired app records remain visible for the current generation. `clean`
means no escalation was recorded; it does not certify that an app saved its data.
Escalation records distinguish a saved signal intent from a signal accepted by
the kernel. `escalated` confirms at least one accepted escalation signal; an
unconfirmed intent remains `unknown` after recovery.
The CLI queries StatusDetails using the coordinator's existing unique bus
owner. Authorization and other failures are reported directly.

Update all RSDM components together and start a fresh graphical session after
upgrading. Recovery records must match the current format.

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
For a recorded compositor service, RSDM reports the terminal result of its
captured systemd InvocationID, including failures when the native launcher
exits successfully. A user-bus reference retains native service status through
cleanup; active management and detached observation use the same result checks.
Successful cleanup does not turn a compositor crash into a successful session.

:::

## Registering applications

Every managed launch has its own transient unit, ownership record and quit policy. RSDM observes verified process groups until they exit.

::: details Implementation and constraints

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

Registered services capture their original process before executing the app.
The internal executor verifies generation, invocation, MainPID and process
membership, then records PID and the kernel's stable pidfs inode ID. Exec keeps
that process identity and passes the original arguments unchanged. Disabled
executors run the command directly. Failed identity capture still allows the
command to run; kernels with anonymous pidfds provide no
original-process ID. This metadata does not itself authorize another cgroup.

If that original process moves into a transient systemd scope, RSDM can capture
the scope while the service invocation and original pidfd ID still match. Scope
members require verified kernel cgroup membership. When every member is the
original process or a live descendant, the whole scope is owned. A mixed scope
also records the pidfd IDs of its verified lineage; only those processes and
their live descendants can receive close requests or signals. Unrelated or
reparented peers do not gain ownership by sharing that scope. Exit observation
still waits for the whole scope to empty. The scope's InvocationID is retained
in the private app record and rechecked for window close, XSMP and signals.
RSDM observes both groups, including scope children after the service becomes
inactive. A changed scope invocation is an ownership error. Legacy records and unsupported pidfd
identities cannot adopt a scope; an uncaptured live original process prevents
reporting a clean exit after its service stops.

The isolated Flatpak check installs a local GTK fixture and runtime, using
Flatpak's actual sandbox, process scopes and D-Bus proxies. It verifies clean
Wayland close, preservation of a save dialog across repeated logout, and an
unchanged foreign instance with the same app ID. The runtime mounts the Nix
store read-only for its test dependencies. Other Flatpak applications and
runtimes remain outside that verification. A process whose ancestry vanished
before capture remains unowned; its continued scope membership prevents a
clean exit and does not authorize a forced signal.

With unified cgroups and stable pidfs IDs, a private lifetime observer remains
in the launch service. It keeps that service active if the original process exits
while its scope children are still running, preventing an automatic ExecStop
from requesting their shutdown. The observer pins cgroup files and is excluded
from application ownership, process counts and signals. After hot disable it
only observes process exit; it neither writes recovery records nor dispatches
shutdown actions. Its own exit does not mean that application data was saved.

In a managed WM or native niri session, auto chooses an explicit quit command,
a connected XSMP client, a verified window close, then SIGTERM to the main process. Remaining children
are tracked through the service cgroup and any authenticated migrated scope. Force timeout gives them up to five
seconds of SIGTERM grace, then SIGKILL. That grace is additional to the app
timeout. Apps prepare in parallel, so logout does not wait one full timeout per
app in sequence. Policies apply to that launch; relaunch an app to change them.

Quit commands use argv parsing without a shell or variable expansion. Use the
application's documented quit method where available. Helper success,
`SaveYourselfDone`, window disappearance and main PID exit do not prove app exit.
RSDM waits for the app's verified cgroups. Arguments after `--` are passed through unchanged;
any shell expansion happens before RSDM receives them. Quote `'$HOME'` in a shell
to pass those characters literally.

`example` and `examplectl` above are placeholders. Replace them with the app and
its documented quit command. The quit helper runs with that app's recorded
environment and working directory.

The registered InvocationID is checked before and after those property reads,
then again in the final IPC slot before `StartTransientUnit`. Force does not
bypass these ownership checks. A rejected pre-dispatch check records the
command as `not_sent` without stopping or unreferencing a helper unit. Once
start may have been accepted, an unconfirmed reply retains dispatch intent
and prevents another quit-command start.

Each launch creates a transient service in `app-graphical.slice`, named
`app-rsdm-<application>@<generation-and-instance>.service`. Stable application
identity supports portal permissions. Registration precedes unit creation, so
shutdown includes accepted launches with pending start jobs. Startup launches
wait up to ten seconds for readiness. Shutdown rejects new launch/finalize
requests; stale generation tokens fail rather than targeting a later login.

Only `rsdm app` launches enter this registry. Menus, autostart entries and arbitrary UID processes are not automatically wrapped. The CLI attaches to a compatible external graphical session when needed; disabled coordination runs the command directly.

Launch the first instance through `rsdm app`. A launcher that forwards to an
already running instance or a separately activated service does not move that
process into the registered unit. Programs outside that unit are not covered
by its shutdown policy.

:::

## Readiness and recovery

Finalize publishes live display endpoints. Recovery preserves ownership and uses bounded deadlines; it does not sweep unrelated processes.

::: details Implementation and constraints

RSDM publishes existing `PATH`, `LANG`, `XDG_RUNTIME_DIR`, validated session
identity, seat/VT data and its session-management endpoint. Other PAM values
reach the compositor; export extra values with finalize or `extra_env`. Keyrings
are unlocked by DM's PAM login before the coordinator; see [keyrings.md](../keyrings.md).

After the compositor's display becomes available:

```sh
rsdm session finalize
rsdm session finalize MY_VAR ANOTHER_VAR
```

Finalize exports live `WAYLAND_DISPLAY`, `DISPLAY` and `XAUTHORITY` when present
and activates the anchor once the provider is ready. Repeating finalize in a
running session updates the environment without recreating its anchor. Automatic
managed readiness observes a newly published display or an already active graphical target;
native providers must also confirm their own readiness. Stale display
assignments are cleared before launch without inventing addresses. For managed
sessions, a readiness timeout keeps owned targets inactive; a late explicit
finalize can still activate them.

Finalize publications run serially in a worker, so their D-Bus calls do not
block status or cancellation. The coordinator saves ownership before publication.
Automatic niri readiness retains values from an active finalize publication while
discovering the remaining display endpoints from the user manager.
Shutdown rejects queued publications and waits for the active one within the
shared shutdown deadline before preparing applications. Cancelling shutdown
preserves an accepted startup finalize until readiness is confirmed.
Activation completions apply that cancellation before advancing readiness. An
activation skipped during logout can then retry, rather than waiting for an
anchor that was never started.
Readiness observations also apply cancellation before completion. Snapshots taken
during shutdown preparation are discarded after resuming, so readiness is
checked against the running session again.

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

If recovery cannot confirm the launcher's exit, the owning start call releases
the coordinator and inhibitor but keeps PAM open through passive observation.
It retains the original process handle or child when an invocation is replaced,
a stop fails or an unfinished worker still holds the session process. Pending
workers cannot issue new management actions after this handoff. An unsubmitted
managed command with no child needs no launcher wait. The original coordinator
error or nonzero result is preserved after the wait; a clean launcher exit does
not turn failed recovery into success.

User-manager reload and reexec, including the reexec performed by a NixOS
configuration switch, do not request logout. Temporary D-Bus read failures defer
observation and readiness checks. If a start or stop reply is lost, RSDM checks
the resulting unit state without sending the command again.

Temporary failures reading graphical-target ownership or discovering niri's
environment preserve the original startup deadline and leave accepted finalize
requests pending until a later check can complete.
Activation preparation retries reads only when the start request is known not
to have been submitted.

After a user-bus restart, the coordinator reconnects its manager and control
connections, restores references to matching owned invocations and republishes
matching activation-environment values. A private runtime lock prevents another
coordinator or recovery helper from replacing the live session while its D-Bus
name is unavailable. Applications and native desktop services still need to
handle their own disconnected D-Bus connections; RSDM cannot reconnect them.

For dbus-broker, activation uses the user manager's environment. RSDM identifies
the bus executable through its D-Bus PID and updates that environment directly,
awaiting the reply before logout removes owned variables. Other bus implementations
receive the standard `UpdateActivationEnvironment` call.

An environment update records both the proposed values and previous owned values
until publication succeeds. If publication fails or is interrupted, logout and
recovery can clear either value while preserving values changed by another owner.

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

Within a finite budget, application preparation ends early: up to two seconds
are reserved for escalation and teardown, capped at half the remaining budget
when the phase starts. Forced application shutdown and helper/target cleanup
each leave up to one second for the final compositor stop, with the same cap.
These phases share the original deadline and its revisions; they never create
a new budget after expiry. An exhausted graceful phase escalates using the
remaining time, while normal cancellable logout retains each app's policy.

Independently running application `ExecStop` hooks follow reductions of the
persisted deadline during connection setup, dispatch and exit waits. Once
systemd has accepted a unit stop, a later transaction or revoked session budget
does not extend that hook's remaining time.

Per-app metadata leases normally wait at most 250 ms for contention. During
shutdown/recovery their retries and sleeps also obey the shared deadline;
revisions or revocation affect an already waiting lease. The lease is released
before application exit waits or systemd jobs.
After a lock-file replacement, a waiter reacquires the current inode within
the same budget. Reaping checks the inode before removing a lock file, so an
obsolete descriptor cannot delete a newer application's lease.

Registration, session records and recovery filesystem operations run on blocking
tasks. The coordinator awaits each write before its dependent external action;
pending writes and contended app leases leave Tokio timers and I/O running.

Stopping a native/external launcher pins the original child with a pidfd.
Its TERM grace and post-KILL exit check share the shutdown deadline. Expiry
interrupts either wait and still permits immediate KILL of that owned child;
it does not start another blocking wait. An exit that cannot be confirmed is
reported as cleanup failure rather than successful completion.

Installing an updated package does not replace a running coordinator. When
upgrading from an affected version, end the old session before applying a live
configuration switch, then start the updated DM and a new session. A reboot into
the updated configuration also starts the new coordinator.

:::

## Power and PAM lifetime

Power actions use authorized logind or native desktop requests. The session's PAM owner and inhibitors stay alive through bounded cleanup.

::: details Implementation and constraints

Managed reboot/poweroff checks logind authorization, prepares apps, then requests
power as the user. Rejection keeps the compositor/services alive; closed apps
cannot be restored. Authorization and the single power request share the
coordinator's shutdown deadline, including system-bus setup and pending replies;
a timeout does not replay the request. Native DE power delegates once to its
manager. Suspend and
hibernate do not prepare logout. Greeter power actions call logind directly.

In an interactive terminal, `rsdm power reboot` and `rsdm power poweroff` show
an animated shutdown monitor. It reports application preparation, observed quit
methods, per-app timeout policies, elapsed waits, shutdown inhibitors with their
owners and reasons, and queued systemd jobs. Running jobs are executing; waiting
jobs await systemd ordering. The stage bar covers applications, the power request
and system services; it does not estimate total shutdown time. Counts cover only
observed work: jobs that leave the queue are not assumed successful, and jobs
shorter than the polling interval may be missed.

The monitor reads status without acquiring an inhibitor or changing the shutdown
policy. Ctrl+C stops watching; a dispatched power request may still continue.
The last snapshot stays in terminal scrollback. Monitoring ends when the terminal
closes, status becomes unavailable, or shutdown is no longer reported as active.
If no active shutdown appears within five seconds after handoff, the command
returns and points to desktop dialogs or the system console. A terminal inside
the graphical session cannot display later stages after that session closes.
Redirected output, `NO_COLOR`, and dumb terminals retain the one-shot output.

The coordinator holds a delay inhibitor and listens for `PrepareForShutdown`.
External shutdown cannot be cancelled by user requests, but logind can revoke it
while preparation is running. Its published delay budget limits app timeouts,
with 250 ms reserved. The monitor records an absolute monotonic deadline when
it receives the notice; time spent in the coordinator queue consumes that budget.
RSDM does not increase global timeouts or polkit privileges.
If a user inhibitor is denied, notifications remain active and RSDM warns that
it depends on the PAM owner's separate guard. Finite OS delay cannot guarantee
saving through hung applications, storage or an unresponsive bus.

DM forks the session child behind a start gate, then attempts to acquire the
root delay inhibitor before allowing any user-session setup or execution.
System-bus connection and inhibitor acquisition share one five-second timeout.
The child is a fresh exec of the installed rsdm image. Its bounded sealed
memfd request carries only the validated session command, identity, wrapper,
and environment; the helper then performs groups, credentials, environment,
directory setup, and exec after the fresh image has loaded. The gate uses only
close/read operations and never starts a bus executor before fork. Parent
failure closes the gate and aborts the waiting child.
If logind cannot grant the root inhibitor, RSDM logs the failure and retains the
existing fallback behavior; the user coordinator still attempts its own guard.
DM's detached root owner keeps a granted delay FD through user cleanup, PAM
close and PAM end. Restarting DM does not terminate that leader's graphical session.
Recovery installs its shared remaining budget before connecting to the user bus;
authentication, validation, the coordinator-name lease, app preparation, and
unit teardown use the same deadline. A saved or active deadline is never
extended by recovery. A failed coordinator also drains its accepted workers
within that budget; unconfirmed jobs keep their recovery ownership records.
Root bounds its helper wait separately to 90 seconds, reserving the last second
for SIGKILL and nonblocking exit checks. An unconfirmed exit remains a cleanup
failure; root does not wait indefinitely after escalation. Lock handles
power requests asynchronously and retains opaque surfaces through rejection or
cancellation; only authentication or privileged emergency unlock releases them.

:::

## Control request limits

D-Bus calls, worker jobs and recovery share bounded request slots and deadlines.

::: details Implementation and constraints

Each preparation persists a shutdown transaction ID, action, monotonic start,
absolute deadline and last result. An ordinary request reserves the largest
registered application timeout (30 seconds when no apps are registered), plus
90 seconds, measured from the start of preparation. Repeated requests reuse this
transaction and deadline; logind may shorten the budget. Recovery uses the saved
remaining budget rather than restarting it. Recovery outside an active shutdown
has a five-second budget.

Before a graceful action, the application record stores its transaction ID,
InvocationID, method and dispatch intent. An unconfirmed reply preserves that
intent and prevents replay or a second graceful method. Only a proven unsent
request permits fallback. Cancellation preserves command and window-close
claims. XSMP records distinguish the save from `Die`: before `Die`, a separate
intent is persisted for the same transaction and InvocationID. A sent or
unconfirmed `Die` blocks another graceful close after cancellation or recovery.
A cancelled save can start again in a new transaction only when the record
proves that `Die` was not sent. StatusDetails reports an outstanding `Die` intent
even when the preceding save was sent.

Application preparation records retain `clean`, `forced`, `cancelled` or
`unknown`, bound to the transaction and InvocationID. Before escalation, its
intent is persisted so recovery cannot later report a clean exit. `clean`
means the invocation exited without escalation; it does not guarantee saved
data. Completed app records are
archived privately under the generation's `results` directory before removal.
Reaping and cleanup take the result writer's record lease and recheck the
InvocationID before archival, so stale cleanup cannot remove a replacement
record or discard a result written while cleanup waited.

The user-bus API accepts at most 4096 application arguments, 1024 environment
entries, and 64 KiB per string. Quit commands allow up to 256 arguments and
64 KiB of encoded data. Each complete Launch/Finalize/XSMP request body is
limited to 256 KiB, including D-Bus encoding overhead. Oversized or NUL-containing
payloads are rejected before coordinator admission or recovery-record changes.
The CLI checks launch/finalize payloads before sending them.

Admission allows 64 unfinished Launch/Finalize/Attach/StatusDetails requests. Stop, Cancel,
and XSMP preparation have independent limits of 4, 2, and 2 so launch traffic
cannot consume their slots. Excess calls receive D-Bus `LimitsExceeded` and
may be retried after outstanding work finishes. Slots remain held through
queued/worker work and replies, including when a caller disconnects. Stop
acknowledgements retain their slot until the coordinator receives them.

XSMP stop hooks retry explicit admission refusals within the existing request
timeout and shared shutdown deadline. Accepted requests and ambiguous transport
failures are never replayed.

`org.rsdm.Session1.Handshake` returns the generation after checking the caller's
bus credentials, live process and logind session. A process without a logind
session must belong to a live user service or scope, with pinned process and
cgroup membership and a stable InvocationID. Shared user units follow logind's
primary graphical login (`org.freedesktop.login1.User.Display`); when it differs,
only explicitly registered application or compositor invocations are accepted.
This supports compositor-created scopes and independent desktop launcher services
without treating another actual tty login as part of the desktop. Environment
hints alone cannot establish membership. Every control method
performs these checks before queueing its request; authorization allows four
concurrent queries, each within a five-second total budget. CLI app, finalize,
stop and cancel obtain their generation through the handshake rather than
trusting an environment token.

:::

## Configuration and diagnostics

Validated live policy updates can detach an active generation. A fresh session is required to resume full coordination.

::: details Implementation and constraints

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

Disable coordination with `enabled = false`. A running CLI session observes
validated file updates, including atomic replacement. Disabling detaches the
coordinator and its private XSMP server without stopping applications or the WM;
the launcher still waits for the WM to exit before releasing its PAM session.
GNOME/Plasma observation uses the Shell/KWin process rather than the desktop
session manager. Once pinned, its process handle survives loss of the bus name.
This requires an identifiable compositor on the user bus. A detached WM that
never exposes its native name cannot be tracked through this path; the original
launcher remains the fallback. Buses without process-FD credentials also retain
a numeric-PID reuse gap before the handle is opened.
Re-enabling does not resume that generation's hooks or recovery. A request
already accepted by another service may still complete.
Pending XSMP saves and dialogs receive `ShutdownCancelled` before disconnection,
as defined by the [XSMP protocol](https://xorg.freedesktop.org/archive/X11R7.7/doc/libSM/xsmp.html).
Unresponsive peers are interrupted within the existing worker shutdown budget;
cancellation delivery cannot be confirmed for a broken or stalled connection.
The generation reserves a separate retirement file before startup. It keeps old
hooks passive when replacing `session.toml` fails, provided the retirement file
can still be updated. Storage errors do not bypass infrastructure shutdown or
allow the launcher to return before the WM exits.

Alternatively, set
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

:::
