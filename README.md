# RSDM — Rust Display Manager

[![License: GPL-3.0](https://img.shields.io/badge/license-GPL--3.0-blue)](LICENSE)
[![Documentation](https://img.shields.io/badge/docs-RSDM_Wiki-087c78)](https://frysuni.github.io/rsdm/)
[![Release](https://img.shields.io/github/v/release/Frysuni/rsdm)](https://github.com/Frysuni/rsdm/releases/latest)

RSDM is a standalone Linux display manager with a terminal Greeter, a Wayland
Lock, compositor-driven idle locking and systemd user-session coordination.

One Rust binary provides PAM login, session selection, independent visual
themes, application shutdown policies and session-aware power actions.
The Greeter runs on a Linux virtual terminal; Lock runs inside a compatible
Wayland session.

> [!WARNING]
> RSDM is experimental. Keep a working console login when trying a new
> display manager. Save your work before testing logout or power actions.

## Install

| Distribution | Installation |
| --- | --- |
| NixOS | [Flake module and channels](docs/installation/nixos.md) |
| Arch Linux | [AUR packages](docs/installation/arch.md) |
| Fedora | [Source or release installation](docs/installation/fedora.md) |
| Ubuntu | [Source or release installation](docs/installation/ubuntu.md) |

Choose a guide to install the PAM and systemd integration as well as the binary.
For the supported platforms, protocols and test scope, see
[Compatibility](docs/compatibility.md).

## Use

```sh
rsdm validate-config
rsdm status
rsdm lock
rsdm app -- foot
rsdm session status
```

Set `[lock].enable = true` before using Lock. Run Lock and application/session
commands from your Wayland desktop as your own user. DM normally starts as
`rsdm.service`. Use `rsdm session stop` when you are ready to log out. Use `rsdm --help` or the
[command reference](docs/README.md) for all public commands.

## Documentation

**[Read the RSDM Wiki](https://frysuni.github.io/rsdm/)** for searchable guides,
examples and expandable technical references.

| Topic | Guide |
| --- | --- |
| Login, VT and session selection | [DM and Greeter](docs/display-manager.md) |
| Lock and automatic idle locking | [Lock](docs/lock.md) · [Idle](docs/idle.md) |
| Orderly logout and power | [Session Manager](docs/session-manager.md) |
| Themes, wallpaper and all options | [Configuration](docs/configuration.md) |
| Keyring integration | [Keyrings](docs/keyrings.md) |
| Compositor setup | [niri](docs/compositors/niri.md) · [Hyprland](docs/compositors/hyprland.md) · [Sway](docs/compositors/sway.md) |
| Recovery and diagnostics | [Troubleshooting](docs/troubleshooting.md) |
| Development and internals | [Development](docs/development.md) · [Architecture](docs/ARCHITECTURE.md) · [Security](docs/SECURITY.md) |

The administrative configuration is `/etc/rsdm.toml`. See the
[complete example](rsdm.toml). On NixOS, the module generates this file.

## Compatibility

RSDM targets Wayland on Linux. NixOS and niri have maintainer-tested setups;
the project includes isolated integration fixtures for other session providers
and applications. A distribution package or a successful build does not by
itself establish end-to-end desktop compatibility. See the
[compatibility matrix](docs/compatibility.md) for the distinction between
login, locking, idle and shutdown support.

## Build from source

```sh
nix build .#rsdm-unstable
# Or enter the dev shell and build with Cargo:
nix develop
cargo build --release --locked -p rsdm
```

Rust 1.89+ is required. See [development](docs/development.md) for system
dependencies, tests and the local documentation build. Report reproducible
issues or new compatibility results through [GitHub Issues](https://github.com/Frysuni/rsdm/issues/new/choose).

## License and credits

Licensed under [GPL-3.0](LICENSE). RSDM draws inspiration from
[greetd](https://github.com/kennylevinsen/greetd),
[tuigreet](https://github.com/apognu/tuigreet),
[sysc-greet](https://github.com/Nomadcxx/sysc-greet),
[uwsm](https://github.com/Vladimir-csp/uwsm),
[hypridle](https://github.com/hyprwm/hypridle) and
[hyprlock](https://github.com/hyprwm/hyprlock).

> [!NOTE]
> **RSDM is built with AI as an engineering tool.**
