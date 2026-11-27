# Hyprland

Choose Hyprland in the Greeter, or set `dm.fixed_session = "hyprland"` (NixOS: `services.rsdm.dm.fixedSession`). Use the desktop entry installed by your package. RSDM detects whether the original session owns the graphical target.

## Bind keys and launch apps

Use the syntax for your installed Hyprland version. Merge these into your existing configuration and avoid duplicate bindings.

::: code-group

```lua [hyprland.lua]
hl.bind("SUPER + L", hl.dsp.exec_cmd("rsdm lock"))
hl.bind("SUPER + SHIFT + E", hl.dsp.exec_cmd("rsdm session stop"))

hl.on("hyprland.start", function()
    hl.exec_cmd("rsdm app -- waybar")
end)
```

```ini [Legacy hyprland.conf]
bind = SUPER, L, exec, rsdm lock
bind = SUPER SHIFT, E, exec, rsdm session stop
exec-once = rsdm app -- waybar
```

:::

Enable [Lock](../lock.md). If you use hypridle, retain its existing lifecycle integration or switch to [RSDM Idle](../idle.md); avoid duplicate lockers.

## Readiness

For a bare Hyprland session without native graphical target setup, add finalize after the compositor starts.

::: code-group

```lua [hyprland.lua]
hl.on("hyprland.start", function()
    hl.exec_cmd("rsdm session finalize")
end)
```

```ini [Legacy hyprland.conf]
exec-once = rsdm session finalize
```

:::

Current Lua IPC can provide verified app window close on supported versions. Older IPC falls back to the application policy. Check `rsdm session status`.

See the [Hyprland wiki](https://wiki.hypr.land/) and [session backends](../reference/session-backends.md#hyprland-window-close).
