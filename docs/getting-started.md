---
description: Choose the RSDM components you need and install them safely.
---

# Getting started

RSDM combines four components. Choose only the ones you need.

| Component | Purpose | Default TOML setting |
| --- | --- | --- |
| DM / Greeter | PAM login on a Linux virtual terminal and Wayland session startup | `dm.enable = true` |
| Lock | Password-protected screen lock inside a Wayland session | `lock.enable = false` |
| Idle | Automatic locking after inactivity | `idle.enable = false` |
| Session Manager | Application launch, environment publication and orderly shutdown | `session_manager.enabled = true` |

DM is the display manager runtime. Greeter is its interactive terminal
interface. Lock is a separate Wayland client with its own configuration.

## Choose your distribution

- [NixOS](installation/nixos.md) — declarative module and packages.
- [Arch Linux](installation/arch.md) — AUR source or prebuilt packages.
- [Fedora](installation/fedora.md) — native build or release archive.
- [Ubuntu](installation/ubuntu.md) — native build or release archive.

All guides install the necessary runtime files. The requirements differ
between components; check [compatibility](compatibility.md).

## First login

1. Install a Wayland compositor and make sure it runs with your usual login.
2. Install RSDM, review the PAM configuration and choose the Greeter's VT.
3. Validate the configuration with `rsdm validate-config`.
4. Activate DM according to the distribution guide. Log in and select a
   session (or set `dm.fixed_session` to skip the picker).
5. In your desktop, run `rsdm session status` and check its capabilities.

::: warning Before changing your display manager
Keep access to another virtual terminal and its normal login. Save your
work, log out of the existing graphical session and switch to that console
before replacing the display-manager service.
:::

## First lock

Enable Lock in the configuration, then run the following **as your desktop
user, inside Wayland**:

::: code-group

```toml [TOML]
[lock]
enable = true
```

```nix [NixOS]
services.rsdm.lock.enable = true;
```

:::

```sh
rsdm lock
```

Unlock with your current account credentials. Set a compositor shortcut
using the [Lock guide](lock.md), then optionally enable [Idle](idle.md).

## Use orderly shutdown

Session Manager is enabled by default for logins started by RSDM.
Register an application to give it a shutdown policy:

```sh
rsdm app -- foot
rsdm session status
```

Use `rsdm session stop` to log out. It prepares registered applications
before ending an owned session. See [Session Manager](session-manager.md)
for external sessions, native desktops and timeout behavior.

## Where to go next

- [Commands](README.md) — all CLI commands.
- [Configuration](configuration.md) — every setting and its default.
- [Compositor setup](compositors/niri.md) — startup and key bindings.
- [Troubleshooting](troubleshooting.md) — diagnostics and recovery.
