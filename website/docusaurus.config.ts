import {themes as prismThemes} from 'prism-react-renderer';
import type {Config} from '@docusaurus/types';
import type * as Preset from '@docusaurus/preset-classic';

// This runs in Node.js - Don't use client-side code here (browser APIs, JSX...)

const config: Config = {
  title: 'Heldar',
  tagline: 'Open visual-event intelligence for physical spaces',
  favicon: 'img/favicon.svg',

  // Future flags, see https://docusaurus.io/docs/api/docusaurus-config#future
  future: {
    v4: true, // Improve compatibility with the upcoming Docusaurus v4
  },

  // Production URL: served from Cloudflare Workers (Static Assets) at the root of
  // the Worker's domain (the *.workers.dev subdomain or a custom domain). No path
  // prefix, so baseUrl is "/". Set this to your final custom domain.
  url: 'https://docs.heldar.ai',
  baseUrl: '/',

  // Source repo metadata (used for editUrl and links).
  organizationName: 'Straits-AI',
  projectName: 'heldar',
  trailingSlash: false,

  onBrokenLinks: 'warn',

  // Brand typography, mirrored from the Heldar dashboard:
  // Archivo (display/headings), Hanken Grotesk (body/UI), JetBrains Mono (data/code).
  headTags: [
    {
      tagName: 'link',
      attributes: {rel: 'preconnect', href: 'https://fonts.googleapis.com'},
    },
    {
      tagName: 'link',
      attributes: {
        rel: 'preconnect',
        href: 'https://fonts.gstatic.com',
        crossorigin: 'anonymous',
      },
    },
    {
      tagName: 'link',
      attributes: {
        rel: 'stylesheet',
        href: 'https://fonts.googleapis.com/css2?family=Archivo:wght@600;700;800&family=Hanken+Grotesk:wght@400;500;600;700&family=JetBrains+Mono:wght@400;500;600&display=swap',
      },
    },
  ],

  markdown: {
    hooks: {
      onBrokenMarkdownLinks: 'warn',
    },
  },

  // Even if you don't use internationalization, you can use this field to set
  // useful metadata like html lang.
  i18n: {
    defaultLocale: 'en',
    locales: ['en'],
  },

  presets: [
    [
      'classic',
      {
        docs: {
          sidebarPath: './sidebars.ts',
          editUrl: 'https://github.com/Straits-AI/heldar/tree/main/website/',
        },
        blog: false,
        theme: {
          customCss: './src/css/custom.css',
        },
      } satisfies Preset.Options,
    ],
  ],

  themeConfig: {
    // Replace with your project's social card
    image: 'img/docusaurus-social-card.jpg',
    // Heldar is a dark SOC console; the marketing + docs site is dark by default.
    colorMode: {
      defaultMode: 'dark',
      respectPrefersColorScheme: false,
    },
    navbar: {
      title: 'Heldar',
      logo: {
        alt: 'Heldar Logo',
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
          href: 'https://github.com/Straits-AI/heldar',
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
            {
              label: 'Introduction',
              to: '/docs',
            },
            {
              label: 'Quickstart',
              to: '/docs/getting-started/quickstart',
            },
          ],
        },
        {
          title: 'Project',
          items: [
            {
              label: 'GitHub',
              href: 'https://github.com/Straits-AI/heldar',
            },
            {
              label: 'Issues',
              href: 'https://github.com/Straits-AI/heldar/issues',
            },
          ],
        },
      ],
      copyright: `Copyright © ${new Date().getFullYear()} Straits AI. Built with Docusaurus.`,
    },
    prism: {
      theme: prismThemes.github,
      darkTheme: prismThemes.dracula,
    },
  } satisfies Preset.ThemeConfig,
};

export default config;
