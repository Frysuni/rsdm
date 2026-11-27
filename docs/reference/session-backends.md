# Session backends

The backend determines which lifecycle and application operations are available. Capabilities are probed for the current session, including ownership and protocol support.

See the [user guide](../session-manager.md) for setup and commands.

## Native WM and DE ownership

Native sessions retain ownership of their compositor and desktop dialogs. RSDM verifies the live provider before handing off logout or touching shared infrastructure.

::: details Implementation and constraints

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
server. Ordinary native-DE policies use verified native shutdown without a
preliminary SIGTERM; the native manager controls their save prompts and timeouts.
Explicit term/quit-command policies run an RSDM preflight first. With verified native
control, `auto` with `--on-timeout cancel` is rejected because RSDM cannot enforce
cancellation inside native dialogs. Native `partial` accepts this policy for
generic preparation. If native control becomes available before shutdown, RSDM
rejects preparation before closing apps or delegating instead of bypassing the
registered cancellation policy.

Before preparation, RSDM probes the native logout API using the same ownership
checks as StatusDetails. If it is unavailable or unknown, registered `auto`
applications use generic preparation too. Logout then returns `prepared` and
retains the WM, launcher and shared infrastructure. Reboot and poweroff use
logind after application preparation and authorization.

If native lifecycle control is unavailable during external shutdown, preparation
finishes without stopping desktop units, shared targets or the original launcher.
The coordinator closes and releases its inhibitor, while the owning `session start`
call continues passive observation until the original session process exits.
The same handoff applies after logind accepts poweroff or reboot in native `partial`.
PAM remains open during this wait, including if the completion record cannot be saved.
Native sessions use this ownership check before teardown after a stopped
graphical target or an exited launcher too. A lifecycle notification alone does
not authorize stopping infrastructure when native control is unverified.
Recovery also probes native ownership before infrastructure cleanup and launcher
termination. If control is unavailable or unknown, it cleans up registered app
invocations while preserving the desktop infrastructure and waiting for the
original launcher to exit.

`delegated` means the native manager accepted the request, not that logout
completed: it may still show a dialog, cancel or wait for an application. RSDM
keeps observing the lifecycle. Native API rejection leaves the compositor alive
and returns an error. A failed or uncertain native dispatch never starts generic
preparation or a second power request as a fallback.

An authoritative logind shutdown during native preflight or dispatch prepares
ordinary `auto` apps left for desktop dialogs within the existing shortened
budget. Earlier explicit preparation results and XSMP selections are retained;
persisted dispatch prevents another close. A late native or power reply cannot
clear that budget or resume ordinary operation. If logind revokes shutdown before
infrastructure stop, further forced preparation is cancelled, including while a
native reply is pending. The desktop still owns any native request already accepted.

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

:::

## Niri window close

Owned niri sessions can use authenticated JSON IPC to close registered application windows.

::: details Implementation and constraints

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

:::

## Sway window close

Owned Sway sessions can close verified native Wayland windows through Sway IPC.

::: details Implementation and constraints

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

:::

## Hyprland window close

Supported Hyprland Lua IPC exposes stable window identity and mapped process ownership. Older versions fall back to another shutdown method.

::: details Implementation and constraints

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

:::

## Wayfire discovery

Wayfire IPC provides read-only window discovery where its IPC plugins are available.

::: details Implementation and constraints

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

:::

## Wayland registry discovery

Generic toplevel discovery reports windows but does not authorize a close.

::: details Implementation and constraints

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

:::

## KWin standalone

Verified standalone KWin 6.4+ can close owned windows using its authorized Plasma window-management protocol.

::: details Implementation and constraints

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

:::

## XSMP

Default builds support local authenticated X Session Management Protocol clients, including compatible XWayland apps.

::: details Implementation and constraints

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
[installation guides](../README.md).

:::
