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

## External graphical sessions

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
remains untouched. Quit-command helpers bind to the attached coordinator's
verified invocation and run with the application's environment and working
directory. `session stop` prepares registered applications, then delegates logout
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
Power requests prepare applications before requesting logind; disabling releases
only the attached coordinator. Window-close also requires verified backend ownership.

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

`rsdm session status` reads the current coordinator without activating it. It
reports `inactive` when no coordinator is running and `disabled` when session
management is off. A manually started session can report `full` even when DM is
disabled; attaching to an external WM reports `partial`, with the specific
lifecycle limitations.

The authorized `org.rsdm.Session1.StatusDetails` method returns `enabled`,
`state`, the existing status fields in `status`, `capabilities`, `limitations`
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
unavailable or unknown, with the probe's reason in the limitations. The provider
and legacy status fields stay unchanged; optional window backends still do not
affect the manager's scope.

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

Preparation results include the application InvocationID, transaction, selected
method, dispatch state, `clean`/`forced`/`cancelled`/`unknown` result and escalation
flag. Retired app records remain visible for the current generation. `clean`
means no escalation was recorded; it does not certify that an app saved its data.
Escalation records distinguish a saved signal intent from a signal accepted by
the kernel. `escalated` confirms at least one accepted escalation signal; an
unconfirmed intent remains `unknown` after recovery. Older escalation flags
without that confirmation are also treated as uncertain.
The original Status and Stop wire formats remain unchanged. When an older
coordinator lacks StatusDetails, the CLI reads and displays its legacy Status
using the same existing unique bus owner. Authorization and other failures are
reported rather than masked by that fallback.

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

Registered services capture their original process before executing the app.
The internal executor verifies generation, invocation, MainPID and process
membership, then records PID and the kernel's stable pidfs inode ID. Exec keeps
that process identity and passes the original arguments unchanged. Disabled
executors run the command directly. Failed identity capture still allows the
command to run; kernels with anonymous pidfds and legacy records provide no
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

Before delegation, RSDM resolves the current unique bus owner and authenticates
its UID, broker-provided ProcessFD and logind session. It rechecks that owner and
login membership immediately before dispatch
on the same bus connection, and addresses the unique owner with `NoAutoStart`.
A changed or exited owner cannot redirect the request to a replacement desktop
service. When Plasma's on-demand `LogoutPrompt` helper has no owner, RSDM first
authenticates the live `ksmserver` in this login, explicitly activates the helper,
then pins its owner and rechecks both processes before submitting shutdown.
In full mode, a prompt user service without a logind session can act behind
that verified desktop; a prompt in another login is rejected. GNOME and
KSMServer without a verified login remain unavailable. A lost
activation or shutdown reply is not retried.

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

## Niri window close

For `auto`, Niri's direct JSON IPC follows quit-command and confirmed XSMP,
before TERM. `NIRI_SOCKET` is captured by finalize or native niri readiness and
stored with the generation. Bare niri uses the managed provider and can publish
both endpoints through `rsdm session start -- niri -- rsdm session finalize`.
The socket's UID and peer pidfd must match the recorded compositor unit's
MainPID and InvocationID. The selected window's PID must belong to the current
application invocation and cgroup; its pidfd stays pinned across a second
window snapshot and dispatch. A separate connection to the same pinned peer
subscribes to EventStream before selecting a window. Its initial WindowsChanged
snapshot supplies candidates; queued events are checked immediately before
close. A closed, replaced or missing target cannot be restored by a later event
in that burst. Disconnects, malformed or incomplete events and oversized bursts
prevent close. Neither connection waits for application exit. Titles and app
IDs do not establish ownership.

Only one addressed window receives close. Durable intent prevents another
close after a lost reply, cancellation or recovery; application exit still
requires an empty cgroup. Missing PID, unverified compositor ownership or an
unavailable transport leaves TERM available when no close was dispatched.
Attached compositors currently use fallback because their lifecycle ownership
does not establish this Niri proof. Events only validate the selected target;
they do not establish application exit or trigger additional closes.

## Sway window close

For `auto`, the direct binary IPC uses the generation's `SWAYSOCK`, captured
by finalize. Both command and window-event connections authenticate the same
compositor peer pidfd and recorded MainPID/InvocationID. GET_TREE traverses
both tiled and floating children. Only native `xdg_shell` windows with positive
PIDs can be considered; XWayland's claimed PID does not authorize a close.
Current application InvocationID, dedicated cgroup and a live pidfd are checked
again against a second tree before sending one `[con_id=…] kill` command.
Queued close, replacement or ownership-change events prevent that dispatch.

The shared durable ledger, cancellation, exit observation and TERM fallback
follow the same rules as Niri. A single rejected command allows fallback;
a lost or malformed response remains an unconfirmed dispatch. The isolated
Sway VM exercises GTK Wayland windows with the headless/pixman compositor,
save-dialog cancellation, foreign windows with the same app ID and an
individual stop. It also edits actual Kate and Mousepad documents, preserves
their unsaved text and save dialogs across repeated logout, and leaves foreign
editor instances and the compositor unchanged. Hardware rendering, XWayland and attached-compositor close
are outside that verification.

The separate media VM check runs actual OBS, Mumble and Waybar clients on
headless Sway with software graphics. An idle OBS window closes cleanly;
Mumble retains its Audio Tuning Wizard across cancelled and repeated logout;
Waybar's layer surface has no toplevel window and uses the owned TERM fallback.
Each case keeps a foreign instance and the compositor unchanged. The check
does not cover OBS recording or streaming, Mumble voice connections, or other
application settings.

## Hyprland window close

Finalize captures `HYPRLAND_INSTANCE_SIGNATURE` for the generation. IPC paths
are derived only from that signature and the recorded `XDG_RUNTIME_DIR`.
Command and event sockets authenticate the recorded compositor invocation;
each short-lived command connection is checked against its pinned peer.
Discovery requires the Lua API and native mapped clients with a positive PID
and `stableId`. Older address-only clients and XWayland are excluded.

The application invocation, cgroup and pidfd checks use the shared window-close
path. The compositor then checks stable ID, address, PID and native mapped state
and dispatches the addressed close in one synchronous Lua IPC call. A changed
target returns a specific rejection before dispatch; other errors or lost
responses remain unconfirmed. Window open/close events veto a changed address.
The manager sends no focus command and does not rewrite compositor settings.
Configurations without the Lua API retain TERM fallback.

The isolated VM exercises Hyprland 0.55.4 with a Lua configuration, software
rendering, GTK save-dialog cancellation, foreign windows with the same app ID
and individual clean stop. Other versions, hardware rendering, XWayland and
attached-compositor close are outside that verification.

## Wayfire discovery

Finalize captures `WAYFIRE_SOCKET` for the generation. Read-only discovery uses
Wayfire's length-prefixed JSON IPC directly, with bounded frames and deadlines.
The socket peer must belong to the current compositor MainPID and InvocationID.
`list-methods` checks the availability of `ipc-rules`, including when no windows
exist; `list-views` and `view-info` check the advertised read APIs.

Window close remains unavailable: reported PIDs may come from XWayland and
`close-view` accepts only a view ID without an atomic identity or owner guard.
Status distinguishes verified discovery, missing plugins and an unverified
compositor. Discovery does not change the session's full/partial scope, close
windows, enable plugins or edit Wayfire configuration. The existing TERM
fallback remains subject to each registered application's shutdown policy.

The isolated VM check uses Wayfire 0.10.1 with software rendering and GTK
Wayland windows. It covers discovery with and without `ipc-rules`, individual
TERM fallback, preservation of foreign windows and the owned compositor, and
unchanged configuration. This does not establish window-close support or
verification of other Wayfire versions, hardware rendering or XWayland.

## Wayland registry discovery

For owned compositors without a supported command transport, StatusDetails
queries the generation's recorded `WAYLAND_DISPLAY` and `XDG_RUNTIME_DIR`.
An absolute display socket is also accepted. The peer must match the session
UID, live pidfd, compositor MainPID and current InvocationID before and after
the query. Core registry discovery uses only `get_registry` and `sync`, bounded
by the IPC concurrency limit, a two-second deadline and a one-MiB event budget.

Status reports advertised `ext_foreign_toplevel_list_v1`,
`zwlr_foreign_toplevel_manager_v1` and `org_kde_plasma_window_management`
versions. Removed globals are excluded. Registry presence alone does not
establish window PID/cgroup ownership or an authenticated close handle;
generic window-close remains unavailable and TERM follows the registered app policy.
The registry query binds no foreign-toplevel, Plasma or River window-manager interface.
Titles and app IDs are not used to infer ownership. Unowned attached sessions
do not connect to the compositor for this probe.

The isolated check covers Labwc 0.20.0 and River 0.4.5 with software rendering.
Labwc additionally runs registered and foreign GTK Wayland windows to verify
policy TERM fallback without close requests or configuration changes. River
is checked for registry discovery only, without binding its exclusive window
manager API. These discovery checks do not establish hardware rendering,
XWayland or the full application compatibility matrix.

## KWin standalone

The owned KWin backend binds `org_kde_plasma_window_management` v17 and obtains
each window by UUID. It keeps the resulting resource on the same connection,
never reuses object IDs, reads PID and parent after `initial_state`, and prefers
a window without a known parent. Close targets that live resource, without
activating windows or changing focus, desktops, shortcuts or compositor settings.
Queued unmap, PID changes, missing state and incomplete frames revoke authorization.
The existing invocation, cgroup, process handle and durable dispatch checks apply;
cancellation or a lost reply does not authorize another close for the invocation.

The D-Bus owner of `org.kde.KWin` must have a broker-provided ProcessFD matching
the Wayland socket peer. A read of `supportInformation` to that unique owner,
without service activation, establishes KWin 6.x starting with 6.4. Earlier versions use
claimed X11 PIDs and cannot authorize this backend. Current KWin obtains native
Wayland PIDs from client credentials and XWayland PIDs through XRes; a forged
`_NET_WM_PID` does not grant ownership.
See the PID implementations in [KWin 6.3](https://github.com/KDE/kwin/blob/v6.3.0/src/x11window.cpp)
and [KWin 6.4](https://github.com/KDE/kwin/blob/v6.4.0/src/x11window.cpp), and the
[Plasma window protocol](https://github.com/KDE/plasma-wayland-protocols/blob/master/src/protocols/plasma-window-management.xml).

KWin's normal interface authorization needs the installed
`packaging/applications/org.rsdm.SessionManager.desktop`, with an `Exec` path
matching the installed executable. The Nix source package installs it using its
store path; Arch packaging uses `/usr/bin/rsdm`. Other installations should copy
it into the application directory and adjust `Exec` to the binary's absolute path.
KService must also have a working standard XDG application catalog. A missing
protocol, permission, version proof or compositor invocation reports a limitation
and leaves fallback to app policy. GNOME/Plasma native providers retain desktop
delegation instead of using this backend.

The isolated KWin check uses virtual software rendering, registered and foreign
GTK windows on Wayland and XWayland, cancellation and repeat logout, save dialogs,
and clean individual exit. It also forges a foreign X11 window's claimed PID.
These checks do not establish hardware rendering, other KWin versions or the full
application compatibility matrix.

## XSMP

Official builds enable `xsmp`. Managed WM and native niri sessions publish
`SESSION_MANAGER` and `ICEAUTHORITY` before app launch. Local Unix ICE connections
use private cookie authentication. Clients must belong to a registered app's
verified unit invocation; claimed `SmProcessID` is not ownership evidence.
The server pins the connecting process through `SO_PEERPIDFD`. Before each
shutdown save, phase 2, interaction grant and `Die`, it acquires an IPC request
slot and rechecks the live process, current unit invocation, cgroup membership
excluding `ControlPID`, generation record, transaction and deadlines. Force
does not bypass these checks. Revoked ownership releases an outstanding save
with `ShutdownCancelled` and prevents further shutdown messages on that peer.
Listener creation, accept, protocol processing and callback replies, local
checkpoints and cancellation share the same four IPC slots as user-manager
requests. Cancellation of an existing save remains allowed after disable or
deadline expiry within the worker's 100 ms cleanup grace. Saturation or stalled
I/O may prevent delivery before disconnection. Disconnect disables
[ICE shutdown negotiation](https://xorg.freedesktop.org/archive/current/doc/libICE/ICElib.html).
The dedicated Tokio thread awaits socket readiness. Stop interrupts ICE I/O
during admission and awaits completion inside the session deadline without join.

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
and activates the anchor once the provider is ready. Repeating finalize in a
running session updates the environment without recreating its anchor. Automatic
managed readiness observes a newly published display or an already active graphical target;
native providers must also confirm their own readiness. Stale display
assignments are cleared before launch without inventing addresses. For managed
sessions, a readiness timeout keeps owned targets inactive; a late explicit
finalize can still activate them.

Finalize publications run serially in a worker, so their D-Bus calls do not
block status or cancellation. The coordinator saves ownership before publication.
Shutdown rejects queued publications and waits for the active one within the
shared shutdown deadline before preparing applications. Cancelling shutdown
preserves an accepted startup finalize until readiness is confirmed.

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

For dbus-broker, activation uses the user manager's environment. RSDM identifies
the bus executable through its D-Bus PID and updates that environment directly,
awaiting the reply before logout removes owned variables. Other bus implementations
receive the standard `UpdateActivationEnvironment` call.

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

## Control request limits

Each preparation persists a shutdown transaction ID, action, monotonic start,
absolute deadline and last result. An ordinary request reserves the largest
registered application timeout (30 seconds when no apps are registered), plus
90 seconds, measured from the start of preparation. Repeated requests reuse this
transaction and deadline; logind may shorten the budget. Recovery uses the saved
remaining budget rather than restarting it. Legacy records without a saved
deadline retain the existing five-second recovery fallback.

Before a graceful action, the application record stores its transaction ID,
InvocationID, method and dispatch intent. An unconfirmed reply preserves that
intent and prevents replay or a second graceful method. Only a proven unsent
request permits fallback. Cancellation preserves command and window-close
claims. XSMP records distinguish the save from `Die`: before `Die`, a separate
intent is persisted for the same transaction and InvocationID. A sent or
unconfirmed `Die` blocks another graceful close after cancellation or recovery.
A cancelled save can start again in a new transaction only when the record
proves that `Die` was not sent. Legacy records without this evidence remain
conservative; `quit_started=true` is treated as possibly dispatched. StatusDetails
reports an outstanding `Die` intent even when the preceding save was sent.

Application preparation records retain `clean`, `forced`, `cancelled` or
`unknown`, bound to the transaction and InvocationID. Before escalation, its
intent is persisted so recovery cannot later report a clean exit. `clean`
means the invocation exited without escalation; it does not guarantee saved
data. Unknown legacy dispatch remains `unknown`. Completed app records are
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

Admission allows 64 unfinished Launch/Finalize/Status requests. Stop, Cancel,
and XSMP preparation have independent limits of 4, 2, and 2 so launch traffic
cannot consume their slots. Excess calls receive D-Bus `LimitsExceeded` and
may be retried after outstanding work finishes. Slots remain held through
queued/worker work and replies, including when a caller disconnects. Stop
acknowledgements retain their slot until the coordinator receives them.

`org.rsdm.Session1.Handshake` returns the generation after checking the caller's
bus credentials, live process and logind session. A process without a logind
session must match a registered unit cgroup and its current InvocationID.
Environment hints alone cannot establish membership. Every control method
performs these checks before queueing its request; authorization allows four
concurrent queries, each within a five-second total budget. CLI app, finalize,
stop and cancel obtain their generation through the handshake rather than
trusting an environment token.

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
