# Installation: Ubuntu

The source and release binary build on Ubuntu 26.04 LTS. For a source build:

```sh
sudo apt update
sudo apt install cargo rustc gcc pkg-config libpam0g-dev libwayland-dev \
  libxkbcommon-dev libsm-dev libice-dev
cargo build --release --locked -p rsdm
```

rsdm requires Rust 1.88 or newer. On an older supported Ubuntu release whose
archive Rust is older, install a current Rust toolchain with rustup and keep the
same system development packages.

The session manager requires systemd 250 or newer, a logind login session and
a user D-Bus. Release binaries also need `libsm6` and `libice6` at runtime.
XSMP is enabled by default; see [session-manager.md](../session-manager.md).

Install the runtime files from the repository root:

```sh
sudo install -Dm755 target/release/rsdm /usr/bin/rsdm
sudo install -Dm644 rsdm.toml /etc/rsdm.toml
sudo install -Dm644 packaging/pam/debian/rsdm /etc/pam.d/rsdm
sudo install -Dm644 packaging/pam/debian/rsdm-lock /etc/pam.d/rsdm-lock
sudo install -Dm644 packaging/systemd/rsdm.service \
  /etc/systemd/system/rsdm.service
sudo install -Dm644 packaging/systemd/rsdm-idle.service \
  /usr/lib/systemd/user/rsdm-idle.service
sudo systemctl daemon-reload
systemctl --user daemon-reload
```

Review PAM and `/etc/rsdm.toml`, run `rsdm validate-config`, disable any
other display manager, then enable `rsdm.service`. Do not enable an unreviewed
PAM/display-manager configuration on a remote-only machine.

For idle locking, set `[idle].enable = true`, then run
`systemctl --user enable --now rsdm-idle.service` inside the desktop session.

The Ubuntu PAM files use the distribution's `common-*` policy. Add the same
optional keyring modules used by the system's normal login stack if automatic
keyring unlock is desired.
