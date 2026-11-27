---
description: Component boundaries, login lifecycle, rendering, session coordination and recovery.
---

# Architecture

RSDM uses a shared terminal-style UI with separate Linux VT and Wayland backends. Authentication, session lifecycle and rendering have distinct responsibilities.

## Workspace

| Crate | Responsibility |
| --- | --- |
| rsdm-core | Domain types, ports, configuration and login use cases |
| rsdm-infra | PAM, user/session discovery, VT ownership, storage and DM launcher |
| rsdm-system | Linux process handles, clocks, generation IDs and logind transport |
| rsdm-ui | Shared cell-based design, palettes, animation, title and menus |
| rsdm-tui | Ratatui Greeter backend and input |
| rsdm-lock | Wayland session-lock surfaces, console-font renderer and authentication |
| rsdm-idle | Wayland idle notifications, locker supervision and hooks |
| rsdm-session-manager | User-side lifecycle, application units, recovery and XSMP |
| rsdm-cli | Public command routing, diagnostics and power monitor |

Core is independent of Linux OS APIs and rendering libraries. UI depends on Core. Session Manager depends on Core and System without depending on PAM or the frontends.

## Login lifecycle

1. DM validates configuration, resolves sessions and acquires its VT. It refuses a duplicate instance and waits for existing owners.
2. Greeter presents the login form. A PAM-owning helper authenticates the user and enforces the final account policy.
3. The helper establishes a logind session with its seat, TTY and Wayland desktop metadata. PAM can unlock the user's keyring.
4. Greeter clears/releases its VT after authentication. The PAM owner starts a gated child with a fresh rsdm image and a sealed request containing the validated session setup.
5. The child initializes groups, drops credentials, applies the environment, changes to HOME and execs the original command, normally wrapped in Session Manager.
6. The root PAM owner observes the actual session lifetime, performs bounded recovery as the desktop UID and closes PAM. DM waits for the VT to be free before drawing again.

::: details Process and terminal boundaries
The PAM owner lives in the logind session scope. Service restart does not intentionally terminate the live desktop. VT ownership checks cover controlling terminals and open VT descriptors; unreadable process state is treated as busy. Greeter temporarily suppresses console messages and restores the saved policy when leaving.

A start gate lets the PAM owner attempt its logind delay inhibitor before user-session setup; the gate closes if its parent fails. The root inhibitor remains held through user cleanup and PAM end. See [power and PAM lifetime](reference/session-lifecycle.md#power-and-pam-lifetime).
:::

## Rendering and Lock

Both frontends render the same styled glyph cells using the shared UI Surface. Greeter adapts them to the VT's character grid; Lock rasterizes them with the published console font and applies output scaling. Each has an independent design configuration.

Lock uses `ext-session-lock-v1`, creates opaque surfaces and waits for compositor confirmation. Authentication runs separately from the Wayland event loop. Emergency unlock is authenticated as described in [security](SECURITY.md#lock-screen). The personal overlay can persist only selected visual fields.

Idle tracks each seat via ext-idle-notify-v1. Its built-in locker readiness is a private channel from the confirmed lock, while an external foreground locker supplies process-start readiness. Hooks run as the user.

## Session coordination

A per-UID coordinator owns one generation and a non-replaceable D-Bus service name. Typed requests feed a serial lifecycle actor; workers perform systemd operations, application preparation, readiness and recovery.

Registered launches use transient user units. The coordinator records ownership before issuing side effects and verifies invocations, process handles and cgroups when acting. It preserves native desktop lifecycle where available. Application policies, readiness and cancellation are exposed through [Session Manager](session-manager.md).

::: details Async and recovery implementation
Session, app and power CLI commands own a Tokio runtime. The PAM owner starts its inhibitor runtime after fork. Lock keeps its event loop running while power/authentication workers work. XSMP uses its own libSM/libICE thread with bounded shutdown. Recovery leases and records are private, and filesystem work is kept away from D-Bus waits.

Request slots, absolute deadlines, saved dispatch intent and live ownership checks prevent stale requests from affecting later generations. User-manager reexec and bus reconnection are observed without treating them as logout. Read [lifecycle internals](reference/session-lifecycle.md) for precise guarantees, limits and recovery behavior.
:::

## CLI output

Styled reports use in-memory Ratatui widgets and write rows to ordinary scrollback. Terminal detection selects plain output on redirects, `NO_COLOR`, dumb or narrow terminals. Styled journal reads decode records incrementally; plain output delegates to journalctl.

See [development](development.md) for build and test commands and [security](SECURITY.md) for trust boundaries.
