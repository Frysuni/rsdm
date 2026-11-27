---
description: Unlock GNOME Keyring, KWallet or another PAM-backed keyring on login.
---

# Keyrings

RSDM uses your login PAM stack to unlock keyrings. The keyring's own PAM module receives the login credentials, starts its agent and publishes its environment. DM transfers that environment to the desktop.

## NixOS

```nix
services.rsdm.keyring = "auto";
```

| Value | Behavior |
| --- | --- |
| auto | Detect GNOME Keyring first, then KWallet |
| gnome | Select GNOME Keyring |
| kwallet | Select KWallet |
| none | Leave keyring PAM integration to your configuration |

The auto option checks existing PAM settings (and the GNOME Keyring service). Choose explicitly if both keyrings are installed. For a custom PAM module, select none and add the appropriate structured PAM auth/session rules for security.pam.services.rsdm.

## Other distributions

| Distribution | Shipped PAM stack |
| --- | --- |
| Arch | system-login plus optional GNOME Keyring and KWallet modules |
| Fedora | login (inherits the distribution policy) |
| Ubuntu / Debian | common-auth, common-account, common-password and common-session |

Install the appropriate keyring package and inspect /etc/pam.d/rsdm. Follow your distribution's normal keyring PAM integration, including the module name and order for its auth and session rules. Debian-style files may need explicit optional keyring hooks.

::: warning
PAM changes affect login. Keep another authenticated VT open while testing. Do not copy an entire stack from another distribution.
:::

::: details What happens during login
The PAM auth hook receives credentials; the session hook opens the keyring and starts its agent. pam_systemd registers the desktop login. RSDM uses the resulting PAM environment, including agent variables such as SSH_AUTH_SOCK where provided. Keyring shutdown is handled by the PAM module and desktop's lifetime policy. Lock uses a separate authentication-only PAM stack and does not open a new login session.
:::

## Troubleshoot

Verify the PAM module is installed and included in the login stack. The keyring's password must match the login credential for automatic unlock. Check the journal for PAM errors, and inspect whether the expected agent variables exist inside the desktop. Never publish their values if they contain sensitive information.

See [ArchWiki GNOME Keyring](https://wiki.archlinux.org/title/GNOME/Keyring), [KDE Wallet](https://wiki.archlinux.org/title/KDE_Wallet) and [security](SECURITY.md).
