# rsdm documentation

`rsdm` is a standalone Rust display manager (TTY/TUI greeter) and screen locker
for Wayland sessions. No greetd, GTK, Qt, webview, or Electron. One binary,
`rsdm`, with a few subcommands:

| command                 | what it does                                        |
|-------------------------|-----------------------------------------------------|
| `rsdm dm`               | the greeter, on the configured VT                   |
| `rsdm lock`             | lock the current Wayland session                    |
| `rsdm idle`             | lock after compositor-reported inactivity           |
| `rsdm unlock`           | privileged emergency unlock of a live rsdm locker   |
| `rsdm logs`             | open the system/user journal interactively          |
| `rsdm status`           | show config and live lock state                     |
| `rsdm session start`    | start coordination around the original session command |
| `rsdm session finalize` | export env + activate the session from a compositor |
| `rsdm session stop`     | prepare registered apps, then request logout         |
| `rsdm session cancel`   | cancel preparation before session teardown           |
| `rsdm session status`   | show the provider, phase and registered apps          |
| `rsdm power reboot`     | prepare the session and request reboot                |
| `rsdm power poweroff`   | prepare the session and request shutdown              |
| `rsdm app -- <cmd>`     | register and launch an app with its shutdown policy   |
| `rsdm validate-config`  | check `rsdm.toml`                                   |

All commands take a global `--config <path>` (default `/etc/rsdm.toml`,
also `RSDM_CONFIG`).

## CLI output

Help, system/session status, command results and diagnostics use colored panels
in the terminal. Reports stay in scrollback and the command returns normally.
Long values wrap to the available width, including application unit names and
configuration paths.

```sh
rsdm --help
rsdm status
rsdm session status
rsdm validate-config
env NO_COLOR=1 rsdm status
rsdm session status > session-status.txt
```

Colors distinguish successful states, pending/cancelled requests, warnings and
errors. Redirected streams keep the plain text format. A nonempty `NO_COLOR`,
`TERM=dumb` or a terminal narrower than 28 columns selects plain output too.
Errors and warnings remain on stderr, and command exit codes retain their
meaning.

Successful app launches, finalize and cancel requests show a confirmation when
terminal styling is active; they stay quiet in scripts and autostart. The
Greeter and Lock keep their existing interfaces. `rsdm logs` opens journalctl
with its normal pager/follow behavior and formatting.

## Guides

- [Installation: Arch](installation/arch.md)
- [Installation: Fedora](installation/fedora.md)
- [Installation: NixOS](installation/nixos.md)
- [Installation: Ubuntu](installation/ubuntu.md)
- [Configuration reference](configuration.md)
- [Display manager (greeter)](display-manager.md)
- [Lock screen](lock.md)
- [Idle locking](idle.md)
- [Keyrings](keyrings.md)
- [Session manager](session-manager.md)
- [Troubleshooting](troubleshooting.md)

## Compositors

`rsdm` is compositor-agnostic. Setup notes for the common ones:

- [niri](compositors/niri.md)
- [Hyprland](compositors/hyprland.md)

## See also

- [ARCHITECTURE.md](ARCHITECTURE.md) - crate layout and runtime paths
- [SECURITY.md](SECURITY.md) - PAM, privilege handling, lock guarantees
- [rsdm.toml](../rsdm.toml) - practical commented configuration example
