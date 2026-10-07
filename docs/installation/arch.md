# Installation: Arch Linux

## From the AUR

AUR packages are installed with an AUR helper (or a manual `git clone` +
`makepkg -si`), not with `pacman -S`:

```sh
# source package: builds rsdm from a tagged release with cargo
yay -S rsdm
# or the prebuilt binary package
yay -S rsdm-bin
```

(`paru`, `aurutils`, or a manual `makepkg -si` work the same.)

## From this repo

```sh
cd packaging/arch
makepkg -si
```

Both paths install:

| file                                    | purpose                              |
|-----------------------------------------|--------------------------------------|
| `/usr/bin/rsdm`                         | the binary                           |
| `/usr/lib/systemd/system/rsdm.service`  | greeter unit (alias `display-manager`) |
| `/usr/lib/systemd/user/rsdm-idle.service` | optional per-user idle monitor       |
| `/etc/pam.d/rsdm`                       | greeter login PAM stack (unlocks the keyring) |
| `/etc/pam.d/rsdm-lock`                  | locker PAM stack (auth/account only) |
| `/etc/rsdm.toml`                 | configuration (a `backup` file)      |
| `/etc/logrotate.d/rsdm`                 | log rotation (only used in file mode)|

## Enable

1. Review and edit the config - the TTY and the session command especially:

   ```sh
   sudoedit /etc/rsdm.toml
   rsdm validate-config
   ```

   Set `[dm] fixed_session` (or leave it unset for the picker), and make sure
   `[dm.tty].path` is the VT you want the greeter to own. See
   [configuration.md](../configuration.md).

2. Enable the service (this conflicts with `getty@tty1`):

   ```sh
sudo systemctl enable --now rsdm.service
```

For built-in idle locking, set `[idle].enable = true` and enable its user unit
inside the desktop session:

```sh
systemctl --user enable --now rsdm-idle.service
```

3. Watch it:

   ```sh
   journalctl -u rsdm.service -b
   ```

## Dependencies

Runtime: `pam`, `systemd>=250`, `systemd-libs`, `wayland`, `libxkbcommon`,
`libsm`, `libice`, `gcc-libs`.
Build: `cargo`, `pkgconf`.

The session manager uses a logind login session and a user D-Bus. XSMP is
enabled by default; see [session-manager.md](../session-manager.md).

A Wayland session must be installed and discoverable. Picker mode reads
`.desktop` files from `/usr/share/wayland-sessions` (where compositors like niri,
Hyprland, sway, KDE, GNOME drop their session files).

## Keyrings

The greeter `rsdm` is the login, so it unlocks the keyring in the same PAM stack
that checks your password - just like `/etc/pam.d/login`. The shipped
`/etc/pam.d/rsdm` already includes `system-login` plus `optional` gnome-keyring
and kwallet lines, so those work as soon as the module is installed. For any
other keyring, edit those lines. See [keyrings.md](../keyrings.md).

## Notes

- The systemd unit must keep `StandardOutput=journal` / `StandardError=journal`
  so logs never land on the active TTY. `rsdm dm` refuses to start if stdout or
  stderr points at the live terminal.
- The lock screen is run from inside your session, not by the service. Bind
  `rsdm lock` to a compositor key - see [lock.md](../lock.md).
