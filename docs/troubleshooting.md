---
description: Diagnose failed login, session startup, Lock, Idle and shutdown safely.
---

# Troubleshooting

Keep an alternate working VT before changing DM or PAM. Run desktop commands as the logged-in user.

## Collect diagnostics

```sh
rsdm --version
rsdm validate-config
rsdm status
rsdm session status
rsdm logs --lines 100
```

::: details Systemd logs and services
```sh
systemctl status rsdm.service
journalctl -b -u rsdm.service
systemctl --user status graphical-session.target
systemctl --user status rsdm-idle.service
journalctl --user -b
```

For a template instance replace `rsdm.service` with `rsdm@tty2.service` or the actual unit. The CLI DM filter covers the standard unit. Use `logging.level` = "debug" or `RUST_LOG`=rsdm=debug for more detail at the next safe launch.
:::

## Login and Greeter

| Symptom | Check |
| --- | --- |
| No Greeter at boot | Enabled unit, `graphical.target`, TTYPath and display-manager alias |
| VT already owned / waiting | An active login still holds that VT; wait or use another console |
| No sessions listed | Installed Wayland desktop files and `dm.session_dirs`; check fixed_session |
| Authentication fails | PAM stack, account/group policy and attempt limit |
| Garbled TTY | Use the packaged unit with journal output; check kernel console and font |
| Keyring remains locked | [PAM keyring setup](keyrings.md) |
| Greeter cloned across displays | Linux fbcon exposes one shared VT grid |

If login fails, use another VT to inspect configuration and logs. See [activation and recovery](installation/activate.md) or roll back the NixOS configuration. Do not restart services holding an active session just to collect logs.

## Lock and Idle

| Symptom | Check |
| --- | --- |
| Lock refuses to start | `lock.enable`, Wayland session-lock protocol, current user, personal overlay |
| Wrong monitor | `lock.primary_output`; use the compositor's output list |
| Font differs from Greeter | Remove size override and republish the console font at the next DM start |
| Wallpaper invisible | Absolute readable PNG/JPEG path and wallpaper_dim above zero |
| Blurry fractional scale | `fractional-scale-v1` and viewporter support; check scale_120 in debug logs |
| Idle never locks | `idle.enable`, running user service, protocol globals and idle inhibitors |
| Hook missing | Built-in confirmation, custom locker exit status and hook logs |

Use [emergency unlock](lock.md#emergency-unlock) only from an authenticated administrator session if a live locker cannot authenticate. A crashed locker may require terminating the affected graphical session.

## Session Manager

| Symptom | Check |
| --- | --- |
| inactive / disabled | Start or attach inside the correct graphical login; check enabled policy |
| starting persists | Compositor readiness; run finalize inside the desktop |
| partial | Read the capability reasons; external sessions intentionally have limited lifecycle control |
| App absent from status | Register the first instance through rsdm app; existing processes are not adopted |
| App forced | Use its documented quit method, increase timeout or use cancel where supported |
| prepared | Native control unavailable or attached lifecycle; exit the original desktop separately |
| delegated | Check native logout dialog; request acceptance is not final completion |
| Cancellation rejected | Preparation may already have committed teardown |

See [status and outcomes](session-manager.md#status-and-outcomes) and [backend limitations](reference/session-backends.md).

## Report an issue

Include the RSDM version, distribution, compositor/version, relevant configuration (remove sensitive values), session status, and a short journal excerpt. Mention the reproduction steps and expected behavior. Never include passwords, tokens or private authentication material.

[Open an issue](https://github.com/Frysuni/rsdm/issues/new/choose).
