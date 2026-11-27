---
description: Set automatic Wayland idle locking, inhibitor behavior and lock/unlock hooks.
---

# Idle locking

Idle tracks compositor input inactivity and starts a locker after the configured delay. It works independently of DM.

## Enable

::: code-group

```toml [TOML]
[lock]
enable = true

[idle]
enable = true
timeout = 300
```

```nix [NixOS]
services.rsdm.lock.enable = true;
services.rsdm.idle = {
  enable = true;
  timeout = 300;
};
```

:::

The NixOS module installs the graphical user service. On other distributions, enable the packaged service from your graphical session:

```sh
systemctl --user daemon-reload
systemctl --user enable --now rsdm-idle.service
```

For a compositor without `graphical-session.target` integration, launch `rsdm idle` from its startup configuration instead. Use one idle monitor. Stop any duplicate automatic locker. For foreground diagnostics, run `RUST_LOG=rsdm_idle=debug,rsdm_lock=debug rsdm idle` instead of the service.

## Inhibitors

By default, application idle inhibitors are respected, for example during video playback. Set `ignore_inhibitors = true` under `[idle]` to track raw input inactivity instead. This requires `ext-idle-notify-v1` version 2; version 1 falls back to respecting inhibitors.

All seats must become idle before locking. Activity resets the timer but never unlocks the screen. A failed locker is retried while every monitored seat remains idle.

## Run commands when locking or unlocking

```toml
[idle]
enable = true
timeout = 300
on_lock = [["notify-send", "Screen locked"]]
on_unlock = [["notify-send", "Screen unlocked"]]
```

The example requires `[lock].enable = true`. Hooks run as the desktop user, in order, as literal argv arrays. Use explicit `sh -c` only if you intentionally need shell syntax.

::: details Hook timing
The built-in locker reports compositor-confirmed lock over a private readiness channel. `on_lock` runs after that confirmation. With a custom foreground locker, readiness means the process started. `on_unlock` runs only after a successful exit following readiness (including emergency unlock). It does not run after a crash, signal or nonzero exit. Each hook phase shares a 30-second budget; timed-out hook process groups are killed and later hooks in that phase are skipped. A new lock cycle can start while previous hooks finish. Under the user service, managed locks have their own transient scope so restarting the monitor does not kill a live lock.
:::

## Use another locker

```toml
[idle]
enable = true
lock_command = ["swaylock"]
```

Use a foreground command that stays alive until unlock. The built-in Lock component can remain disabled when a custom locker is configured.

::: warning Suspend
Idle reacts to compositor inactivity. It does not by itself guarantee the screen is locked before every system suspend. If you need pre-suspend locking, use your compositor/desktop's supported readiness-aware integration and test it separately.
:::

See [configuration](configuration.md#idle) and [troubleshooting](troubleshooting.md).
