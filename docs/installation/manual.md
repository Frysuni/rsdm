---
description: Install RSDM binaries, systemd units and distro-specific PAM files manually.
---

# Manual installation

Use this guide on [Fedora](fedora.md), [Ubuntu](ubuntu.md) or another compatible
Linux distribution. Arch and NixOS have their own package/module instructions.
Install your distribution's dependencies first.

## Obtain RSDM

::: code-group

```sh [Source]
git clone https://github.com/Frysuni/rsdm.git
cd rsdm
# Optionally check out the desired release tag before building.
cargo build --release --locked -p rsdm
```

```sh [Release archive]
# Download the matching .tar.gz and .tar.gz.sha256 from GitHub Releases.
# Run in the directory containing those files; replace 2.5.0 as needed.
archive=rsdm-2.5.0-x86_64-unknown-linux-gnu.tar.gz
sha256sum -c "$archive.sha256" && \
  mkdir rsdm-release && \
  tar -xzf "$archive" -C rsdm-release && \
  cd rsdm-release
```

:::

[Download releases](https://github.com/Frysuni/rsdm/releases).
Use the `aarch64-unknown-linux-gnu` archive for ARM64. The release includes the
binary, configuration example and packaging files. A source build puts the
binary in `target/release/rsdm`.

::: warning Upgrading an existing install
Back up your existing `/etc/rsdm.toml` and `/etc/pam.d/rsdm*` files.
The install commands replace files at those paths; merge your local
configuration with the new examples rather than overwriting it blindly.
Run upgrades after logging out of an old RSDM session, or reboot into
the updated version.
:::

## Install runtime files

Run these commands from the source checkout or extracted release root.

::: code-group

```sh [Source binary]
sudo install -Dm755 target/release/rsdm /usr/bin/rsdm
```

```sh [Release binary]
sudo install -Dm755 rsdm /usr/bin/rsdm
```

:::

Install the common files:

```sh
sudo install -Dm644 rsdm.toml /etc/rsdm.toml
sudo install -Dm644 packaging/systemd/rsdm.service /etc/systemd/system/rsdm.service
sudo install -Dm644 packaging/systemd/rsdm@.service /etc/systemd/system/rsdm@.service
sudo install -Dm644 packaging/systemd/rsdm-idle.service /usr/lib/systemd/user/rsdm-idle.service
sudo install -Dm644 packaging/applications/org.rsdm.SessionManager.desktop \
  /usr/share/applications/org.rsdm.SessionManager.desktop
sudo systemctl daemon-reload
```

Use the PAM files from your [Fedora](fedora.md) or [Ubuntu](ubuntu.md)
guide. On another distribution, adapt its normal login and authentication
stacks to the requirements in [Security](../SECURITY.md). Do not blindly
copy a different distribution's PAM includes.

For optional file logging with logrotate installed:

```sh
sudo install -Dm644 packaging/logrotate/rsdm /etc/logrotate.d/rsdm
```

The supplied rule covers `/var/log/rsdm/*.log`; adjust it if you configure
a different log path. The default journal-only logging needs no rule.

## Activate or uninstall

Continue with [Activation and recovery](activate.md).
::: details Remove a manual installation
Follow [Disable DM](activate.md#disable-dm) first, including stopping the
Idle user service if enabled. Log out of RSDM sessions before removing the
binary. For the manual installation described here, remove the installed
runtime files:

```sh
sudo rm -f /usr/bin/rsdm \
  /etc/systemd/system/rsdm.service \
  /etc/systemd/system/rsdm@.service \
  /usr/lib/systemd/user/rsdm-idle.service \
  /usr/share/applications/org.rsdm.SessionManager.desktop \
  /etc/logrotate.d/rsdm
sudo systemctl daemon-reload
systemctl --user daemon-reload
```

Keep `/etc/rsdm.toml`, the RSDM PAM files and saved state if you plan to
reinstall. Remove those separately only when you no longer need them.
For a packaged installation, use the distribution package manager instead.
:::

::: details Custom executable paths
If you install outside `/usr/bin`, update the `ExecStart` in both DM units
and the Idle unit, and the `Exec` in the application desktop file.
The desktop file permits KWin's standalone window-management interface;
its executable must match the installed binary.
:::
