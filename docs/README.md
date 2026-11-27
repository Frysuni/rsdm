---
title: Commands
description: Public RSDM CLI commands, options, terminal output and diagnostics.
---

# Commands

Use `rsdm --help` or append `--help` to any command for the installed version's
exact syntax. Commands accept `--config <path>` (default
`/etc/rsdm.toml`); the `RSDM_CONFIG` environment variable can set that path.

## Command reference

| Command | Purpose |
| --- | --- |
| `rsdm dm` | Start DM and its Greeter on the configured VT (normally through systemd) |
| `rsdm lock` | Lock the current Wayland session |
| `rsdm idle` | Run compositor-driven idle monitoring |
| `rsdm unlock [--user <name>]` | Root-authorized emergency unlock of a live RSDM locker |
| `rsdm status` | Show configured features and live lock state |
| `rsdm validate-config` | Check the administrative TOML file |
| `rsdm logs` | Read RSDM journal entries for the current boot |
| `rsdm session start -- <command>` | Launch a session under coordination |
| `rsdm session attach` | Attach to an existing graphical login |
| `rsdm session finalize [names...]` | Publish the compositor environment and complete readiness |
| `rsdm session status` | Read manager scope, phase, apps and capabilities |
| `rsdm session stop` | Prepare apps and request logout |
| `rsdm session cancel` | Request cancellation before session teardown |
| `rsdm app -- <command> [args...]` | Register and launch an application |
| `rsdm power reboot` | Prepare the session and request reboot |
| `rsdm power poweroff` | Prepare the session and request shutdown |

For start modes, application policies and outcome meanings see
[Session Manager](session-manager.md). Run session/app/lock/idle commands
as the desktop user. The system service runs DM with the privilege needed
for PAM and VT management.

## Common tasks

```sh
rsdm --version
rsdm validate-config --config ./rsdm.toml
rsdm status
rsdm session status
rsdm logs --component dm --lines 50
rsdm logs --follow
```

### Logs

| Option | Default | Description |
| --- | --- | --- |
| `-n, --lines <N>` | `200` | Number of recent entries |
| `-f, --follow` | off | Stream new entries until Ctrl+C |
| `--component <name>` | `all` | `all`, `dm`, `idle`, `lock` |

Filters retain journal access permissions. For a custom DM instance use
`journalctl -b -u rsdm@tty2.service` (replace the instance name).
For the user coordinator and application units use `journalctl --user -b`.

## CLI output

Terminal reports use panels, status colors and normal scrollback.
Redirected streams, nonempty `NO_COLOR`, `TERM=dumb` and terminals narrower
than 28 columns use plain text. Warnings and errors remain on stderr.

```sh
env NO_COLOR=1 rsdm status
rsdm session status > session-status.txt
```

::: details Journal formatting and scripting
Styled logs decode journal entries with UTC timestamps, levels and sources
without a pager. Plain output delegates to journalctl's short-precise
format and its usual pager/follow behavior. Successful app launches,
finalize and cancel requests may show a confirmation in styled output.

Use exit codes for automation. A cancellation or a failed/unknown session
request is nonzero. A successful request can mean preparation or native
delegation rather than completed logout; see
[session outcomes](session-manager.md#status-and-outcomes).
:::

## Further reading

[Getting started](getting-started.md) ·
[Configuration](configuration.md) ·
[Troubleshooting](troubleshooting.md) ·
[Build the documentation](development.md#documentation).
