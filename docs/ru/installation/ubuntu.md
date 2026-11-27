---
description: Сборка RSDM в Ubuntu или установка подходящего бинарного файла Linux из релиза.
---

# Ubuntu

Установите рабочий стол Wayland и убедитесь в доступности systemd/logind и пользовательской шины D-Bus. RSDM использует стек PAM в стиле Debian для Ubuntu.

::: code-group

```sh [Зависимости сборки]
sudo apt update
sudo apt install git cargo rustc gcc pkg-config libpam0g-dev \
  libwayland-dev libxkbcommon-dev libsm-dev libice-dev
```

```sh [Зависимости для релиза]
sudo apt update
sudo apt install libpam0g libwayland-client0 libxkbcommon0 libsm6 libice6
```

:::

CI и процесс выпуска RSDM ориентированы на Ubuntu 26.04. Для сборки требуется Rust 1.89 или новее; используйте актуальный [набор инструментов Rust](https://rustup.rs/), если компилятор дистрибутива старше. При старой glibc может потребоваться локальная сборка вместо готового архива.

## Получение бинарного файла {#obtain-the-binary}

Следуйте разделу [Сборка из исходного кода или использование релиза](manual.md#obtain-rsdm). Затем из корня рабочей копии или распакованного архива установите файлы политики PAM Debian/Ubuntu:

```sh
sudo install -Dm644 packaging/pam/debian/rsdm /etc/pam.d/rsdm
sudo install -Dm644 packaging/pam/debian/rsdm-lock /etc/pam.d/rsdm-lock
```

Выполните [установку общих файлов](manual.md#install-runtime-files), затем [активацию и восстановление](activate.md).

::: details Интеграция PAM
DM включает системные стеки `common-auth`, `common-account`, `common-password` и `common-session`. Lock включает только `common-auth` и `common-account`. Если обычный вход использует необязательные модули связок ключей, интегрируйте их в службу PAM DM в соответствующем порядке. См. [Связки ключей](../keyrings.md).
:::
