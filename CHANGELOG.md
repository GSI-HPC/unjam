<!--
SPDX-License-Identifier: LGPL-3.0-or-later
SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany
-->

# Changelog

All notable changes to unjam are listed here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses [Semantic Versioning](https://semver.org/).

## [0.2.0] - 2026-10-28

### Added

- Terminal UI over the cgroup v2 tree with step, node and flat layouts, themes and a local-mode viewer.
- cgroup and `/proc` metric sources with a file-descriptor cache, and a sampler with per-entity rules.
- Hidden `agent` command with line-based agent and control protocols.
- `unjam --slurm[=JOBID]` attaches to a Slurm job through `srun --overlap`.
- `--unlink-self` makes the agent remove an `srun --bcast` copy of itself.
- `unjam record -- CMD` records a job step to an Arrow IPC journal (LZ4 frames) with unit, kind and source field metadata.
- `unjam --replay FILE...` replays one or more journals with play, step and jump keys, `--at TIME` and `--batch N`.
- `r` and `--record DIR` record the live view to one journal per node.
- Integration tests on sind (Slurm in Docker) for attach, isolation, rules, teardown, load, `--bcast`, record and replay.
- Architecture design document and usage, site and recording documentation.

### Fixed

- Take the unified line of `/proc/self/cgroup` on hybrid cgroup hosts.

### Changed

- Build: MSRV, lints and formatting; budget, binary size and dependency unsafe gates.
- CI: checks run through mise; release binaries are built with provenance.
- New dependencies: `ratatui`, `arrow-ipc`, `arrow-array`, `arrow-schema` and `signal-hook`.

[0.2.0]: https://github.com/GSI-HPC/unjam/compare/v0.1.0...v0.2.0
