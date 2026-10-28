<!--
SPDX-License-Identifier: LGPL-3.0-or-later
SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany
-->

# unjam: notes for Claude Code

unjam is a top-like TUI over the cgroup v2 tree for Linux hosts and Slurm jobs. Design: docs/design.md.
Priorities, in order: correct, small, fast. Every item, dependency and unsafe block must justify itself.

## Shape
- CLI: `unjam` (local), `unjam --slurm[=JOBID]` (attach via `srun --overlap`), `unjam --replay FILE.arrows…`;
  subcommands `record` and hidden `agent` only.
- Three layouts: step, node, flat (`v` cycles). Sort orders siblings only.
- No site requirement: without `EnableExtraControllers=io,pids` the IO/PIDS columns hide or fall back.

## Always
- Load the skill that matches the change: `lean-code` (any src/ item), `lean-deps` (Cargo.toml),
  `hot-path` (agent/source/cgroup/procfs/wire/state/input/draw), `unsafe-policy` (any unsafe or FFI).
- Run `mise run check` before saying a change is done. Integration: `mise run it` (needs Docker; sind).
- Toolchain comes from rust-toolchain.toml (1.93.1); MSRV is 1.88 (`mise run msrv`).
- Conventional Commits. SPDX header on every new file (REUSE); `.claude/**` and `tests/fixtures/**`
  are covered by REUSE.toml.

## Never
- Add a dependency, trait, generic, module or `#[allow]` without the justification the skills require.
- Print to stdout outside wire/batch code (stdout is the agent protocol or the wrapped app's).
- Match Slurm job directories by `job_<id>`: use `cgroup::job_root` (26.05 uses SLUID names).
- Add a per-tick file read without a §5.3 row, a cache-tier decision and a measured cost in docs/perf.md.
- Change `wire.rs`, `record.rs`, `state.rs` or `tui.rs` to add a metric source.
