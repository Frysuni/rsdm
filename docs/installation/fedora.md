# Installation: Fedora

The source and release binary build on Fedora Linux 44 with the distribution
toolchain:

```sh
sudo dnf install rust cargo gcc pkgconf-pkg-config pam-devel wayland-devel \
  libxkbcommon-devel libSM-devel libICE-devel
cargo build --release --locked -p rsdm
```

The session manager requires systemd 250 or newer, a logind login session and
a user D-Bus. Release binaries also need `libSM` and `libICE` at runtime.
XSMP is enabled by default; see [session-manager.md](../session-manager.md).

Install the runtime files from the repository root:

```sh
sudo install -Dm755 target/release/rsdm /usr/bin/rsdm
sudo install -Dm644 rsdm.toml /etc/rsdm.toml
sudo install -Dm644 packaging/pam/fedora/rsdm /etc/pam.d/rsdm
sudo install -Dm644 packaging/pam/fedora/rsdm-lock /etc/pam.d/rsdm-lock
sudo install -Dm644 packaging/systemd/rsdm.service \
  /etc/systemd/system/rsdm.service
sudo install -Dm644 packaging/systemd/rsdm-idle.service \
  /usr/lib/systemd/user/rsdm-idle.service
sudo systemctl daemon-reload
systemctl --user daemon-reload
```

The Fedora greeter PAM service includes the system `login` policy, preserving
authselect, pam_systemd, limits, and configured keyring hooks. The locker uses
only `system-auth` authentication/account checks and opens no session.

Review `/etc/rsdm.toml`, run `rsdm validate-config`, disable any other
display manager, then enable `rsdm.service`. Do not enable an unreviewed
PAM/display-manager configuration on a remote-only machine.

For idle locking, set `[idle].enable = true`, then run
`systemctl --user enable --now rsdm-idle.service` inside the desktop session.
