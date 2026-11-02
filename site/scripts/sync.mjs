// SPDX-License-Identifier: LGPL-3.0-or-later
// SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany

// Copies the repository's living documents into the site before each build, so the site never
// holds a second, drifting copy: docs/design.md becomes the design reference, CHANGELOG.md the
// changelog. The outputs are generated files (see .gitignore).
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const repo = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
const docs = join(repo, 'site', 'src', 'content', 'docs');
const blob = 'https://github.com/GSI-HPC/unjam/blob/main/';

// Drops the SPDX comment and the first H1; the frontmatter title replaces the H1.
function body(file) {
  return readFileSync(join(repo, file), 'utf8')
    .replace(/^<!--[\s\S]*?-->\s*/, '')
    .replace(/^# .*\n+/, '');
}

function write(rel, front, text) {
  const out = join(docs, rel);
  mkdirSync(dirname(out), { recursive: true });
  const yaml = Object.entries(front).map(([k, v]) => `${k}: ${JSON.stringify(v)}`).join('\n');
  writeFileSync(out, `---\n${yaml}\n---\n\n${text}`);
}

write('reference/design-document.md', {
  title: 'Design document',
  description: 'The full living architecture design of unjam (docs/design.md), with every decision and its review history.',
  editUrl: `${blob.replace('/blob/', '/edit/')}docs/design.md`,
  tableOfContents: { minHeadingLevel: 2, maxHeadingLevel: 2 },
}, `:::note[Generated page]\nThis page is [docs/design.md](${blob}docs/design.md), copied at build time. The [Design](/unjam/design/overview/) section explains the same ideas with diagrams.\n:::\n\n${body('docs/design.md')}`);

write('reference/changelog.md', {
  title: 'Changelog',
  description: 'All notable changes to unjam, by release.',
  editUrl: `${blob.replace('/blob/', '/edit/')}CHANGELOG.md`,
  tableOfContents: { minHeadingLevel: 2, maxHeadingLevel: 2 },
}, body('CHANGELOG.md').replace(/^All notable changes.*\n.*\n/m, ''));
