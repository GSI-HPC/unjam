<!--
SPDX-License-Identifier: LGPL-3.0-or-later
SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany
-->

# Dependency ledger

Every `[dependencies]` entry of `Cargo.toml` has a row here (checked by `tests/budget.rs`).
The admission rule is in `.claude/skills/lean-deps/SKILL.md`; size and crate limits are in
`docs/budget.txt`.

| Crate | Version | Features | Why | Alternatives rejected |
|---|---|---|---|---|
| `clap` | 4.6 | `std`, `derive`, `help`, `usage`, `error-context` (no defaults) | The CLI surface; repo convention "clap derive only". Without `color`, `suggestions` and `wrap_help` there is no anstream, strsim or terminal_size | Hand-written parser (more code, worse help); `argh`/`lexopt` (break the repo convention) |
| `ratatui` | 0.30 | `crossterm_0_29` only (no defaults) | Terminal UI: layout, table and paragraph widgets, double-buffered diffing, `TestBackend` for draw goldens; crossterm through the `ratatui::crossterm` re-export, so no direct crossterm entry | Raw crossterm drawing (re-implements diffing and layout); `cursive` (callback model, heavier); `termion` backend (Unix-only, less maintained) |

## Planned

These arrive with the code that uses them (design §10.3). Each gets a row above when added.

| Crate | Version | Purpose |
|---|---|---|
| `arrow-array`, `arrow-schema`, `arrow-ipc` | 60, `arrow-ipc` feature `lz4` | Record and replay (Arrow IPC stream journal) |
| `lz4_flex` | 0.14, `safe-encode`, `safe-decode` | NOT imported: forces the safe LZ4 codec through feature unification |
| `signal-hook` | 0.3 | SIGTERM/SIGINT/SIGHUP to a flag |

## Pins and ignores

- `signal-hook` stays on 0.3: crossterm 0.29 requires `^0.3.17`, so a 0.4 bump would add a
  second signal-hook to the tree. Dependabot ignores `>= 0.4` until crossterm moves.
- Upgrades are grouped: `arrow-*` together; `ratatui*`, `crossterm` and `signal-hook` together.
