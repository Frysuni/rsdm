---
layout: home
title: Rust Display Manager for Wayland
description: Learn how to install, configure and use RSDM, a standalone Rust display manager, TTY greeter and Wayland screen locker.

hero:
  name: RSDM
  text: Rust Display Manager
  tagline: PAM login, a terminal greeter, Wayland screen locking, idle handling and orderly session shutdown in one binary.
  actions:
    - theme: brand
      text: Installation guides
      link: '#choose-your-distribution'
    - theme: alt
      text: Configuration
      link: /configuration
    - theme: alt
      text: GitHub
      link: https://github.com/Frysuni/rsdm

features:
  - title: Install RSDM
    details: Follow distribution-specific instructions for NixOS, Arch Linux, Fedora and Ubuntu.
    link: '#choose-your-distribution'
  - title: DM and Greeter
    details: Configure the virtual terminal, PAM login, session selection and terminal appearance.
    link: /display-manager
  - title: Lock and idle
    details: Secure a compatible Wayland session and lock automatically after inactivity.
    link: /lock
  - title: Session management
    details: Launch applications and coordinate orderly logout, reboot and poweroff through systemd.
    link: /session-manager
  - title: Configuration reference
    details: Find TOML settings, design options and examples for each component.
    link: /configuration
  - title: Troubleshooting
    details: Inspect logs, validate settings and recover from common login and locking issues.
    link: /troubleshooting
---

[![RSDM in action with six views covering TTY login and session selection, Greeter design, Wayland locking and idle, Lock settings, registered apps, and reboot progress](/rsdm-preview.png)](/rsdm-preview.png)

## Choose your distribution

- [NixOS](installation/nixos.md) — flake module with stable and unstable channels.
- [Arch Linux](installation/arch.md) — AUR source and binary packages.
- [Fedora](installation/fedora.md) — dependencies and installation instructions.
- [Ubuntu](installation/ubuntu.md) — dependencies and installation instructions.

RSDM is experimental. See the [project compatibility table](https://github.com/Frysuni/rsdm#compatibility) for the current testing status and [security documentation](SECURITY.md) for runtime guarantees.
