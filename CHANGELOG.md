<!--
SPDX-License-Identifier: LGPL-3.0-or-later
SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany
-->

# Changelog

All notable changes to unjam are listed here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Fixed

- The agent keeps sampling after a backward wall-clock step, and a forward step no longer ends it through the keepalive.
- A process that `exec`s after it was first seen shows its new command name and line.
- An unreadable file, such as another user's `/proc/<pid>/io`, explains its NA with one warning per agent.
- Nodes without the io or pids controllers retry the absent files once per 10 s instead of on every tick.
- IO-R/s and IO-W/s are no longer doubled on nodes with LVM, dm-crypt or md disks.
- `--slurm` refuses to attach when the agent's own step is not below `/sys/fs/cgroup` (a cgroup namespace with a host-view mount), instead of sampling every cgroup on the node.
- `--slurm=<array job id>` attaches to the array's running task and lists the tasks when several run.
- `--interval`, `record --interval` and the `+`/`-` keys round to the nearest 100 ms, like `--rule`.
- `--unlink-self` no longer fails when another agent already removed a shared `--bcast` copy.
- A cgroup or process whose removal line was lost no longer stays with frozen values after a resync.
- Nodes whose subtrees are all off are no longer shown stale between their 10 s rescans.
- A node whose agent speaks another wire protocol is logged instead of staying blank.
- Keys act on the highlighted row even when new data has re-sorted the table.
- The node heat strip bands relative to the busiest node, so imbalance between many-core nodes shows.
- The TUI recording is finished when the viewer exits on an error, such as `--batch` into `| head`; `--batch` also quits on SIGTERM and SIGHUP.
- TUI recordings store each tick before the next tick's structure changes are applied.
- A backward seek in replay restores the rates of entities on slow rules.
- Replay of several journals steps by sampling round instead of one node at a time.
- `unjam record` prints why it runs the app unrecorded before it does.

### Security

- The `--bcast` copy gets a name other users cannot guess, and `--bcast` into another user's job is refused.
- Journals are created owner-only (0600) and TUI recording directories 0700; an agent-reported host name can no longer place a TUI journal outside the recording directory.
- A crafted journal with a cgroup parent cycle no longer overflows the stack during replay.
- `--batch` prints control characters in names as `?`.

### Changed

- The local host memory bar keeps `/proc/meminfo` open, and drawing no longer allocates per bar cell.
- `mise run bench` reports sampler and viewer benchmarks.
- CI: clippy errors fail the checks, the arm64 release binary is run before release, and `cargo publish` skips its verify build while the crates.io token is set.

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

[Unreleased]: https://github.com/GSI-HPC/unjam/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/GSI-HPC/unjam/compare/v0.1.0...v0.2.0
