---
description: Configure the Linux VT Greeter, choose Wayland sessions and manage login.
---

# DM and Greeter

DM owns login and the selected Linux virtual terminal. Greeter is its interactive terminal interface. After PAM authentication, DM starts the selected Wayland session and waits for it to exit.

## Select a session

By default, the Greeter lists installed Wayland .desktop entries. Use a fixed session to skip the picker.

::: code-group

```toml [TOML]
[dm]
fixed_session = "niri-session"
```

```nix [NixOS]
services.rsdm.dm.fixedSession = "niri-session";
```

:::

Use a desktop entry ID (preferred), unique name/Exec, or a literal command. Remove the option to restore discovery. See [configuration](configuration.md#dm-and-greeter) for search paths and TTY settings.

## Controls

| Key | Action |
| --- | --- |
| Tab / Down | Next field |
| Shift+Tab / Up | Previous field |
| Left / Right | Change session |
| F2 / Enter on session field | Open session picker |
| Enter | Advance or authenticate |
| F1 | Appearance menu, if enabled |
| F11 / F12 | Reboot / power off |
| Esc | Exit to console login when permitted |

Power actions and console exit require a second press to confirm. A different key cancels the confirmation and is consumed. PAM can request extra credentials such as an OTP.

## Appearance

```toml
[dm.design]
theme = "nord"
border_style = "modern"
background = "rain"
title_mode = "hostname"
```

F1 changes the current Greeter instance. To keep the design across logins, update the system configuration. See [all design settings](configuration.md#appearance).

## Multiple monitors

The Linux VT exposes one shared character grid. The kernel may clone it across connected displays; Greeter cannot independently select a monitor or render separate layouts. Lock has per-output configuration because it runs inside Wayland.

::: details Console output selection
Kernel display parameters (such as `video=HDMI-A-1:d`) can affect which connectors are enabled at boot. Apply these through your bootloader or NixOS boot.kernelParams only if appropriate for your hardware, and verify that the compositor can restore the desired output. Keep a working console available.
:::

## Console fallback

The fallback is enabled by default but restricted by the login policy. To allow handing the VT to the system login, both `deny_root = false` and `allowed_groups = []` are required. This hands authentication and attempt limits to the normal console login policy.

::: warning
Keep the default restricted policy if you rely on RSDM's account restrictions. Use a separate VT for recovery.
:::

::: details Service and session lifetime
Run DM through its systemd unit with a matching `TTYPath`. `StandardInput=tty` and journal output keep program output separate from the Greeter. The packaged unit manages runtime getty/autovt masks. Avoid `TTYReset`, `TTYVHangup` and `TTYVTDisallocate` on service teardown, since the user's session may still own the VT.

DM uses the configured PAM service, supplies the seat and TTY to logind, transfers the PAM environment and starts the desktop with the user's credentials. Session Manager normally wraps the original Exec command. PAM remains open until the actual desktop exits. See [architecture](ARCHITECTURE.md).
:::

To enable, disable or move the service, follow [activation](installation/activate.md) or the [NixOS guide](installation/nixos.md).
