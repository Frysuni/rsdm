---
description: Install and configure RSDM using its NixOS flake module.
---

# NixOS

The module installs the package, generates /etc/rsdm.toml, configures PAM and manages the systemd units. Keep your existing compositor configuration.

## Add the module

In your system flake, add the RSDM input and module to your existing host.

```nix
{
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  inputs.rsdm.url = "github:Frysuni/rsdm";

  outputs = { nixpkgs, rsdm, ... }: {
    nixosConfigurations.host = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        ./configuration.nix
        rsdm.nixosModules.default
        {
          services.rsdm.enable = true;
        }
      ];
    };
  };
}
```

Replace host with your configuration name and select aarch64-linux if appropriate. Disable any other display manager on the same VT, save your work and keep another console available.

```sh
sudo nixos-rebuild switch --flake .#host
rsdm validate-config
```

Log out and reboot into the Greeter. The module deliberately avoids restarting DM on configuration changes while a desktop is running.

## Configure components

Add these options to an existing NixOS module.

::: code-group

```nix [DM + Lock + Idle]
services.rsdm = {
  enable = true;
  dm = {
    fixedSession = "niri-session";
    design.theme = "catppuccin";
  };
  lock = {
    enable = true;
    design.theme = "catppuccin";
  };
  idle = {
    enable = true;
    timeout = 300;
  };
};
```

```nix [Lock + Idle only]
services.rsdm = {
  enable = true;
  dm.enable = false;
  lock.enable = true;
  idle.enable = true;
};
```

:::

Use the [configuration reference](../configuration.md) for all settings and TOML equivalents. To set security policy or other raw TOML fields, use:

```nix
services.rsdm.extraConfig = {
  security.allowed_groups = [ "users" ];
  session_manager.ready_timeout_secs = 20;
};
```

::: details How the module wires services
- DM uses `rsdm.service` and the configured VT. The module disables its getty by default, creates the display-manager alias automatically for tty1, and selects graphical.target.
- PAM stacks are generated for login (rsdm) and Lock (rsdm-lock). The keyring setting configures the login stack.
- Idle is a graphical user service when enabled. The built-in locker requires Lock.
- With DM disabled the module leaves the boot target, getty and display-manager role alone.
- Raw values merge recursively after typed options. The resulting DM/Lock/Idle enable flags and TTY determine service wiring too.
- Greeter and Lock designs are independent. The module defaults passwordMode to asterisks; the standalone TOML default is hidden. Saved personal Lock settings take precedence for supported visual fields.
:::

## Channels and updates

| Channel | Package |
| --- | --- |
| stable (default) | Pinned stable GitHub Release binary |
| unstable | Builds the selected source revision |

```nix
services.rsdm.channel = "stable";
```

From your flake directory:

```sh
nix flake update rsdm
sudo nixos-rebuild switch --flake .#host
```

Both channels use the same flake input. Stable is pinned by the release metadata in that input; prereleases do not advance it. The module does not add caches. An explicit package override takes precedence over channel.

::: details Direct packages and migration
The flake exposes rsdm-stable and rsdm-unstable (default is stable). Source builds enable XSMP. Dependencies use your existing substituters or are built locally.

Remove the `package` override to use channel selection.
:::

## Recover or remove

Use another VT and restore your previous display manager in the NixOS configuration. Rebuild, or select a previous working NixOS generation from the boot menu. Setting `dm.enable` = false keeps the other RSDM components available.

See [keyrings](../keyrings.md), [troubleshooting](../troubleshooting.md) and the [NixOS manual](https://nixos.org/manual/nixos/stable/).
