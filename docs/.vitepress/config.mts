import { defineConfig } from 'vitepress'
import { ru, ruSearch } from './locales/ru.mts'

const repository = 'https://github.com/Frysuni/rsdm'
const site = 'https://frysuni.github.io/rsdm/'

export default defineConfig({
  title: 'RSDM Wiki',
  description: 'Installation, configuration and usage guides for RSDM, the Rust display manager for Wayland.',
  lang: 'en-US',
  locales: { root: { label: 'English', lang: 'en-US' }, ru },
  base: '/rsdm/',
  appearance: 'dark',
  lastUpdated: true,
  sitemap: { hostname: site },
  transformHead({ pageData }) {
    const path = pageData.relativePath.replace(/(^|\/)index\.md$/, '$1').replace(/\.md$/, '.html')
    const englishPath = path.replace(/^ru\//, '')
    return [
      ['link', { rel: 'canonical', href: new URL(path, site).href }],
      ['link', { rel: 'alternate', hreflang: 'en', href: new URL(englishPath, site).href }],
      ['link', { rel: 'alternate', hreflang: 'ru', href: new URL(`ru/${englishPath}`, site).href }],
      ['link', { rel: 'alternate', hreflang: 'x-default', href: new URL(englishPath, site).href }],
    ]
  },
  themeConfig: {
    siteTitle: 'RSDM',
    nav: [
      { text: 'Start here', link: '/getting-started' },
      { text: 'Configuration', link: '/configuration' },
      { text: 'Commands', link: '/README' },
      {
        text: 'Project',
        items: [
          { text: 'Development', link: '/development' },
          { text: 'Releases', link: `${repository}/releases` },
        ],
      },
    ],
    sidebar: [
      {
        text: 'Start here',
        items: [
          { text: 'Overview', link: '/' },
          { text: 'Getting started', link: '/getting-started' },
          { text: 'Compatibility', link: '/compatibility' },
        ],
      },
      {
        text: 'Installation',
        collapsed: false,
        items: [
          { text: 'NixOS', link: '/installation/nixos' },
          { text: 'Arch Linux', link: '/installation/arch' },
          { text: 'Fedora', link: '/installation/fedora' },
          { text: 'Ubuntu', link: '/installation/ubuntu' },
          { text: 'Manual installation', link: '/installation/manual' },
          { text: 'Activation and recovery', link: '/installation/activate' },
        ],
      },
      {
        text: 'Using RSDM',
        collapsed: false,
        items: [
          { text: 'DM and Greeter', link: '/display-manager' },
          { text: 'Lock screen', link: '/lock' },
          { text: 'Idle locking', link: '/idle' },
          { text: 'Session Manager', link: '/session-manager' },
          { text: 'Keyrings', link: '/keyrings' },
        ],
      },
      {
        text: 'Compositors',
        collapsed: true,
        items: [
          { text: 'niri', link: '/compositors/niri' },
          { text: 'Hyprland', link: '/compositors/hyprland' },
          { text: 'Sway', link: '/compositors/sway' },
        ],
      },
      {
        text: 'Reference',
        collapsed: false,
        items: [
          { text: 'Configuration', link: '/configuration' },
          { text: 'Commands', link: '/README' },
          { text: 'Troubleshooting', link: '/troubleshooting' },
        ],
      },
      {
        text: 'Development',
        collapsed: true,
        items: [
          { text: 'Build and validate', link: '/development' },
          { text: 'Architecture', link: '/ARCHITECTURE' },
          { text: 'Security', link: '/SECURITY' },
          { text: 'Session backends', link: '/reference/session-backends' },
          { text: 'Session lifecycle', link: '/reference/session-lifecycle' },
        ],
      },
    ],
    outline: [2, 3],
    search: { provider: 'local', options: { locales: { ru: ruSearch } } },
    socialLinks: [{ icon: 'github', link: repository }],
    editLink: { pattern: `${repository}/edit/main/docs/:path` },
    footer: { message: 'Released under the GPL-3.0 license.' },
  },
})
