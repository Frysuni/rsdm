---
description: Manage Wayland session startup, registered applications, orderly logout and power actions.
---

# Session Manager

Session Manager gives registered applications time to close while the display is still available. It can own an entire graphical session or attach to one started elsewhere. It is enabled by default and requires Linux pidfds, systemd 250+, logind and the user D-Bus.

## Start a session

::: code-group

```sh [RSDM login]
# Check from the desktop:
rsdm session status
```

```sh [From VT]
rsdm session start -- niri-session
```

```sh [Existing login]
rsdm session attach
rsdm session status
```

:::

Use the existing desktop entry or compositor wrapper. DM automatically wraps the original command, so no additional start command is needed inside the desktop. Attach preserves the external compositor's lifecycle. The first app or finalize command also attaches if necessary. Only one coordinator per UID is supported.

::: details Startup modes
The default mode is auto. For a custom session, select managed or external explicitly.

```sh
rsdm session start --mode managed -- sway
rsdm session start --mode external --native-unit custom-session.service -- original-wrapper
rsdm session start --mode external --logout-command 'sessionctl logout' -- original-wrapper
```

The latter examples use placeholder commands and a unit that you must supply. External providers must establish their own readiness and own their graphical lifecycle. Managed mode cannot observe a native unit. Explicit native logout commands are literal argv, not shell expressions.
:::

## Register applications

Launch the first instance through RSDM:

```sh
rsdm app -- foot
rsdm app --shutdown-timeout 60 --on-timeout cancel -- editor document.txt
rsdm session status
```

Use your installed terminal/editor in these examples. The app command returns after launch. Replace normal commands in compositor bindings and autostart with rsdm app -- followed by the same program and arguments.

| Option | Default | Effect |
| --- | --- | --- |
| `--shutdown-timeout <seconds>` | 30 | Positive seconds to wait for exit; must fit systemd's limits |
| `--on-timeout force\|cancel` | force | Escalate after timeout, or cancel preparation |
| `--shutdown-method auto\|term\|xsmp` | auto | Pick a method, use SIGTERM, or require admitted XSMP |
| `--quit-command '<argv>'` | unset | Documented application quit command; requires auto |

For example, if an application provides a dedicated quit utility:

```sh
rsdm app --quit-command 'examplectl "quit now"' -- example
```

Replace example and examplectl with actual application commands. The helper uses the app's recorded environment and working directory. Arguments are parsed literally without a shell.

::: details How automatic shutdown chooses a method
For managed WMs and native niri, auto uses an explicit quit command, an admitted XSMP client, verified window close, then SIGTERM to the main process. Force gives remaining owned processes up to five additional seconds of SIGTERM grace before SIGKILL. Applications prepare in parallel. Explicit xsmp requires an admitted client at shutdown.

GNOME and Plasma normally use their own verified native dialogs for auto applications. Explicit term/quit-command policies run before delegation. Auto with cancel is rejected when verified native control is available, since RSDM cannot enforce cancellation inside native dialogs.

Only registered invocations and verified descendants/scopes are covered. A launcher that forwards to an unrelated existing instance cannot adopt it. No method guarantees that an application saves its data. See [backend details](reference/session-backends.md) and [ownership internals](reference/session-lifecycle.md#registering-applications).
:::

## Readiness

The compositor must publish its live display environment before target-based services can start.

```sh
rsdm session finalize
rsdm session finalize MY_VARIABLE
```

Official niri-session generally handles its own readiness. Bare WMs can finalize from their startup config. Managed readiness can be detected automatically; if it times out, targets stay inactive until a later finalize. Repeating finalize updates the environment.

See setup examples for [niri](compositors/niri.md), [Hyprland](compositors/hyprland.md) and [Sway](compositors/sway.md).

::: details Environment and graphical targets
RSDM preserves PATH, LANG, `XDG_RUNTIME_DIR`, validated session identity and existing seat/VT metadata. It captures live `WAYLAND_DISPLAY`, DISPLAY, XAUTHORITY and supported compositor endpoints (`NIRI_SOCKET`, `SWAYSOCK`, `HYPRLAND_INSTANCE_SIGNATURE`, `WAYFIRE_SOCKET`). Use `session_manager.extra_env` or finalize names for extra variables.

An owned managed session anchors `graphical-session.target` and xdg-desktop-autostart.target. Native providers own their targets. Attached sessions use private app environment and do not alter global target ownership. See [readiness internals](reference/session-lifecycle.md#readiness-and-recovery).
:::

## Logout and power

Save important work, then use these commands or bind them in your compositor:

```sh
rsdm session stop
rsdm power reboot
rsdm power poweroff
```

To request cancellation during application preparation, use rsdm session cancel from another terminal. It is available before teardown is committed; apps that already closed are not restored.

For an attached session without verified native logout, stop prepares registered apps and leaves the original desktop running. Use the desktop's normal exit action afterward. Native GNOME and Plasma can delegate to their own dialogs. Power requests remain subject to logind/polkit or native policy.

::: warning
Direct compositor exit, a crash or power loss can remove the display before apps finish saving. Force is the default timeout policy.
:::

::: details Interactive power monitor
In a supported terminal, power commands report stages, app methods and elapsed waits, systemd jobs and inhibitors. The stage bar represents observed stages, not an estimated percentage. Ctrl+C stops watching; a dispatched request can continue. A terminal in the graphical session disappears when that session ends. Redirected output uses one-shot reports. Suspend and hibernate do not prepare logout.
:::

## Status and outcomes

Run rsdm session status to inspect the live coordinator without starting it.

| Scope | Meaning |
| --- | --- |
| inactive | No live coordinator |
| disabled | Coordination disabled in config |
| full | Coordinator owns a verified graphical lifecycle |
| partial | Attached or necessary lifecycle control unavailable/unknown |

The phase shows startup, running, application preparation or teardown. Capabilities include a reason for available, unavailable, not-applicable and unknown states.

| Stop result | Meaning |
| --- | --- |
| completed | Owned logout finished |
| prepared | Apps prepared; original desktop remains |
| delegated | Native desktop accepted the request; it may still ask for confirmation |
| cancelled / failed / unknown | Operation cancelled, failed or outcome could not be established |

App preparation results include clean, forced, cancelled and unknown. Clean means no recorded escalation, not a guarantee of data persistence. Check the actual result and native dialog before assuming logout completed.

## Disable coordination

::: code-group

```toml [TOML]
[session_manager]
enabled = false
```

```nix [NixOS]
services.rsdm.sessionManager = false;
```

:::

A validated live change detaches coordination while leaving the apps, desktop and environment running. Re-enabling requires a new login to resume a retired generation. DM, Lock and PAM continue independently.

See [configuration](configuration.md#session-manager), [troubleshooting](troubleshooting.md), [backend reference](reference/session-backends.md) and [lifecycle internals](reference/session-lifecycle.md).
