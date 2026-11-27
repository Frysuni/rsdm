---
description: Ручная установка бинарных файлов RSDM, units systemd и файлов PAM для дистрибутива.
---

# Ручная установка {#manual-installation}

Используйте это руководство в [Fedora](fedora.md), [Ubuntu](ubuntu.md) или другом совместимом дистрибутиве Linux. Для Arch и NixOS есть отдельные инструкции по пакетам и модулю. Сначала установите зависимости своего дистрибутива.

## Получение RSDM {#obtain-rsdm}

::: code-group

```sh [Исходный код]
git clone https://github.com/Frysuni/rsdm.git
cd rsdm
# При необходимости переключитесь на тег нужного релиза перед сборкой.
cargo build --release --locked -p rsdm
```

```sh [Архив релиза]
# Скачайте соответствующие .tar.gz и .tar.gz.sha256 из GitHub Releases.
# Выполните в каталоге с этими файлами; при необходимости замените 2.5.0.
archive=rsdm-2.5.0-x86_64-unknown-linux-gnu.tar.gz
sha256sum -c "$archive.sha256" && \
  mkdir rsdm-release && \
  tar -xzf "$archive" -C rsdm-release && \
  cd rsdm-release
```

:::

[Скачать релизы](https://github.com/Frysuni/rsdm/releases). Для ARM64 используйте архив `aarch64-unknown-linux-gnu`. Релиз включает бинарный файл, пример конфигурации и файлы упаковки. При сборке бинарный файл находится в `target/release/rsdm`.

::: warning Обновление существующей установки
Сделайте резервные копии `/etc/rsdm.toml` и `/etc/pam.d/rsdm*`. Команды установки заменяют файлы по этим путям; объедините локальную конфигурацию с новыми примерами. Обновляйтесь после выхода из старого сеанса RSDM или перезагрузитесь в обновлённую версию.
:::

## Установка файлов для работы {#install-runtime-files}

Выполните команды из корня рабочей копии исходного кода или распакованного релиза.

::: code-group

```sh [Собранный бинарный файл]
sudo install -Dm755 target/release/rsdm /usr/bin/rsdm
```

```sh [Бинарный файл релиза]
sudo install -Dm755 rsdm /usr/bin/rsdm
```

:::

Установите общие файлы:

```sh
sudo install -Dm644 rsdm.toml /etc/rsdm.toml
sudo install -Dm644 packaging/systemd/rsdm.service /etc/systemd/system/rsdm.service
sudo install -Dm644 packaging/systemd/rsdm@.service /etc/systemd/system/rsdm@.service
sudo install -Dm644 packaging/systemd/rsdm-idle.service /usr/lib/systemd/user/rsdm-idle.service
sudo install -Dm644 packaging/applications/org.rsdm.SessionManager.desktop \
  /usr/share/applications/org.rsdm.SessionManager.desktop
sudo systemctl daemon-reload
```

Используйте файлы PAM из руководства [Fedora](fedora.md) или [Ubuntu](ubuntu.md). В другом дистрибутиве адаптируйте его обычные стеки входа и аутентификации к требованиям из раздела [Безопасность](../SECURITY.md). Не копируйте вслепую включения PAM из другого дистрибутива.

Для необязательного файлового журнала при установленном logrotate:

```sh
sudo install -Dm644 packaging/logrotate/rsdm /etc/logrotate.d/rsdm
```

Поставляемое правило охватывает `/var/log/rsdm/*.log`; измените его, если настроили другой путь. При стандартном логировании только в journal правило не требуется.

## Активация или удаление {#activate-or-uninstall}

Продолжите по руководству [Активация и восстановление](activate.md).
::: details Удаление ручной установки
Сначала выполните [отключение DM](activate.md#disable-dm), включая остановку пользовательской службы Idle, если она включена. Выйдите из сеансов RSDM перед удалением бинарного файла. Для описанной здесь ручной установки удалите установленные файлы:

```sh
sudo rm -f /usr/bin/rsdm \
  /etc/systemd/system/rsdm.service \
  /etc/systemd/system/rsdm@.service \
  /usr/lib/systemd/user/rsdm-idle.service \
  /usr/share/applications/org.rsdm.SessionManager.desktop \
  /etc/logrotate.d/rsdm
sudo systemctl daemon-reload
systemctl --user daemon-reload
```

Сохраните `/etc/rsdm.toml`, файлы PAM RSDM и сохранённое состояние, если планируете переустановку. Удалите их отдельно, только когда они больше не нужны. При установке из пакета используйте менеджер пакетов дистрибутива.
:::

::: details Нестандартные пути исполняемого файла
При установке вне `/usr/bin` обновите `ExecStart` обоих units DM и unit Idle, а также `Exec` в desktop-файле приложения. Desktop-файл разрешает интерфейс управления окнами standalone KWin; исполняемый файл должен соответствовать установленному бинарному файлу.
:::
