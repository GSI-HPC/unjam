<!--
SPDX-License-Identifier: LGPL-3.0-or-later
SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany
-->

# Tour video

The sources of the tour video on the landing page of the docs site. The video is about 75 s,
1920×1080 at 30 fps, without sound. `build.sh` writes it to `site/public/media/`:
`tour.mp4` (H.264), `tour.webm` (VP9), `tour-poster.jpg` and `tour.vtt` (English captions).

| Part | Source | Content |
|---|---|---|
| Intro, Problem, How, Record, Outro | `scenes.py` (Manim) | motion graphics in the Nord Frost colours |
| slurm, host, replay | `capture.py` → `render.mjs` | the real unjam TUI on the bench |

The TUI shots are not mock-ups: `capture.py` runs the release binary in tmux, plays a script of
keys and captions, and samples the pane 15 times a second. `render.mjs` draws each sample in a
terminal window with Chromium.

## Bench

`bench/` emulates a host and a 4-node Slurm job (job 4242) on one Linux machine with cgroup
namespaces, so no cluster is necessary. `bench/setup.sh` starts synthetic workloads
(`bench/work.c`) in a cgroup2 mount (default `/sys/fs/cgroup/unified`; set `BENCH_UNIFIED`
for another). node3 is the CPU straggler, node2 writes 512 KiB/s. `bench/bin` has a fake
`squeue`, `scontrol` and `srun`, and `hostview`, which starts a command as on a cgroup v2 host.
The bench needs root.

## Build

Needs ffmpeg (libx264, libvpx), bc, tmux, Node.js with Playwright and Chromium, a C compiler,
Manim Community 0.19 or later, and the fonts Inter and JetBrains Mono.

```sh
cargo build --release
PATH=$PWD/target/release:$PATH docs/tour/build.sh capture   # capture again, then render
docs/tour/build.sh                                          # render the existing captures
```

`TOUR_WORK` sets the work directory (default `/tmp/unjam-tour`), `TOUR_OUT` the output
directory, `MANIM` the manim command.
