// SPDX-License-Identifier: LGPL-3.0-or-later
// SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany

import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

// GitHub Pages serves the project at /unjam/; pages.yml puts the dnf repository at /unjam/rpm/.
export default defineConfig({
  site: 'https://gsi-hpc.github.io',
  base: '/unjam',
  trailingSlash: 'always',
  integrations: [
    starlight({
      title: 'unjam',
      description: 'A top-like terminal UI over the cgroup v2 tree, for Linux hosts and Slurm jobs.',
      logo: { src: './src/assets/logo.svg' },
      favicon: '/favicon.svg',
      customCss: [
        './src/styles/fonts.css',
        './src/styles/nord.css',
        './src/styles/layout.css',
      ],
      // Code blocks: Nord syntax colours on the site's own surfaces, and no empty terminal title
      // bar on a block without a title.
      expressiveCode: {
        themes: ['nord', 'github-light'],
        useStarlightUiThemeColors: true,
        defaultProps: { frame: 'code' },
        styleOverrides: {
          borderRadius: '4px',
          codeFontFamily: 'var(--sl-font-mono)',
          codeFontSize: '0.875rem',
          codeLineHeight: '1.6',
          codePaddingBlock: '0.8125rem',
          codePaddingInline: '1rem',
          uiFontFamily: 'var(--sl-font)',
          frames: { shadowColor: 'transparent' },
        },
      },
      social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/GSI-HPC/unjam' }],
      editLink: { baseUrl: 'https://github.com/GSI-HPC/unjam/edit/main/site/' },
      lastUpdated: false,
      credits: false,
      sidebar: [
        {
          label: 'Start',
          items: [
            { label: 'What is unjam?', slug: 'start/introduction' },
            { label: 'Install', slug: 'start/install' },
            { label: 'Quick start', slug: 'start/quick-start' },
          ],
        },
        {
          label: 'Guide',
          items: [
            { label: 'Watch a host', slug: 'guide/host' },
            { label: 'Watch a Slurm job', slug: 'guide/slurm' },
            { label: 'Navigate the view', slug: 'guide/navigate' },
            { label: 'Control the sampling', slug: 'guide/sampling' },
            { label: 'Record and replay', slug: 'guide/record-replay' },
            { label: 'Analyze recordings', slug: 'guide/analysis' },
            { label: 'Scripts and batch mode', slug: 'guide/batch' },
            { label: 'Themes', slug: 'guide/themes' },
            { label: 'For site administrators', slug: 'guide/site' },
            { label: 'Troubleshooting', slug: 'guide/troubleshooting' },
          ],
        },
        {
          label: 'Design',
          items: [
            { label: 'Architecture', slug: 'design/overview' },
            { label: 'Data model', slug: 'design/data-model' },
            { label: 'Collection', slug: 'design/collection' },
            { label: 'Finding the Slurm job', slug: 'design/job-root' },
            { label: 'Rules and timing', slug: 'design/sampling' },
            { label: 'Wire protocol', slug: 'design/wire' },
            { label: 'The viewer', slug: 'design/viewer' },
            { label: 'Recording and replay', slug: 'design/recording' },
            { label: 'Small by design', slug: 'design/lean' },
            { label: 'Key decisions', slug: 'design/decisions' },
          ],
        },
        {
          label: 'Implementation',
          items: [
            { label: 'Module map', slug: 'internals/modules' },
            { label: 'Life of a sample', slug: 'internals/life-of-a-sample' },
            { label: 'Add a metric source', slug: 'internals/extending' },
            { label: 'Testing', slug: 'internals/testing' },
            { label: 'CI and releases', slug: 'internals/ci' },
            { label: 'Contributing', slug: 'internals/contributing' },
          ],
        },
        {
          label: 'Reference',
          items: [
            { label: 'Command line', slug: 'reference/cli' },
            { label: 'Keys', slug: 'reference/keys' },
            { label: 'Metrics and columns', slug: 'reference/metrics' },
            { label: 'Wire lines', slug: 'reference/wire' },
            { label: 'Recording format', slug: 'reference/recording-format' },
            { label: 'Design document', slug: 'reference/design-document' },
            { label: 'Changelog', slug: 'reference/changelog' },
          ],
        },
      ],
    }),
  ],
});
