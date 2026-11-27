---
description: Component requirements, supported session providers and integration test coverage.
---

# Compatibility

RSDM targets Linux Wayland sessions. The Greeter runs on a Linux VT and can
start installed Wayland sessions. Lock and Idle depend on protocols and
compositor policy; Session Manager has separate lifecycle and application
shutdown capabilities.

## Requirements

| Component | Required |
| --- | --- |
| DM / Greeter | Linux VT, PAM, logind/seat support, a Wayland session command |
| Lock | [ext-session-lock-v1](https://wayland.app/protocols/ext-session-lock-v1) allowed by the compositor |
| Idle | [ext-idle-notify-v1](https://wayland.app/protocols/ext-idle-notify-v1); version 2 for ignoring inhibitors |
| Session Manager | Linux pidfds, systemd 250+, logind, user D-Bus; one graphical coordinator per UID |
| Build | Rust 1.89+, PAM, Wayland, libxkbcommon, pkg-config; libSM/libICE for default XSMP support |

Use `wayland-info` inside the desktop to inspect advertised globals and
`rsdm session status` to inspect RSDM's current capabilities.

::: info X11
RSDM does not start or lock X11 desktop sessions. Some XWayland applications
inside an owned Wayland session can participate in XSMP or supported
window-close paths. That is application compatibility, separate from X11
display-manager support.
:::

## Distributions

| Distribution | Distribution integration | Verification scope |
| --- | --- | --- |
| [NixOS](installation/nixos.md) | Flake module, stable and unstable packages | Maintainer setup; module and isolated NixOS VM fixtures |
| [Arch](installation/arch.md) | AUR `rsdm` and `rsdm-bin` | Packaging available; desktop results welcome |
| [Fedora](installation/fedora.md) | Fedora PAM files, manual install | Source-build instructions; desktop results welcome |
| [Ubuntu](installation/ubuntu.md) | Debian-style PAM files, manual install | CI builds target Ubuntu 26.04; desktop results welcome |

Release archives and flake packages cover `x86_64-linux` and
`aarch64-linux`. Release binaries are Linux GNU binaries; older
distributions may need a native source build.

## Compositors and desktops

| Session | Lifecycle | Registered app window handling |
| --- | --- | --- |
| [niri](compositors/niri.md) | Native `niri-session` or owned bare niri | Verified JSON IPC for owned compositor |
| [Sway](compositors/sway.md) | Owned bare WM | Verified native Wayland window close |
| [Hyprland](compositors/hyprland.md) | Owned bare WM or an external provider | Verified Lua IPC on supported versions |
| GNOME | Native SessionManager | Native dialogs, if ownership verified |
| Plasma | Native LogoutPrompt | Native dialogs, if ownership verified |
| Standalone KWin | Owned compositor | Plasma window management on verified KWin 6.4+ with authorization |
| Wayfire | Owned compositor | Read-only IPC discovery; app policy fallback |
| Labwc / River / other owned WM | Managed when readiness/ownership can be established | Registry discovery; app policy fallback |
| Existing external session | Partial coordination | Native delegation when verified; otherwise generic app preparation |

The table describes implemented Session Manager paths. It does not imply
all compositors provide Lock or Idle. Native GNOME and Plasma may require
their own desktop locker. Check the protocol globals and test actual
locking before relying on RSDM Lock.

::: details What the isolated checks cover
The flake contains VM checks for native and bare sessions, external attach,
scope lifetime, Sway, niri, Hyprland, standalone KWin, Wayfire discovery,
Labwc/River registry discovery, browser windows, Flatpak, media apps,
power, Lock and DM. Window-close fixtures exercise process ownership,
save/cancellation and foreign-instance preservation. Hyprland's fixture
uses 0.55.4; KWin requires its verified 6.x version to be at least 6.4.

These are isolated fixtures, not a hardware or full application test matrix.
See [development](development.md#validation) for the current check names and
[session backends](reference/session-backends.md) for limitations.
:::

## Report a setup

When reporting a new setup, include the distribution, RSDM version,
compositor/version, PAM/keyring setup and relevant commands/status. Avoid
including passwords or other secret values. Use the
[test report form](https://github.com/Frysuni/rsdm/issues/new/choose).
