# RSDM

> [!TIP]
> What happens when you take the evolution of [**greetd**](https://github.com/kennylevinsen/greetd) → [**tuigreet**](https://github.com/apognu/tuigreet) → [**sysc-greet**](https://github.com/Nomadcxx/sysc-greet), combine it with [**uwsm**](https://github.com/Vladimir-csp/uwsm), the responsibilities of a full display manager, [**hypridle**](https://github.com/hyprwm/hypridle), and [**hyprlock**](https://github.com/hyprwm/hyprlock), then rebuild the entire stack as one lightweight Rust binary?
>
> You get a deeply integrated system that handles native PAM authentication, session startup, environment management, idle behavior, and screen locking directly at the system level.
>
> No fragile chains of loosely connected tools. No endless configuration. No critical details left for the user to figure out.
>
> **One binary. One coherent login stack. Everything thoughtfully integrated.**
>
> **That is how RSDM was born.**

A standalone Rust display manager (TTY/TUI greeter) and screen locker for
Wayland sessions. No greetd, GTK, Qt, webview, or Electron - one small binary
that owns a virtual terminal, authenticates through PAM, and launches your
compositor.

- TTY/TUI greeter (`rsdm dm`) with theme, border, animated background, generated
  FIGlet title, and a runtime `F1` design switcher.
- `ext-session-lock-v1` locker (`rsdm lock`) that renders the same design on a
  software framebuffer, with optional wallpaper, dim, and animated background.
- Compositor-driven idle locking (`rsdm idle`) with confirmed-lock hooks and an
  optional user service.
- Optional uwsm-style systemd `--user` session manager (`rsdm session`,
  `rsdm app`) that exports the Wayland/XDG environment and closes registered
  applications before the compositor, with XSMP and native DE delegation.
- Compositor-agnostic: niri, Hyprland, sway, KDE, GNOME, anything with a Wayland
  session.
- Runs on Arch (AUR) and NixOS (flake module). GPL-3.0.

> [!WARNING]
> RSDM is still experimental. Expect bugs, incomplete integrations, and breaking changes.

## Compatibility

| Platform / component               | Status                            |
| ---------------------------------- | --------------------------------- |
| NixOS                              | ✅ Tested                          |
| niri                               | ✅ Tested                          |
| GNOME Keyring                      | ✅ Tested                          |
| Fedora                             | 🟡 Ready, not tested              |
| Ubuntu                             | 🟡 Ready, not tested              |
| Arch Linux                         | 🟡 Ready, not tested              |
| Hyprland                           | 🟡 Ready, not tested              |
| KWallet                            | 🟡 Ready, not tested              |
| Other distributions                | ⚪ Not supported yet, but possible |
| Other Wayland desktop environments | ⚪ Not supported yet, but possible |
| X11                                | ❌ Will never be supported         |

Tested RSDM on a new setup? Please [submit a test report](../../issues/new/choose) with your distribution, compositor, keyring, configuration, and results.

## Commands

```sh
rsdm dm                      # the greeter, on the configured VT (usually via systemd)
rsdm lock                    # lock the current Wayland session
rsdm idle                    # lock automatically after the configured timeout
rsdm unlock                  # root-authorized emergency unlock
rsdm logs --follow           # combined DM/idle/lock journal
rsdm status                  # configuration and live lock state
rsdm session start -- niri-session # use niri's original native session wrapper
rsdm session stop            # prepare registered apps, then close the session
rsdm session cancel          # cancel preparation before infrastructure teardown
rsdm session status          # provider, phase, generation and app policies
rsdm power reboot            # prepare the current session and request reboot
rsdm power poweroff          # prepare the current session and request shutdown
rsdm app -- waybar           # launch a program into the graphical session
rsdm validate-config         # check the config
```

## Session manager

Enabled by default when you log in through RSDM. Launch applications through
`rsdm app` and bind your WM's logout action to `rsdm session stop`:

```sh
rsdm app -- foot
rsdm app --shutdown-timeout 60 --on-timeout cancel -- foot
rsdm session status
rsdm session stop
```

RSDM gives registered apps time to save and exit while the compositor is still
running. This can reduce “did not shut down correctly” warnings on the next
launch. GNOME and Plasma keep their native shutdown dialogs. Use
`rsdm power reboot` or `rsdm power poweroff` for session-aware power actions.

Requires Linux, systemd 250+, logind and a user bus. Apps launched outside
`rsdm app` are not automatically registered; clean saves depend on the app's
quit support. See the [session manager guide](docs/session-manager.md) for WM
bindings, quit commands and timeout policies.

## Install

### Arch

```sh
yay -S rsdm        # build from a tagged release
yay -S rsdm-bin    # prebuilt binary
```

Then edit `/etc/rsdm.toml`, `rsdm validate-config`, and
`sudo systemctl enable --now rsdm.service`. See
[docs/installation/arch.md](docs/installation/arch.md).

### NixOS Flake

```nix
inputs.rsdm.url = "github:Frysuni/rsdm";

services.rsdm = {
  enable = true;
  package = inputs.rsdm.packages.x86_64-linux.default;
  dm = {
    tty = "tty1";
    fixedSession = "niri-session";
  };
  lock = {
    enable = true;
    primaryOutput = "DP-1";
    secondaryOutput = "background"; # background, black, or off (niri)
    size = null; # TTY-like auto size, or an integer from 1 to 12
  };
  idle = {
    enable = true;
    timeout = 300;
  };
};
```

Set `services.rsdm.channel = "stable"` (the default) to download the stable
release binary, or `"unstable"` to compile the development version locally.
Run `nix flake update rsdm` and rebuild to update either channel. Existing Nix
caches provide available dependencies; no project cache configuration is needed.
See [docs/installation/nixos.md](docs/installation/nixos.md).

## Documentation

- [docs/](docs/README.md) - full guide index
- Installation: [NixOS](docs/installation/nixos.md), [Arch](docs/installation/arch.md),
  [Fedora](docs/installation/fedora.md), [Ubuntu](docs/installation/ubuntu.md)
- [Display manager](docs/display-manager.md)
- [Lock screen](docs/lock.md)
- [Idle locking](docs/idle.md)
- [Keyrings](docs/keyrings.md)
- [Session manager](docs/session-manager.md)
- [Configuration](docs/configuration.md)
- [ARCHITECTURE.md](docs/ARCHITECTURE.md) - [SECURITY.md](docs/SECURITY.md)

## Build

```sh
nix build            # ./result/bin/rsdm
nix develop          # dev shell with the toolchain + deps
# or directly:
cargo build --release -p rsdm
```

Dependencies: Rust (edition 2024, >= 1.88), `pam`, `wayland`, `libxkbcommon`,
`pkg-config`, `libSM` and `libICE` (for XSMP, enabled by default).

## Runtime

- PAM services: `rsdm` (greeter login, unlocks the keyring), `rsdm-lock` (locker)
- Default config: `/etc/rsdm.toml`
- Cache/state: `/var/cache/rsdm`
- Logs: journald, through the systemd service (`journalctl -u rsdm.service`)

## License

Licensed under [GNU GPLv3](LICENSE).

> [!NOTE]
> **RSDM is built with AI as an engineering tool.**
