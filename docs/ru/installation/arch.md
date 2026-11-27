---
description: Установка RSDM из Arch User Repository или сборка пакета Arch.
---

# Arch Linux

Выберите один пакет AUR:

::: code-group

```sh [Готовый бинарный файл]
yay -S rsdm-bin
```

```sh [Исходный код]
yay -S rsdm
```

:::

[`rsdm-bin`](https://aur.archlinux.org/packages/rsdm-bin) устанавливает бинарный файл релиза. [`rsdm`](https://aur.archlinux.org/packages/rsdm) собирает релиз с тегом. Можно использовать другой AUR-помощник или клонировать выбранный репозиторий AUR и выполнить `makepkg -si` от обычного пользователя.

::: details Ручная сборка пакета AUR
```sh
git clone https://aur.archlinux.org/rsdm.git
cd rsdm
less PKGBUILD
makepkg -si
```

Шаблоны PKGBUILD в `packaging/arch` исходного репозитория используются при публикации релизов. Актуальную версию и хеши опубликованного пакета берите из самого репозитория AUR.
:::

## Настройка и активация {#configure-and-activate}

Пакет устанавливает бинарный файл, административную конфигурацию, стеки PAM, системные units DM и пользовательский unit Idle. DM автоматически не включается.

```sh
sudoedit /etc/rsdm.toml
rsdm validate-config
```

Продолжите по руководству [Активация и восстановление](activate.md), чтобы сменить прежний менеджер входа, выбрать VT или запустить только Lock/Idle.

## Обновление или удаление {#update-or-remove}

Для обновления используйте свой AUR-помощник. Проверьте файлы `.pacnew` для конфигурации и политики PAM, затем начните новый сеанс с обновлённым пакетом.

Перед удалением RSDM выполните [отключение DM](activate.md#disable-dm) и остановите пользовательскую службу Idle, если она включена. Затем удалите установленный пакет:

```sh
sudo pacman -R rsdm-bin
# или: sudo pacman -R rsdm
```

::: details Содержимое пакета и зависимости
Для работы: `pam`, `systemd>=250`, `wayland`, `libxkbcommon`, `libsm`, `libice`, `gcc-libs`. Для сборки также нужны Cargo и pkgconf.

Пакет устанавливает `/usr/bin/rsdm`, `/usr/lib/systemd/system/rsdm.service`, `rsdm@.service`, `/usr/lib/systemd/user/rsdm-idle.service`, `/etc/pam.d/rsdm`, `/etc/pam.d/rsdm-lock`, `/etc/rsdm.toml`, правило logrotate, desktop-файл авторизации KWin и руководство. См. [руководство по связкам ключей](../keyrings.md) для необязательных записей PAM GNOME Keyring и KWallet в Arch.
:::
