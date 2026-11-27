---
description: Сборка RSDM в Fedora или установка подходящего бинарного файла Linux из релиза.
---

# Fedora

Установите рабочий стол Wayland и убедитесь в доступности systemd/logind и пользовательской шины D-Bus. RSDM поставляет интеграцию PAM для Fedora.

::: code-group

```sh [Зависимости сборки]
sudo dnf install git rust cargo gcc pkgconf-pkg-config pam-devel \
  wayland-devel libxkbcommon-devel libSM-devel libICE-devel
```

```sh [Зависимости для релиза]
sudo dnf install pam systemd libwayland-client libxkbcommon libSM libICE
```

:::

Для сборки требуется Rust 1.89+. Документированная среда сборки — Fedora 44; охват проверок описан в [совместимости](../compatibility.md).

## Получение бинарного файла {#obtain-the-binary}

Следуйте разделу [Сборка из исходного кода или использование релиза](manual.md#obtain-rsdm). Затем из корня рабочей копии или распакованного архива установите файлы политики PAM Fedora:

```sh
sudo install -Dm644 packaging/pam/fedora/rsdm /etc/pam.d/rsdm
sudo install -Dm644 packaging/pam/fedora/rsdm-lock /etc/pam.d/rsdm-lock
```

Выполните [установку общих файлов](manual.md#install-runtime-files), затем [активацию и восстановление](activate.md).

::: details Интеграция PAM
Стек Greeter для Fedora включает службу `login` дистрибутива. Lock включает только части аутентификации и учётных записей из `system-auth`. Проверьте локальные модули authselect и связок ключей перед включением DM; см. [Связки ключей](../keyrings.md).
:::
