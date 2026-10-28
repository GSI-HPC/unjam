<!--
SPDX-License-Identifier: LGPL-3.0-or-later
SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany
-->

# Performance record

Measured numbers only; budgets are in `.claude/skills/hot-path/SKILL.md`. Each row names the
commit, the machine (sind worker, Slurm line, kernel) and the command that produced it.

## Per-tick cost (sind worker)

| Commit | Setup | Case | task-clock per tick | Syscalls per tick | fd-cache hit ratio | Notes |
|---|---|---|---|---|---|---|
| — | — | Typical node: 70 cgroups at `p`, 100 processes, 1 Hz | — | — | — | Budget ≤ 0.5 % of one core |

## Per-file read cost

| Commit | Kernel | File | Cached pread (µs) | Uncached open+read+close (µs) | Notes |
|---|---|---|---|---|---|
| — | — | `cpu.stat` | — | — | rstat flush |
| — | — | `io.stat` | — | — | rstat flush |
| — | — | `memory.stat` | — | — | |
| 485e292 | 6.18.44 (dev container, not a sind worker) | `/proc/meminfo` (1.5 KB) | 1.5–2.0 | 2.6–3.5 | Local TUI only, once per second; `fs::read` with its heap buffer was 4.6–5.9 µs. 20,000 reads per case, 3 runs |

## Benchmarks (`mise run bench`)

| Commit | ns/cgroup | ns/process | bytes/tick | apply ns/value | Notes |
|---|---|---|---|---|---|
| — | — | — | — | — | |

## Allocations (unit test, fixtures)

| Commit | Fixture | Sample + encode | Discovery walk | Notes |
|---|---|---|---|---|
| — | 25.11, 8 cgroups at `p`, 3 processes | 0 | 143 per walk (2 per directory for `read_dir`, 1 name per entry, 1 path per cgroup) | Walk builds paths for directories only (335 when every entry built one). A real cgroup directory has about 40–60 entries, so about 50 per cgroup per walk (estimate) |

## Integration measurements

| Commit | Test | Bytes/s into the TUI | Agent CPU | Agent RSS | Errors | Notes |
|---|---|---|---|---|---|---|
| 8365580 | IT 5 (3 × 500 processes, 100 ms), sind v0.15.0, Slurm 25.11.8 and 26.05.5, kernel 6.18 (VM: no cgroup v2 cpu/memory/pids controllers, so no CPU limits on the 2-CPU workers) | 590 KB/s, 15k lines/s | 10 % of one core (busiest node, from `T`) | 1.8 MiB | 0 | 1,510 processes; 10 Hz × ~1,000 cached preads per node; TUI 3–4 % CPU, 2.4 MiB RSS. Not the default rules: the ≤ 1 % budget applies to those |

## Size

| Commit | Binary bytes | Crates | Notes |
|---|---|---|---|
| — | 556,672 | 10 | clap only, scaffold |
