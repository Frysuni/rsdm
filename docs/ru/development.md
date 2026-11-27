---
description: Сборка RSDM, запуск проверок, просмотр вики и участие в проекте.
---

# Разработка {#development}

Рабочее пространство Rust ориентировано на Linux; минимальная поддерживаемая версия Rust — 1.89. Границы крейтов описаны в [архитектуре](ARCHITECTURE.md).

## Локальная сборка {#build-locally}

::: code-group

```sh [Среда разработки Nix]
nix develop
cargo build --locked -p rsdm
```

```sh [Ubuntu / Debian]
sudo apt install build-essential pkg-config libpam0g-dev libwayland-dev \
  libxkbcommon-dev libsm-dev libice-dev
cargo build --locked -p rsdm
```

:::

Используйте эквивалентные пакеты разработки в [Fedora](installation/fedora.md) или [Arch Linux](installation/arch.md). Сборки по умолчанию включают XSMP через libSM/libICE. Для намеренной сборки без XSMP используйте cargo build --locked -p rsdm --no-default-features.

Отладочный бинарный файл — target/debug/rsdm; сборки релизов используют --release.

## Проверки {#validation}

```sh
cargo test --workspace --locked
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked
```

::: details Проверки Nix и изолированные тестовые ВМ
```sh
nix flake check --all-systems --no-build
nix build .#rsdm-unstable
nix build .#checks.x86_64-linux.session-lifecycle
```

Имена проверок объявлены в flake.nix. Выбирайте соответствующую архитектуру, где она поддерживается.

| Область | Имена проверок |
| --- | --- |
| Модуль Nix | module |
| Запуск / подключение / владение сеансом | session-lifecycle, session-attach, scope-lifetime, native-niri |
| Обработка окон | sway-windows, browser-windows, flatpak-windows, media-windows, hyprland-windows, kwin-windows |
| Обнаружение | wayfire-discovery, wayland-discovery |
| Нативное делегирование рабочему столу | native-de-handoff, native-attached, native-full-scope |
| Питание / вход | power-lifecycle, lock-power, dm-lifecycle, dm-recovery, dm-cancel |

Сборки ВМ требуют значительных ресурсов и поддержки KVM для нормальной производительности. Эти тестовые сценарии проверяют изолированное поведение, а не все сочетания оборудования и композиторов.
:::

Следуйте [политике CI](https://github.com/Frysuni/rsdm/blob/main/.github/CONTRIBUTING.md). CI охватывает тесты Rust, минимальную версию Rust, аудит зависимостей, упаковку исходного кода/готовых файлов, вычисление Nix и изолированные сценарии. Не добавляйте rustfmt/cargo fmt или actionlint в проверки workflow.

## Документация {#documentation}

Вики находится в docs/ и использует VitePress, Node 24 и зафиксированный npm lockfile.

```sh
cd docs
npm ci
npm run dev
```

Откройте URL, который выведет VitePress. Проверьте production-сборку перед публикацией изменений:

```sh
npm run build
npm run preview
```

Используйте относительные ссылки Markdown, небольшие практические примеры, группы кода для вариантов TOML/NixOS и блоки details для расширенного описания. Согласуйте примеры с rsdm.toml, справкой CLI и текущей реализацией. См. [расширения Markdown VitePress](https://vitepress.dev/guide/markdown).

Английские страницы находятся в корне docs/, русские — в docs/ru/ с теми же путями. При добавлении страницы или изменении заголовков обновляйте обе версии и сохраняйте общие идентификаторы заголовков. Команда `npm test` проверяет наличие переводов и выбор языка.

GitHub Pages собирается workflow документации; PR собирают сайт, а публикации main размещают его.

## Участие в проекте {#contribute}

Ограничивайте изменения конкретной задачей. Описывайте изменения поведения и выполненные проверки в pull request. Обновляйте пример конфигурации и соответствующие руководства при смене параметра.

Для изменений, затрагивающих безопасность, прочитайте [безопасность](SECURITY.md).
