<!--
SPDX-License-Identifier: LGPL-3.0-or-later
SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany
-->

# unjam documentation site

The site at <https://gsi-hpc.github.io/unjam/>: [Astro](https://astro.build/) with
[Starlight](https://starlight.astro.build/) and the Nord theme in `src/styles/nord.css`.
`pages.yml` builds it and deploys it at `/`, next to the dnf repository at `/rpm/`.

```sh
mise run docs:dev   # http://localhost:4321/unjam/ with live reload
mise run docs       # static build into site/dist
```

Node comes from `mise.toml`. `npm run build` and `npm run dev` first run `scripts/sync.mjs`,
which copies `docs/design.md` and `CHANGELOG.md` into `src/content/docs/reference/` (those two
pages are generated and ignored by git).

## Layout

| Path | Holds |
|---|---|
| `astro.config.mjs` | Site settings and the sidebar |
| `src/content/docs/` | The pages (MDX) |
| `src/components/diagrams/` | One inline-SVG diagram per component; motion in CSS keyframes and SMIL |
| `src/components/Terminal.astro` | The terminal player (no `frame` prop) and still frames (`frame={n}`) |
| `src/lib/ansi.ts` | ANSI to HTML, shared by the build (stills) and the player |
| `src/data/term/*.json` | Recorded terminal sessions |
| `public/media/` | The tour video: `tour.mp4` (required), `tour.webm`, `tour-poster.jpg`, `tour.vtt` |

Diagrams use the `--uj-*` colour tokens of `nord.css`, so they follow the light and dark
mode, and put moving parts in a `.motion` group, which `prefers-reduced-motion` hides.

## Terminal recordings

A recording is `{"size": "COLSxROWS", "frames": [{"a": ANSI screen, "c": caption, "k": keys,
"d": seconds}]}`, made from real unjam screens with `scripts/capture.py` (needs tmux):

```sh
cat > steps.txt <<'EOF'
WAIT 3 The step layout: job, step, node, task, process.
KEY Down Down Down | Move to a node.
KEY @2.5 Enter | Enter shows every value of the row.
KEY Escape q | q quits.
EOF
scripts/capture.py src/data/term/slurm.json 100x30 -- unjam --slurm=4242 < steps.txt
```

Use 100x30, a fresh `unjam` release build, and the default theme. The checked-in recordings
were made on a four-node Slurm stand-in: shims for `squeue`, `scontrol`, `srun` and
`scancel`, and one cgroup namespace per "node" over a real cgroup2 tree with real processes.
That host had no cgroup2 memory, io or pids controllers, so MEM and IO show `-` on cgroup rows
and PIDS shows the `~` fallback, as on a Slurm site without `EnableExtraControllers`.
