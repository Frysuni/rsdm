---
description: Build RSDM on Fedora or install the matching Linux release binary.
---

# Fedora

Install a Wayland desktop and ensure systemd/logind and the user D-Bus are
available. RSDM ships Fedora-specific PAM integration.

::: code-group

```sh [Build dependencies]
sudo dnf install git rust cargo gcc pkgconf-pkg-config pam-devel \
  wayland-devel libxkbcommon-devel libSM-devel libICE-devel
```

```sh [Release runtime]
sudo dnf install pam systemd libwayland-client libxkbcommon libSM libICE
```

:::

Rust 1.89+ is required for a source build. Fedora 44 is the documented build
environment; see [compatibility](../compatibility.md) for verification scope.

## Obtain the binary

Follow [Build from source or use a release](manual.md#obtain-rsdm).
Then, from the source checkout or extracted archive root, install the runtime
files for the Fedora PAM policy:

```sh
sudo install -Dm644 packaging/pam/fedora/rsdm /etc/pam.d/rsdm
sudo install -Dm644 packaging/pam/fedora/rsdm-lock /etc/pam.d/rsdm-lock
```

Follow [Install the common runtime files](manual.md#install-runtime-files),
then [Activation and recovery](activate.md).

::: details PAM integration
The Fedora Greeter stack includes the distribution's `login` service.
Lock includes only the authentication and account portions of `system-auth`.
Review locally configured authselect and keyring modules before enabling DM;
see [Keyrings](../keyrings.md).
:::
