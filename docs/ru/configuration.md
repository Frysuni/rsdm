---
description: Полный справочник TOML RSDM со значениями по умолчанию, допустимыми значениями и эквивалентами NixOS.
---

# Конфигурация {#configuration}

По умолчанию RSDM читает `/etc/rsdm.toml`. Все поля необязательны. Неизвестные ключи и недопустимые значения отклоняются. Изменения считываются при следующем запуске компонента; Session Manager также отслеживает изменения своей конфигурации во время работы.

```sh
rsdm validate-config
rsdm --config ./rsdm.toml validate-config
```

Используйте [полный пример с комментариями](https://github.com/Frysuni/rsdm/blob/main/rsdm.toml) как отправную точку. В NixOS задайте `services.rsdm` в системной конфигурации; модуль сгенерирует файл. Приведённые ниже параметры NixOS указаны относительно этого префикса.

## Компоненты {#components}

::: code-group

```toml [TOML]
[dm]
enable = true

[lock]
enable = true

[idle]
enable = true
timeout = 300

[session_manager]
enabled = true
```

```nix [NixOS]
services.rsdm = {
  enable = true;
  dm.enable = true;
  lock.enable = true;
  idle.enable = true;
  idle.timeout = 300;
  sessionManager = true;
};
```

:::

## DM и Greeter {#dm-and-greeter}

| Поле TOML | По умолчанию | NixOS |
| --- | --- | --- |
| `dm.enable` | `true` | `dm.enable` |
| `dm.pam_service` | `"rsdm"` | необработанная конфигурация |
| `dm.fixed_session` | не задано | `dm.fixedSession` |
| `dm.session_dirs` | системные пути (см. ниже) | `dm.sessionDirs` |
| `dm.tty.path` | `"/dev/tty1"` | `dm.tty` (например, `"tty1"`) |
| `dm.tty.seat` | `"seat0"` | `dm.seat` |
| `dm.remember.username` | `true` | `dm.remember.username` |
| `dm.remember.session` | `true` | `dm.remember.session` |
| `dm.fallback.enabled` | `true` | `dm.fallback.enable` |
| `dm.fallback.command` | `[]` | `dm.fallback.command` |

::: details Обнаружение сеансов и резервный вход
Каталоги сеансов просматриваются по порядку, по умолчанию /usr/local/share/wayland-sessions и /usr/share/wayland-sessions. NixOS по умолчанию добавляет в начало /run/current-system/sw/share/wayland-sessions; задайте `dm.sessionDirs` = null для общих значений по умолчанию. Пути должны быть абсолютными. Fixed session принимает desktop ID, уникальное имя/Exec или буквальную команду. При неоднозначных именах/Exec требуется точный desktop ID.

Используйте путь виртуального терминала Linux, например /dev/tty1. Настройте соответствующие службы. Параметры remember предварительно заполняют последний успешный вход и выбор сеанса. Пустой argv fallback автоматически обнаруживает системный getty. Явный argv интерпретируется буквально. Для fallback требуются deny_root = false, allowed_groups = [], рабочая конфигурация и свободный VT.
:::

См. [DM и Greeter](display-manager.md).

## Lock

| Поле TOML | По умолчанию | NixOS |
| --- | --- | --- |
| `lock.enable` | `false` | `lock.enable` |
| `lock.pam_service` | `"rsdm-lock"` | необработанная конфигурация |
| `lock.primary_output` | не задано (наибольшая площадь в пикселях) | `lock.primaryOutput` |
| `lock.secondary_output` | `"background"` | `lock.secondaryOutput` |
| `lock.size` | не задано (исходный размер) | `lock.size` |

Допустимые значения для дополнительных выходов: background, black и off (сейчас непрозрачный чёрный фон с включёнными выходами). Size — целое число от 1 до 12, где 1 соответствует исходному размеру глифа. См. [Lock](lock.md).

## Idle

| Поле TOML | По умолчанию | NixOS |
| --- | --- | --- |
| `idle.enable` | `false` | `idle.enable` |
| `idle.timeout` | `300` секунд | `idle.timeout` |
| `idle.ignore_inhibitors` | `false` | `idle.ignoreInhibitors` |
| `idle.lock_command` | `[]` | `idle.lockCommand` |
| `idle.on_lock` | `[]` | `idle.onLock` |
| `idle.on_unlock` | `[]` | `idle.onUnlock` |

Timeout принимает 1–4294967 секунд. Пустой lock_command выбирает встроенный Lock, который должен быть включён. Хуки — массивы массивов argv, например `[["notify-send", "Unlocked"]]`. См. [Idle](idle.md) для поведения готовности и ингибиторов.

## Оформление {#appearance}

У `dm.design` и `lock.design` независимые значения по умолчанию. NixOS использует те же префиксы с именами полей в camelCase.

| Поле | По умолчанию | Поле NixOS |
| --- | --- | --- |
| `theme` | `"monochrome"` | `theme` |
| `border_style` | `"classic"` | `borderStyle` |
| `background` | `"none"` | `background` |
| `background_speed` | `5` (0–10) | `backgroundSpeed` |
| `title_mode` | `"custom"` | `titleMode` |
| `title_text` | не задано (RSDM) | `titleText` |
| `title_preset` | `"rsdm"` | `titlePreset` |
| `title_font` | `"ANSI Shadow"` | `titleFont` |
| `show_clock` | `true` | `showClock` |
| `show_hostname` | `true` | `showHostname` |
| `password_mode` | `"hidden"` | `passwordMode` (по умолчанию asterisks) |
| `menu` | `true` | `menu` |

::: details Все варианты оформления
| Параметр | Значения |
| --- | --- |
| Темы | minimal, monochrome, cyberpunk, catppuccin, gruvbox, nord, dracula, tokyo-night, material, solarized, eldritch, rama, dark, trans-is-hard-job |
| Рамки | none, classic, modern, minimal, ascii1, ascii2, ascii3, ascii4, wave, pulse |
| Фоны | none, matrix, fire, rain, plasma, starfield |
| Режимы заголовка | custom, hostname, os-release, session-name, preset-logo |
| Готовые логотипы | minimal, block, rsdm, rustty, waytty, niri, hyprland, kde, gnome, nixos, arch, tux |
| Шрифты | имена шрифтов figrs/FIGlet |
| Пароль | hidden, asterisks |

Скорость 0 останавливает анимацию. Режим custom использует title_text; preset-logo использует title_preset. Menu включает F1.
:::

### Обои Lock {#lock-wallpaper}

| Поле в `lock.design` | По умолчанию | Поле NixOS |
| --- | --- | --- |
| `wallpaper` | не задано | `wallpaper` |
| `wallpaper_dim` | `6` | `wallpaperDim` |
| `background_opacity` | `8` | `backgroundOpacity` |

Wallpaper — абсолютный путь PNG/JPEG. Dim принимает значения от 0 (чёрный) до 10 (оригинал). Прозрачность фона принимает значения от 0 (только обои) до 10 (непрозрачный эффект) и игнорируется без обоев. Схема TOML принимает поля обоев в оформлении Greeter, где они не влияют на изображение.

### Личные настройки Lock {#personal-lock-settings}

F1 меняет текущее оформление в реальном времени. **Save settings** в Lock записывает личные переопределения в `$XDG_CONFIG_HOME/rsdm/lock.toml` (или `~/.config/rsdm/lock.toml`, если `XDG_CONFIG_HOME` не задан или является относительным путём). Переопределения загружаются после системной конфигурации.

::: details Что именно можно сохранить
Размер Lock, сброс размера, secondary_output и поля оформления theme, border_style, background, background_speed, title_font, wallpaper_dim и background_opacity. Аутентификация, PAM, путь к обоям, основной выход и другие административные настройки остаются в системном файле. Изменения Greeter действуют только для текущего экземпляра.
:::

Чтобы сбросить сохранённые настройки, переименуйте файл переопределений при разблокированном экране. Недопустимый файл мешает запуску Lock, поэтому при необходимости используйте другой VT.

```sh
mv "${XDG_CONFIG_HOME:-$HOME/.config}/rsdm/lock.toml" \
   "${XDG_CONFIG_HOME:-$HOME/.config}/rsdm/lock.toml.bak"
```

Если `XDG_CONFIG_HOME` относительный, явно используйте резервный путь.

## Session Manager {#session-manager}

| Поле TOML | По умолчанию | NixOS |
| --- | --- | --- |
| `session_manager.enabled` | `true` | `sessionManager` |
| `session_manager.extra_env` | `[]` | необработанная конфигурация |
| `session_manager.ready_timeout_secs` | `10` | необработанная конфигурация |

Extra_env задаёт имена дополнительных переменных для публикации при готовности. Тайм-аут неотрицателен, задан в секундах и должен помещаться в монотонный deadline. Тайм-аут готовности управляемого сеанса оставляет targets неактивными до finalize. См. [Session Manager](session-manager.md).

## Безопасность и пути {#security-and-paths}

| Поле TOML | По умолчанию | Значения |
| --- | --- | --- |
| `security.deny_root` | `true` | Логическое |
| `security.allowed_groups` | `[]` | Имена локальных групп (достаточно любой одной) |
| `security.max_failed_attempts` | `5` | 1–255 |
| `security.failure_delay_ms` | `1200` | Неотрицательные миллисекунды |
| `paths.cache_dir` | `"/var/cache/rsdm"` | Абсолютный путь к каталогу |

Параметры root/групп ограничивают входы DM. Greeter и Lock используют настроенный лимит попыток и задержку. В NixOS это необработанные настройки. См. [безопасность](SECURITY.md) для идентичности PAM, ограничений частоты и границ доверия.

## Журналирование {#logging}

| Поле TOML | По умолчанию | NixOS |
| --- | --- | --- |
| `logging.level` | `"info"` | `logging.level` |
| `logging.file` | не задано | `logging.file` |

Уровни: error, warn, info, debug и trace. Необязательный путь к файлу должен быть абсолютным, в доверенных каталогах, а сам файл — обычным, принадлежащим пользователю процесса, без совместной записи или символьных ссылок. Journald остаётся включённым. `RUST_LOG` переопределяет подробность отдельных запусков.

## Параметры модуля NixOS {#nixos-module-options}

Эти параметры управляют упаковкой и настройкой служб, а не исполняемой конфигурацией TOML.

| Параметр | По умолчанию | Назначение |
| --- | --- | --- |
| `enable` | `false` | Установить и настроить RSDM |
| `channel` | `"stable"` | stable или unstable |
| `package` | пакет канала | Переопределить выбранный пакет |
| `keyring` | `"auto"` | auto, gnome, kwallet или none |
| `disableGetty` | `true` | Освободить настроенный VT |
| `displayManagerAlias` | `null` | Автоматически для tty1, явное логическое переопределение |
| `extraConfig` | `{}` | Необработанные значения рекурсивно объединяются последними |
| `config` | `{}` | Устаревший псевдоним, объединяется перед extraConfig |

Например:

```nix
services.rsdm.extraConfig = {
  security.max_failed_attempts = 3;
  session_manager.extra_env = [ "MY_VARIABLE" ];
};
```

См. [руководство установки NixOS](installation/nixos.md) и [Связки ключей](keyrings.md).
