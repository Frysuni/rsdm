---
description: Build RSDM on Ubuntu or install the matching Linux release binary.
---

# Ubuntu

Install a Wayland desktop and ensure systemd/logind and the user D-Bus are
available. RSDM uses the Debian-style PAM stack on Ubuntu.

::: code-group

```sh [Build dependencies]
sudo apt update
sudo apt install git cargo rustc gcc pkg-config libpam0g-dev \
  libwayland-dev libxkbcommon-dev libsm-dev libice-dev
```

```sh [Release runtime]
sudo apt update
sudo apt install libpam0g libwayland-client0 libxkbcommon0 libsm6 libice6
```

:::

RSDM's CI and release workflow target Ubuntu 26.04. A source build needs
Rust 1.89 or newer; use a current [Rust toolchain](https://rustup.rs/) if your
distribution's compiler is older. An older glibc may require building
locally instead of using a prebuilt archive.

## Obtain the binary

Follow [Build from source or use a release](manual.md#obtain-rsdm).
Then, from the source checkout or extracted archive root, install the runtime
files for the Debian/Ubuntu PAM policy:

```sh
sudo install -Dm644 packaging/pam/debian/rsdm /etc/pam.d/rsdm
sudo install -Dm644 packaging/pam/debian/rsdm-lock /etc/pam.d/rsdm-lock
```

Follow [Install the common runtime files](manual.md#install-runtime-files),
then [Activation and recovery](activate.md).

::: details PAM integration
DM includes the system's `common-auth`, `common-account`,
`common-password` and `common-session` stacks. Lock includes only
`common-auth` and `common-account`. If your normal login uses optional
keyring modules, integrate them into DM's PAM service in the appropriate
order. See [Keyrings](../keyrings.md).
:::
