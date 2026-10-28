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

## Benchmarks (`mise run bench`)

| Commit | ns/cgroup | ns/process | bytes/tick | apply ns/value | Notes |
|---|---|---|---|---|---|
| — | — | — | — | — | |

## Integration measurements

| Commit | Test | Bytes/s into the TUI | Agent CPU | Agent RSS | Errors | Notes |
|---|---|---|---|---|---|---|
| — | IT 5 (3 × 500 processes, 100 ms) | — | — | — | — | |

## Size

| Commit | Binary bytes | Crates | Notes |
|---|---|---|---|
| — | 556,672 | 10 | clap only, scaffold |
