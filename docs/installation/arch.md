---
description: Install RSDM from the Arch User Repository or build the Arch package.
---

# Arch Linux

Choose one AUR package:

::: code-group

```sh [Prebuilt]
yay -S rsdm-bin
```

```sh [Source]
yay -S rsdm
```

:::

[`rsdm-bin`](https://aur.archlinux.org/packages/rsdm-bin) installs a release
binary. [`rsdm`](https://aur.archlinux.org/packages/rsdm) builds a tagged
release. You can use another AUR helper or clone the chosen AUR repository
and run `makepkg -si` as a normal user.

::: details Build the AUR package manually
```sh
git clone https://aur.archlinux.org/rsdm.git
cd rsdm
less PKGBUILD
makepkg -si
```

The PKGBUILD templates under `packaging/arch` in the upstream source tree
are used by release publishing; use the actual AUR repository for the
published package version and hashes.
:::

## Configure and activate

The package installs the binary, administrative configuration, PAM stacks,
system DM units and user Idle unit. It does not enable DM automatically.

```sh
sudoedit /etc/rsdm.toml
rsdm validate-config
```

Continue with [Activation and recovery](activate.md) to switch from your
previous display manager, choose a VT or run only Lock/Idle.

## Update or remove

Use your AUR helper to update. Review any `.pacnew` files for the configuration
and PAM policy, then start a new session with the updated package.

Before removing RSDM, follow [Disable DM](activate.md#disable-dm) and stop
the Idle user service if enabled. Then remove whichever package you installed:

```sh
sudo pacman -R rsdm-bin
# or: sudo pacman -R rsdm
```

::: details Package contents and dependencies
Runtime: `pam`, `systemd>=250`, `wayland`, `libxkbcommon`, `libsm`,
`libice`, `gcc-libs`. Source build additionally needs Cargo and pkgconf.

The package installs `/usr/bin/rsdm`, `/usr/lib/systemd/system/rsdm.service`,
`rsdm@.service`, `/usr/lib/systemd/user/rsdm-idle.service`,
`/etc/pam.d/rsdm`, `/etc/pam.d/rsdm-lock`, `/etc/rsdm.toml` and the
logrotate rule, KWin authorization desktop file and packaged guide. See the [keyring guide](../keyrings.md) for Arch's optional
GNOME Keyring and KWallet PAM entries.
:::
