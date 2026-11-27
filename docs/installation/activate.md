---
description: Enable, switch and safely disable RSDM systemd services on non-NixOS distributions.
---

# Activation and recovery

For packaged or manually installed RSDM on systemd distributions.
On NixOS, use the [module](nixos.md) instead.

## Prepare

```sh
sudoedit /etc/rsdm.toml
rsdm validate-config
```

Check `[dm.tty].path`, your `[dm].fixed_session` (or installed
Wayland session entries), and the DM PAM service. Keep a working login
on a different VT such as tty3.

::: warning Switching display managers
Save your work, log out and use another VT. Identify the old DM before
disabling it. Switching the active service may terminate its graphical
session. Never test a new PAM configuration on a remote-only host.
:::

## Enable DM

Inspect the existing display-manager alias:

```sh
systemctl status display-manager.service
systemctl get-default
```

If another manager owns the alias, disable its actual unit first (for
example, `sudo systemctl disable --now sddm.service` if you use SDDM).
Then enable RSDM:

```sh
sudo systemctl enable rsdm.service
sudo systemctl set-default graphical.target
sudo systemctl start rsdm.service
```

The default unit uses tty1 and installs the `display-manager.service` alias.
The Greeter should now appear on tty1. Check
`journalctl -b -u rsdm.service` if it does not.

::: details Use a different virtual terminal
Set the matching path in the config:

```toml
[dm.tty]
path = "/dev/tty2"
seat = "seat0"
```

Use the installed template rather than the tty1 service:

```sh
sudo systemctl enable --now rsdm@tty2.service
```

Enable only one RSDM instance with this shared configuration. The template
has no display-manager alias and derives its VT and getty masks from its
instance. Stop the instance before changing the VT or removing the unit.
:::

## Use Lock and Idle without DM

You can leave DM disabled and still use RSDM inside another Wayland login.

```toml
[dm]
enable = false

[lock]
enable = true

[idle]
enable = true
timeout = 300
```

Inside your graphical session:

```sh
rsdm lock
systemctl --user daemon-reload
systemctl --user enable --now rsdm-idle.service
```

Unlock first, then enable the Idle monitor. It needs a compatible compositor
and its graphical-session target. See [Idle](../idle.md) for other launch
methods. Session Manager remains independently configurable.

## Disable DM

Log out of the graphical session and switch to a working VT, then:

```sh
sudo systemctl disable --now rsdm.service
# If you used the template:
# sudo systemctl disable --now rsdm@tty2.service
```

The packaged unit releases its runtime getty/autovt masks on stop.
If you intend to use console login on tty1, check and start its getty:

```sh
sudo systemctl start getty@tty1.service
```

Restore your previous DM's service if desired, or set
`sudo systemctl set-default multi-user.target` for text-mode boot.
To disable Idle separately, run
`systemctl --user disable --now rsdm-idle.service` inside the session.

See [Troubleshooting](../troubleshooting.md) for failures and emergency unlock.
