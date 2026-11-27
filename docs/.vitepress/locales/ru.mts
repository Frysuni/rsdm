import type { DefaultTheme, LocaleConfig } from 'vitepress'

const repository = 'https://github.com/Frysuni/rsdm'

export const ru: LocaleConfig<DefaultTheme.Config>[string] = {
  label: 'Русский',
  lang: 'ru-RU',
  link: '/ru/',
  title: 'Вики RSDM',
  description: 'Установка, настройка и использование RSDM — дисплейного менеджера на Rust для Wayland.',
  markdown: {
    container: {
      infoLabel: 'ИНФОРМАЦИЯ',
      noteLabel: 'ПРИМЕЧАНИЕ',
      tipLabel: 'СОВЕТ',
      warningLabel: 'ВНИМАНИЕ',
      dangerLabel: 'ОПАСНОСТЬ',
      detailsLabel: 'Подробности',
      importantLabel: 'ВАЖНО',
      cautionLabel: 'ОСТОРОЖНО',
    },
    codeCopyButton: { tooltipText: 'Копировать код', copiedText: 'Скопировано' },
  },
  themeConfig: {
    nav: [
      { text: 'Начало работы', link: '/ru/getting-started' },
      { text: 'Настройка', link: '/ru/configuration' },
      { text: 'Команды', link: '/ru/README' },
      {
        text: 'Проект',
        items: [
          { text: 'Разработка', link: '/ru/development' },
          { text: 'Релизы', link: `${repository}/releases` },
        ],
      },
    ],
    sidebar: [
      {
        text: 'Начало работы',
        items: [
          { text: 'Обзор', link: '/ru/' },
          { text: 'Первый запуск', link: '/ru/getting-started' },
          { text: 'Совместимость', link: '/ru/compatibility' },
        ],
      },
      {
        text: 'Установка',
        collapsed: false,
        items: [
          { text: 'NixOS', link: '/ru/installation/nixos' },
          { text: 'Arch Linux', link: '/ru/installation/arch' },
          { text: 'Fedora', link: '/ru/installation/fedora' },
          { text: 'Ubuntu', link: '/ru/installation/ubuntu' },
          { text: 'Ручная установка', link: '/ru/installation/manual' },
          { text: 'Активация и восстановление', link: '/ru/installation/activate' },
        ],
      },
      {
        text: 'Использование RSDM',
        collapsed: false,
        items: [
          { text: 'DM и Greeter', link: '/ru/display-manager' },
          { text: 'Экран блокировки', link: '/ru/lock' },
          { text: 'Блокировка при бездействии', link: '/ru/idle' },
          { text: 'Session Manager', link: '/ru/session-manager' },
          { text: 'Связки ключей', link: '/ru/keyrings' },
        ],
      },
      {
        text: 'Композиторы',
        collapsed: true,
        items: [
          { text: 'niri', link: '/ru/compositors/niri' },
          { text: 'Hyprland', link: '/ru/compositors/hyprland' },
          { text: 'Sway', link: '/ru/compositors/sway' },
        ],
      },
      {
        text: 'Справочник',
        collapsed: false,
        items: [
          { text: 'Настройка', link: '/ru/configuration' },
          { text: 'Команды', link: '/ru/README' },
          { text: 'Устранение неполадок', link: '/ru/troubleshooting' },
        ],
      },
      {
        text: 'Разработка',
        collapsed: true,
        items: [
          { text: 'Сборка и проверки', link: '/ru/development' },
          { text: 'Архитектура', link: '/ru/ARCHITECTURE' },
          { text: 'Безопасность', link: '/ru/SECURITY' },
          { text: 'Бэкенды сеанса', link: '/ru/reference/session-backends' },
          { text: 'Жизненный цикл сеанса', link: '/ru/reference/session-lifecycle' },
        ],
      },
    ],
    outline: { level: [2, 3], label: 'На этой странице' },
    editLink: {
      pattern: `${repository}/edit/main/docs/:path`,
      text: 'Редактировать на GitHub',
    },
    lastUpdated: {
      text: 'Обновлено',
      formatOptions: { dateStyle: 'medium', timeStyle: 'short', forceLocale: true },
    },
    docFooter: { prev: 'Предыдущая страница', next: 'Следующая страница' },
    sidebarMenuLabel: 'Меню',
    returnToTopLabel: 'Наверх',
    langMenuLabel: 'Выбрать язык',
    navMenuLabel: 'Основная навигация',
    mobileMenuLabel: 'Меню',
    extraMenuLabel: 'Дополнительные параметры',
    skipToContentLabel: 'Перейти к содержимому',
    darkModeSwitchLabel: 'Оформление',
    lightModeSwitchTitle: 'Включить светлую тему',
    darkModeSwitchTitle: 'Включить тёмную тему',
    footer: { message: 'Распространяется по лицензии GPL-3.0.' },
    notFound: {
      title: 'СТРАНИЦА НЕ НАЙДЕНА',
      quote: 'Проверьте адрес или вернитесь к оглавлению вики.',
      link: '/ru/',
      linkLabel: 'На главную',
      linkText: 'На главную',
    },
  },
}

export const ruSearch: DefaultTheme.LocalSearchOptions = {
  translations: {
    button: { buttonText: 'Поиск', buttonAriaLabel: 'Поиск по документации' },
    modal: {
      displayDetails: 'Показать подробности',
      resetButtonTitle: 'Очистить поиск',
      backButtonTitle: 'Назад',
      noResultsText: 'Ничего не найдено по запросу',
      footer: {
        selectText: 'выбрать',
        selectKeyAriaLabel: 'Enter',
        navigateText: 'перейти',
        navigateUpKeyAriaLabel: 'Стрелка вверх',
        navigateDownKeyAriaLabel: 'Стрелка вниз',
        closeText: 'закрыть',
        closeKeyAriaLabel: 'Escape',
      },
    },
  },
}
