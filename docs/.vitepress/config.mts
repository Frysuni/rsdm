import { defineConfig } from 'vitepress'

const repository = 'https://github.com/Frysuni/rsdm'
const site = 'https://frysuni.github.io/rsdm/'

export default defineConfig({
  title: 'RSDM Wiki',
  description: 'Installation, configuration and usage guides for RSDM, the Rust display manager for Wayland.',
  lang: 'en-US',
  base: '/rsdm/',
  appearance: 'dark',
  lastUpdated: true,
  sitemap: { hostname: site },
  transformHead({ pageData }) {
    const path = pageData.relativePath.replace(/(^|\/)index\.md$/, '$1').replace(/\.md$/, '.html')
    return [['link', { rel: 'canonical', href: new URL(path, site).href }]]
  },
  themeConfig: {
    siteTitle: 'RSDM',
    nav: [
      {
        text: 'Install',
        items: [
          { text: 'NixOS', link: '/installation/nixos' },
          { text: 'Arch Linux', link: '/installation/arch' },
          { text: 'Fedora', link: '/installation/fedora' },
          { text: 'Ubuntu', link: '/installation/ubuntu' },
        ],
      },
      { text: 'Commands', link: '/README' },
      { text: 'Configuration', link: '/configuration' },
      { text: 'Releases', link: `${repository}/releases` },
    ],
    sidebar: [
      {
        text: 'Getting started',
        items: [
          { text: 'Overview', link: '/' },
          { text: 'Commands and CLI', link: '/README' },
        ],
      },
      {
        text: 'Installation',
        items: [
          { text: 'NixOS', link: '/installation/nixos' },
          { text: 'Arch Linux', link: '/installation/arch' },
          { text: 'Fedora', link: '/installation/fedora' },
          { text: 'Ubuntu', link: '/installation/ubuntu' },
        ],
      },
      {
        text: 'Using RSDM',
        items: [
          { text: 'DM and Greeter', link: '/display-manager' },
          { text: 'Lock screen', link: '/lock' },
          { text: 'Idle locking', link: '/idle' },
          { text: 'Session manager', link: '/session-manager' },
          { text: 'Keyrings', link: '/keyrings' },
        ],
      },
      {
        text: 'Compositors',
        items: [
          { text: 'niri', link: '/compositors/niri' },
          { text: 'Hyprland', link: '/compositors/hyprland' },
        ],
      },
      {
        text: 'Reference',
        items: [
          { text: 'Configuration', link: '/configuration' },
          { text: 'Troubleshooting', link: '/troubleshooting' },
          { text: 'Architecture', link: '/ARCHITECTURE' },
          { text: 'Security', link: '/SECURITY' },
          { text: 'Audit remediation', link: '/audit-remediation' },
        ],
      },
    ],
    outline: [2, 3],
    search: { provider: 'local' },
    socialLinks: [{ icon: 'github', link: repository }],
    editLink: { pattern: `${repository}/edit/main/docs/:path` },
    footer: { message: 'Released under the GPL-3.0 license.' },
  },
})
