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
| `arrow-array` | 60 | none (no defaults) | Record and replay: typed column builders for the journal and typed views of decoded batches | `parquet` (row groups need a footer, so a killed recorder loses everything; much larger); a hand-written columnar format (no pyarrow/polars/DuckDB readers) |
| `arrow-schema` | 60 | none (no defaults) | Schema and field metadata (`unjam.unit`, `unjam.kind`, `unjam.source`) that analysis tools read | Part of the same Arrow choice |
| `arrow-ipc` | 60 | `lz4` (no defaults) | The IPC stream format: append-only, readable up to the last complete batch after SIGKILL; LZ4_FRAME compression keeps journals small | IPC file format (footer written last, unreadable after a crash); `zstd` feature (C code, larger binary) |
| `lz4_flex` | 0.14 | `safe-encode`, `safe-decode` (no defaults) | NOT imported: arrow-ipc's `lz4` pulls it in; these features force its safe codec through feature unification | `lz4` crate (C bindings) |
| `signal-hook` | 0.3 | none (no defaults) | SIGTERM/SIGINT/SIGHUP set a flag: the recorder finishes its journal, the TUI tears down; already in the tree through crossterm | Raw `sigaction` (unsafe in our code); `ctrlc` (SIGINT/SIGTERM only, extra crate) |

## Pins and ignores

- `signal-hook` stays on 0.3: crossterm 0.29 requires `^0.3.17`, so a 0.4 bump would add a
  second signal-hook to the tree. Dependabot ignores `>= 0.4` until crossterm moves.
- Upgrades are grouped: `arrow-*` together; `ratatui*`, `crossterm` and `signal-hook` together.
