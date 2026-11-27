---
description: Включение, переключение и безопасное отключение служб systemd RSDM в дистрибутивах, отличных от NixOS.
---

# Активация и восстановление {#activation-and-recovery}

Для RSDM, установленного из пакета или вручную, в дистрибутивах с systemd. В NixOS используйте [модуль](nixos.md).

## Подготовка {#prepare}

```sh
sudoedit /etc/rsdm.toml
rsdm validate-config
```

Проверьте `[dm.tty].path`, свой `[dm].fixed_session` (или установленные записи сеансов Wayland) и службу PAM для DM. Сохраните рабочий вход на другом VT, например tty3.

::: warning Смена менеджера входа
Сохраните работу, выйдите из сеанса и используйте другой VT. Определите старый DM перед отключением. Переключение активной службы может завершить её графический сеанс. Не проверяйте новую конфигурацию PAM на хосте, доступном только удалённо.
:::

## Включение DM {#enable-dm}

Проверьте существующий псевдоним display-manager:

```sh
systemctl status display-manager.service
systemctl get-default
```

Если псевдоним принадлежит другому менеджеру, сначала отключите его фактический unit (например, `sudo systemctl disable --now sddm.service`, если используете SDDM). Затем включите RSDM:

```sh
sudo systemctl enable rsdm.service
sudo systemctl set-default graphical.target
sudo systemctl start rsdm.service
```

Стандартный unit использует tty1 и устанавливает псевдоним `display-manager.service`. Greeter должен появиться на tty1. Если этого не произошло, проверьте `journalctl -b -u rsdm.service`.

::: details Использование другого виртуального терминала
Укажите соответствующий путь в конфигурации:

```toml
[dm.tty]
path = "/dev/tty2"
seat = "seat0"
```

Используйте установленный шаблон вместо службы tty1:

```sh
sudo systemctl enable --now rsdm@tty2.service
```

Включайте только один экземпляр RSDM с этой общей конфигурацией. У шаблона нет псевдонима display-manager; VT и маски getty определяются именем экземпляра. Остановите экземпляр перед сменой VT или удалением unit.
:::

## Lock и Idle без DM {#use-lock-and-idle-without-dm}

Можно оставить DM отключённым и использовать RSDM внутри другого сеанса Wayland.

```toml
[dm]
enable = false

[lock]
enable = true

[idle]
enable = true
timeout = 300
```

Внутри графического сеанса:

```sh
rsdm lock
systemctl --user daemon-reload
systemctl --user enable --now rsdm-idle.service
```

Сначала разблокируйте экран, затем включите монитор Idle. Ему нужен совместимый композитор и его graphical-session target. Другие способы запуска описаны в [Idle](../idle.md). Session Manager настраивается независимо.

## Отключение DM {#disable-dm}

Выйдите из графического сеанса, переключитесь на рабочий VT и выполните:

```sh
sudo systemctl disable --now rsdm.service
# Если использовался шаблон:
# sudo systemctl disable --now rsdm@tty2.service
```

При остановке поставляемый unit снимает свои runtime-маски getty/autovt. Если планируете использовать консольный вход на tty1, проверьте и запустите его getty:

```sh
sudo systemctl start getty@tty1.service
```

При необходимости восстановите службу прежнего DM или задайте `sudo systemctl set-default multi-user.target` для загрузки в текстовом режиме. Чтобы отключить Idle отдельно, выполните `systemctl --user disable --now rsdm-idle.service` внутри сеанса.

См. [Устранение неполадок](../troubleshooting.md) для ошибок и аварийной разблокировки.
