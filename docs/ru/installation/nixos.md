---
description: Установка и настройка RSDM через flake-модуль NixOS.
---

# NixOS

Модуль устанавливает пакет, генерирует /etc/rsdm.toml, настраивает PAM и управляет units systemd. Сохраните существующую конфигурацию композитора.

## Добавление модуля {#add-the-module}

В системном flake добавьте input RSDM и модуль к существующему хосту.

```nix
{
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  inputs.rsdm.url = "github:Frysuni/rsdm";

  outputs = { nixpkgs, rsdm, ... }: {
    nixosConfigurations.host = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        ./configuration.nix
        rsdm.nixosModules.default
        {
          services.rsdm.enable = true;
        }
      ];
    };
  };
}
```

Замените host именем своей конфигурации и при необходимости выберите aarch64-linux. Отключите другой менеджер входа на том же VT, сохраните работу и оставьте доступной другую консоль.

```sh
sudo nixos-rebuild switch --flake .#host
rsdm validate-config
```

Выйдите из сеанса и перезагрузитесь в Greeter. Модуль намеренно избегает перезапуска DM при изменениях конфигурации, пока работает рабочий стол.

## Настройка компонентов {#configure-components}

Добавьте эти параметры в существующий модуль NixOS.

::: code-group

```nix [DM + Lock + Idle]
services.rsdm = {
  enable = true;
  dm = {
    fixedSession = "niri-session";
    design.theme = "catppuccin";
  };
  lock = {
    enable = true;
    design.theme = "catppuccin";
  };
  idle = {
    enable = true;
    timeout = 300;
  };
};
```

```nix [Только Lock + Idle]
services.rsdm = {
  enable = true;
  dm.enable = false;
  lock.enable = true;
  idle.enable = true;
};
```

:::

Все параметры и эквиваленты TOML см. в [справочнике конфигурации](../configuration.md). Для политики безопасности и других полей TOML используйте:

```nix
services.rsdm.extraConfig = {
  security.allowed_groups = [ "users" ];
  session_manager.ready_timeout_secs = 20;
};
```

::: details Как модуль связывает службы
- DM использует `rsdm.service` и настроенный VT. Модуль по умолчанию отключает его getty, автоматически создаёт псевдоним display-manager для tty1 и выбирает graphical.target.
- Стеки PAM генерируются для входа (rsdm) и Lock (rsdm-lock). Параметр keyring настраивает стек входа.
- При включении Idle становится пользовательской службой графического сеанса. Встроенный блокировщик требует Lock.
- При отключённом DM модуль не меняет цель загрузки, getty и роль display-manager.
- Необработанные значения рекурсивно объединяются после типизированных параметров. Устаревший псевдоним config объединяется перед extraConfig. Итоговые флаги включения DM/Lock/Idle и TTY также определяют настройки служб.
- Оформление Greeter и Lock независимо. Модуль по умолчанию задаёт passwordMode равным asterisks; в самостоятельной конфигурации TOML по умолчанию hidden. Сохранённые личные настройки Lock имеют приоритет для поддерживаемых визуальных полей.
:::

## Каналы и обновления {#channels-and-updates}

| Канал | Пакет |
| --- | --- |
| stable (по умолчанию) | Зафиксированный бинарный файл стабильного релиза GitHub |
| unstable | Сборка выбранной ревизии исходного кода |

```nix
services.rsdm.channel = "stable";
```

Из каталога своего flake:

```sh
nix flake update rsdm
sudo nixos-rebuild switch --flake .#host
```

Оба канала используют один input flake. Stable зафиксирован метаданными релиза в этом input; предварительные релизы его не обновляют. Модуль не добавляет кеши. Явное переопределение package имеет приоритет над channel.

::: details Прямые пакеты и миграция
Flake предоставляет rsdm-stable и rsdm-unstable (default — stable); rsdm-prebuilt и rsdm-source остаются псевдонимами. Сборка из исходного кода включает XSMP. Зависимости используют ваши существующие substituters или собираются локально.

При обновлении со старой конфигурации удалите `services.rsdm.binaryCache.enable` и старые настройки кеша RSDM Pages. Для выбора канала удалите переопределение `package`. При явном выборе старого пакета стиль рамки `none` требует RSDM 2.0.0 или новее.
:::

## Восстановление или удаление {#recover-or-remove}

Используйте другой VT и восстановите прежний менеджер входа в конфигурации NixOS. Пересоберите систему или выберите предыдущую рабочую генерацию NixOS в меню загрузки. Установка `dm.enable` в false сохраняет доступность остальных компонентов RSDM.

См. [Связки ключей](../keyrings.md), [Устранение неполадок](../troubleshooting.md) и [руководство NixOS](https://nixos.org/manual/nixos/stable/).
