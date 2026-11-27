# niri

Выберите niri в Greeter или задайте `dm.fixed_session = "niri-session"` (NixOS: `services.rsdm.dm.fixedSession`). Исходная desktop-запись обычно запускает niri-session, реальная служба `niri.service` которого владеет графическими targets.

## Сочетания клавиш и запуск приложений {#bind-keys-and-launch-apps}

Добавьте эти записи в существующий `~/.config/niri/config.kdl`:

```kdl
binds {
    Super+L { spawn "rsdm" "lock"; }
    Super+Shift+E { spawn "rsdm" "session" "stop"; }
}

spawn-at-startup "rsdm" "app" "--" "waybar"
```

Объедините сочетания с существующим блоком binds и замените конфликтующие клавиши. Сначала включите [Lock](../lock.md). Используйте `niri msg outputs` для определения имени разъёма дисплея.

## Готовность {#readiness}

Официальная обёртка niri-session не требует явного finalize. Для bare-сеанса niri опубликуйте его окружение через команду запуска:

```sh
rsdm session start -- niri -- rsdm session finalize
```

или конфигурацию:

```kdl
spawn-at-startup "rsdm" "session" "finalize"
```

## Idle

Включите [Idle](../idle.md); поставляемая пользовательская служба следует за graphical-session.target. Используйте один монитор бездействия. В bare niri выполните finalize перед использованием служб, зависящих от target.

См. [документацию niri](https://github.com/niri-wm/niri/wiki) и [бэкенды сеанса](../reference/session-backends.md#niri-window-close).
