import { themes as prismThemes } from 'prism-react-renderer';
import type { Config } from '@docusaurus/types';
import type * as Preset from '@docusaurus/preset-classic';

const config: Config = {
  title: 'Hoard',
  tagline: 'eBPF zero-copy file replication to S3',
  favicon: 'img/favicon.ico',

  url: 'https://hoard-project.github.io',
  baseUrl: '/hoard/',
  organizationName: 'hoard-project',
  projectName: 'hoard',

  onBrokenLinks: 'throw',
  onBrokenMarkdownLinks: 'warn',

  i18n: {
    defaultLocale: 'en',
    locales: ['en'],
  },

  markdown: {
    mermaid: true,
  },
  themes: ['@docusaurus/theme-mermaid'],
  plugins: ['./src/plugins/webpack-fix.js'],

  presets: [
    [
      'classic',
      {
        docs: {
          sidebarPath: './sidebars.ts',
          editUrl: 'https://github.com/hoard-project/hoard/tree/main/website/',
        },
        blog: false,
        theme: {
          customCss: './src/css/custom.css',
        },
      } satisfies Preset.Options,
    ],
  ],

  themeConfig: {
    colorMode: {
      defaultMode: 'dark',
      respectPrefersColorScheme: true,
    },
    navbar: {
      title: 'Hoard',
      logo: {
        alt: 'Hoard',
        src: 'img/logo.svg',
      },
      items: [
        {
          type: 'docSidebar',
          sidebarId: 'docsSidebar',
          position: 'left',
          label: 'Docs',
        },
        {
          href: 'https://github.com/hoard-project/hoard',
          label: 'GitHub',
          position: 'right',
        },
      ],
    },
    footer: {
      style: 'dark',
      links: [
        {
          title: 'Docs',
          items: [
            { label: 'Quickstart', to: '/docs/quickstart' },
            { label: 'Configuration', to: '/docs/configuration' },
            { label: 'Architecture', to: '/docs/architecture' },
          ],
        },
        {
          title: 'Community',
          items: [
            { label: 'GitHub', href: 'https://github.com/hoard-project/hoard' },
            { label: 'Releases', href: 'https://github.com/hoard-project/hoard/releases' },
          ],
        },
      ],
      copyright: `GPL-3.0 · Hoard ${new Date().getFullYear()}`,
    },
    prism: {
      theme: prismThemes.github,
      darkTheme: prismThemes.dracula,
      additionalLanguages: ['bash', 'toml', 'hcl', 'json'],
    },
    mermaid: {
      theme: { light: 'neutral', dark: 'dark' },
    },
  } satisfies Preset.ThemeConfig,
};

export default config;
