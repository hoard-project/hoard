import type { SidebarsConfig } from '@docusaurus/plugin-content-docs';

const sidebars: SidebarsConfig = {
  docsSidebar: [
    'index',
    'quickstart',
    {
      type: 'category',
      label: 'Guides',
      items: ['configuration', 'operations', 'architecture', 'nomad'],
    },
    {
      type: 'category',
      label: 'Reference',
      items: ['reference/cli', 'reference/env-vars', 'reference/control-socket'],
    },
  ],
};

export default sidebars;
