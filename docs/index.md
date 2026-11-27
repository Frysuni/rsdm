---
layout: home
title: Rust Display Manager for Wayland
description: Install, configure and use RSDM on Linux. PAM login, a terminal greeter, Wayland locking and systemd session management.

hero:
  name: RSDM
  text: Rust Display Manager
  tagline: A terminal Greeter, secure Wayland Lock and orderly session shutdown. One binary, independent components.
  actions:
    - theme: brand
      text: Get started
      link: /getting-started
    - theme: alt
      text: Configuration
      link: /configuration
    - theme: alt
      text: GitHub
      link: https://github.com/Frysuni/rsdm

features:
  - title: Install
    details: NixOS module, Arch AUR and manual installation for Fedora or Ubuntu.
    link: /getting-started#choose-your-distribution
  - title: DM & Greeter
    details: Choose a session, authenticate with PAM and customize your terminal login.
    link: /display-manager
  - title: Lock & Idle
    details: Lock compatible Wayland desktops, configure displays and automate inactivity handling.
    link: /lock
  - title: Session Manager
    details: Register apps, preserve save dialogs and coordinate logout, reboot and poweroff.
    link: /session-manager
  - title: Configuration
    details: Every TOML field, its default, accepted values and NixOS equivalents.
    link: /configuration
  - title: Troubleshooting
    details: Diagnose startup, authentication, session readiness and lock problems.
    link: /troubleshooting
---

<video src="/rsdm-preview.mp4" autoplay loop muted playsinline controls preload="metadata" aria-label="RSDM screen recording"></video>

## Choose your distribution

| Distribution | Recommended installation |
| --- | --- |
| [NixOS](installation/nixos.md) | Flake module with stable and unstable channels |
| [Arch Linux](installation/arch.md) | AUR source or prebuilt package |
| [Fedora](installation/fedora.md) | Build from source or install a release |
| [Ubuntu](installation/ubuntu.md) | Build from source or install a release |

RSDM is experimental. Check [compatibility](compatibility.md) before switching
your login stack.
