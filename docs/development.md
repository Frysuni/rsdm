---
description: Build RSDM, run checks, preview the wiki and contribute to the project.
---

# Development

The Rust workspace targets Linux and declares Rust 1.89 as its minimum supported version. See [architecture](ARCHITECTURE.md) for crate boundaries.

## Build locally

::: code-group

```sh [Nix development shell]
nix develop
cargo build --locked -p rsdm
```

```sh [Ubuntu / Debian]
sudo apt install build-essential pkg-config libpam0g-dev libwayland-dev \
  libxkbcommon-dev libsm-dev libice-dev
cargo build --locked -p rsdm
```

:::

Use the equivalent development packages on [Fedora](installation/fedora.md) or [Arch Linux](installation/arch.md). Default builds include XSMP via libSM/libICE. An intentional build without XSMP uses cargo build --locked -p rsdm --no-default-features.

The debug binary is target/debug/rsdm; release builds use --release.

## Validation

```sh
cargo test --workspace --locked
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked
```

::: details Nix checks and isolated VM fixtures
```sh
nix flake check --all-systems --no-build
nix build .#rsdm-unstable
nix build .#checks.x86_64-linux.session-lifecycle
```

The check names are declared in flake.nix. Select the corresponding architecture where supported.

| Area | Check names |
| --- | --- |
| Nix module | module |
| Session startup / attach / ownership | session-lifecycle, session-attach, scope-lifetime, native-niri |
| Window handling | sway-windows, browser-windows, flatpak-windows, media-windows, hyprland-windows, kwin-windows |
| Discovery | wayfire-discovery, wayland-discovery |
| Native desktop delegation | native-de-handoff, native-attached, native-full-scope |
| Power / login | power-lifecycle, lock-power, dm-lifecycle, dm-recovery, dm-cancel |

VM builds require substantial resources and KVM support for normal performance. These fixtures verify isolated behavior rather than every hardware and compositor combination.
:::

Use the tracked [CI policy](https://github.com/Frysuni/rsdm/blob/main/.github/CONTRIBUTING.md). CI covers Rust tests, minimum Rust version, dependency auditing, source/prebuilt packaging, Nix evaluation and isolated fixtures. Do not introduce rustfmt/cargo fmt or actionlint as workflow checks.

## Documentation

The wiki lives in docs/ and uses VitePress, Node 24 and the committed npm lockfile.

```sh
cd docs
npm ci
npm run dev
```

Open the URL printed by VitePress. Check the production build before publishing changes:

```sh
npm run build
npm run preview
```

Use relative Markdown links, small practical examples, code groups for TOML/NixOS alternatives and details blocks for advanced behavior. Keep examples consistent with rsdm.toml, CLI help and current implementation. See [VitePress Markdown extensions](https://vitepress.dev/guide/markdown).

English pages live in the root of docs/ and Russian pages in docs/ru/ with the same paths. When adding a page or changing headings, update both versions and preserve matching heading IDs. Run `npm test` to check translation coverage and language selection.

GitHub Pages is built by the documentation workflow; PRs build the site and main deployments publish it.

## Contribute

Keep changes focused. Explain the behavior change and checks run in a pull request. Update the example configuration and relevant guides when a setting changes.

For security-sensitive changes, read [security](SECURITY.md). The [historical audit](audit-remediation.md) records an older remediation effort.
