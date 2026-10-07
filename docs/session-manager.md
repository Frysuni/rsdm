# Session manager

With `session_manager.enabled = true` (the default), DM wraps the selected
session's original command in `rsdm session start`, supplying the config path
and desktop metadata. The coordinator runs as the desktop user. Greeter owns
login UI; Lock owns the Wayland session lock.

Coordination requires Linux pidfd support, systemd 250+, logind and a user bus.
Only one graphical session per UID is supported because the bus, activation
environment and graphical targets are shared. A second coordinator is rejected
before shared state changes. Disabling the wrapper preserves PAM/keyring login.

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

Defaults: 30-second timeout, `--on-timeout force`, `--shutdown-method auto`.
In a managed WM, auto chooses an explicit quit command, a connected XSMP client,
then SIGTERM to the main process. Remaining children are tracked through the
service cgroup. Force timeout gives them up to five seconds of SIGTERM grace,
then SIGKILL. Deadlines run in parallel, rather than adding one timeout per app.

Quit commands use argv parsing without a shell or variable expansion. Use the
application's documented quit method where available. Helper success,
`SaveYourselfDone`, window disappearance and main PID exit do not prove app exit.
RSDM waits for the app cgroup. Arguments after `--`, including empty strings and
`$HOME`, remain literal.

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
server. Ordinary native-DE policies use native shutdown without a preliminary SIGTERM. Explicit
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

## Power and PAM lifetime

Managed reboot/poweroff checks logind authorization, prepares apps, then requests
power as the user. Rejection keeps the compositor/services alive; closed apps
cannot be restored. Native DE power delegates once to its manager. Suspend and
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
Recovery has a short app budget and root bounds its helper wait. Lock handles
power requests asynchronously and retains opaque surfaces through rejection or
cancellation; only authentication or privileged emergency unlock releases them.

## Configuration and diagnostics

No WM/DE tables or app profiles are needed in `rsdm.toml`:

```toml
[session_manager]
enabled = true
extra_env = []
ready_timeout_secs = 10
```

Disable coordination with `enabled = false`, or
`services.rsdm.sessionManager = false` on NixOS. Inspect `rsdm session status`,
`journalctl --user -b` and the DM journal for readiness/recovery. Enable graphical
services under `WantedBy=graphical-session.target`.

Adapters follow upstream [GNOME SessionManager](https://github.com/GNOME/gnome-session/blob/master/gnome-session/org.gnome.SessionManager.xml),
[Plasma LogoutPrompt](https://github.com/KDE/plasma-workspace/blob/master/logout-greeter/org.kde.LogoutPrompt.xml)
and [XSMP](https://www.x.org/releases/current/doc/libSM/SMlib.html).
