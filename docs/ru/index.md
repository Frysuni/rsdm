---
layout: home
title: Rust Display Manager для Wayland
description: Установка, настройка и использование RSDM в Linux. Вход через PAM, терминальный Greeter, блокировка Wayland и управление сеансами systemd.

hero:
  name: RSDM
  text: Rust Display Manager
  tagline: Терминальный Greeter, защищённая блокировка Wayland и корректное завершение сеанса. Один бинарный файл, независимые компоненты.
  actions:
    - theme: brand
      text: Начало работы
      link: /ru/getting-started
    - theme: alt
      text: Настройка
      link: /ru/configuration
    - theme: alt
      text: GitHub
      link: https://github.com/Frysuni/rsdm

features:
  - title: Установка
    details: Модуль NixOS, Arch AUR и ручная установка для Fedora или Ubuntu.
    link: /ru/getting-started#choose-your-distribution
  - title: DM и Greeter
    details: Выбор сеанса, аутентификация через PAM и настройка терминального экрана входа.
    link: /ru/display-manager
  - title: Lock и Idle
    details: Блокировка совместимых рабочих столов Wayland, настройка дисплеев и автоматическая блокировка при бездействии.
    link: /ru/lock
  - title: Session Manager
    details: Регистрация приложений, диалоги сохранения и координация выхода, перезагрузки и выключения.
    link: /ru/session-manager
  - title: Настройка
    details: Все поля TOML, значения по умолчанию, допустимые значения и эквиваленты для NixOS.
    link: /ru/configuration
  - title: Устранение неполадок
    details: Диагностика запуска, аутентификации, готовности сеанса и блокировки.
    link: /ru/troubleshooting
---

<video src="/rsdm-preview.mp4" autoplay loop muted playsinline controls preload="metadata" aria-label="Демонстрация RSDM"></video>

## Выберите дистрибутив {#choose-your-distribution}

| Дистрибутив | Рекомендуемый способ установки |
| --- | --- |
| [NixOS](installation/nixos.md) | Flake-модуль со стабильным и нестабильным каналами |
| [Arch Linux](installation/arch.md) | Пакет из AUR с исходным кодом или готовым бинарным файлом |
| [Fedora](installation/fedora.md) | Сборка из исходного кода или установка релиза |
| [Ubuntu](installation/ubuntu.md) | Сборка из исходного кода или установка релиза |

RSDM — экспериментальный проект. Перед сменой системы входа проверьте [совместимость](compatibility.md).
