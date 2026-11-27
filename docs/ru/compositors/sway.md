# Sway

Выберите Sway в Greeter или задайте `dm.fixed_session = "sway"` (NixOS: `services.rsdm.dm.fixedSession`).

## Сочетания клавиш и публикация готовности {#bind-keys-and-publish-readiness}

Добавьте в существующий `~/.config/sway/config` (предполагается, что $mod уже определён):

```ini
exec rsdm session finalize
bindsym $mod+l exec rsdm lock
bindsym $mod+Shift+e exec rsdm session stop
exec rsdm app -- waybar
```

Замените конфликтующие сочетания и записи автозапуска. Сначала включите [Lock](../lock.md). Finalize публикует окружение композитора, включая `SWAYSOCK`, для bare-сеанса.

```sh
sway --validate
swaymsg -t get_outputs
rsdm session status
```

## Idle

Используйте [RSDM Idle](../idle.md) или существующую интеграцию swayidle. Если сохраняете swayidle, используйте foreground-блокировщик и интеграцию сна с подтверждением готовности. Избегайте одновременного запуска двух автоматических блокировщиков.

Для управляемых сеансов Sway проверенные нативные окна приложений Wayland могут получать запросы закрытия. Окна XWayland используют другие применимые политики приложений.

См. [sway(5)](https://github.com/swaywm/sway/blob/master/sway/sway.5.scd) и [бэкенды сеанса](../reference/session-backends.md#sway-window-close).
