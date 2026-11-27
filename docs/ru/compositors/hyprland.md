# Hyprland

Выберите Hyprland в Greeter или задайте `dm.fixed_session = "hyprland"` (NixOS: `services.rsdm.dm.fixedSession`). Используйте desktop-запись из установленного пакета. RSDM определяет, владеет ли исходный сеанс графическим target.

## Сочетания клавиш и запуск приложений {#bind-keys-and-launch-apps}

Используйте синтаксис установленной версии Hyprland. Объедините эти записи с существующей конфигурацией и избегайте дублирования сочетаний.

::: code-group

```lua [hyprland.lua]
hl.bind("SUPER + L", hl.dsp.exec_cmd("rsdm lock"))
hl.bind("SUPER + SHIFT + E", hl.dsp.exec_cmd("rsdm session stop"))

hl.on("hyprland.start", function()
    hl.exec_cmd("rsdm app -- waybar")
end)
```

```ini [Старый hyprland.conf]
bind = SUPER, L, exec, rsdm lock
bind = SUPER SHIFT, E, exec, rsdm session stop
exec-once = rsdm app -- waybar
```

:::

Включите [Lock](../lock.md). Если используете hypridle, сохраните существующую интеграцию жизненного цикла или перейдите на [RSDM Idle](../idle.md); избегайте дублирующих блокировщиков.

## Готовность {#readiness}

Для bare-сеанса Hyprland без нативной настройки графического target добавьте finalize после запуска композитора.

::: code-group

```lua [hyprland.lua]
hl.on("hyprland.start", function()
    hl.exec_cmd("rsdm session finalize")
end)
```

```ini [Старый hyprland.conf]
exec-once = rsdm session finalize
```

:::

Современный Lua IPC может обеспечивать проверенное закрытие окон приложений на поддерживаемых версиях. Старый IPC использует резервную политику приложений. Проверьте `rsdm session status`.

См. [вики Hyprland](https://wiki.hypr.land/) и [бэкенды сеанса](../reference/session-backends.md#hyprland-window-close).
