---
name: lean-code
description: Use when adding, changing, or reviewing any Rust item (fn, struct, enum, trait, type, const, static, mod, impl) under src/ in unjam. Enforces "every item justifies its existence", the per-module LOC budget, no speculative generality, and the file allowlist for new metric sources.
---

# lean-code: every item must earn its place

## Before you write anything
1. Find what already exists: `grep -rn "fn <verb>\|struct <Noun>" src/`. Extend an existing item if it can do the job.
2. Write the `///` doc first. Its first sentence answers: *why must this exist, and why can't an existing item do it?*
   No answer → do not add the item.
3. Check the module's line in `docs/budget.txt`. If the change exceeds it, delete code elsewhere first,
   or raise the number in the same PR with a one-line reason next to it.

## Hard rules (a reviewer rejects violations)
- Every fn/struct/enum/trait/type/const/static/mod has a `///` doc (checked by `cargo test --test budget`).
- The `#[cfg(test)]` module is the last item of a file; the budget gate stops reading there.
- Compiled-in data tables (the theme table) sit between `// budget: data` and `// budget: end`.
- No trait with fewer than two implementations today. Only exception: `Source`.
- No generic parameter with a single instantiation; no `impl Trait` argument used by one caller.
- No builder types, no getters/setters on crate-internal structs, no `Default` impls nobody calls.
- No new module without a `docs/budget.txt` line. No `macro_rules!` unless it removes ≥ 30 LOC.
- `Box<dyn …>` only for `Source` and for `Box<dyn std::error::Error>` as the error type of `main`.
  No `Rc`/`RefCell`. No `async`: std threads and `std::sync::mpsc` only.
- Functions ≤ 60 lines (clippy `too_many_lines`). Every `#[allow(..)]` has `reason = "…"`.
- Errors: `io::Error` or `String` (plus `Box<dyn Error>` in `main` only); no error enums unless two callers match on variants.
- Prefer deleting code over adding it. Dead code is removed, not `#[allow(dead_code)]`-ed.

## Shape of the program (do not grow new axes)
- CLI: one viewer command with a source flag: `unjam` (local), `unjam --slurm[=JOBID]`,
  `unjam --replay FILE.arrows…`; `record` and hidden `agent` are the only subcommands. `--slurm`
  and `--replay` are one clap group; Slurm-only flags use `requires = "slurm"`.
- Exactly three layouts, as level orders in `View` walked by one `state::rows`: step, node, flat.
  A new view is a new level order, not new walking code. No "by command" layout, no RANK column.
- The sort column orders siblings only; a row never leaves its parent.
- Missing controllers (io, pids) are NA, never an error: the column hides, PIDS falls back to `~`.

## Adding a metric source (R10): file allowlist
A new source MAY touch only:
- `src/<name>.rs` (struct, `METRICS: &[Metric]`, `probe(&Env) -> Result<Box<dyn Source>, String>`)
- one line in `SOURCES` in `src/source.rs`
- one line in `docs/budget.txt`, `tests/fixtures/<name>/**`, and `docs/deps.md` if it adds a dependency
It MUST NOT touch `wire.rs`, `record.rs`, `state.rs`, `tui.rs`. Metric names: `<cg|p>_<source>_<quantity>_<unit>`,
`[a-z0-9_]+`, append-only, never renamed. There is no per-metric cost class: measure the source's per-entity
read cost (hot-path skill) and record it in `docs/perf.md`; users control cost through rule intervals.
If a measured source is too expensive for the default tick, raise it with the owner (a cost field can be
appended to `D` lines without a proto bump). Probe failures return `Err(reason)`, never panic.
The only sanctioned core change is a new `Ent` variant, and it needs owner approval.

## Commands (all must pass before you report done)
    mise run check            # fmt, clippy -D warnings, test (incl. budget), reuse, deny, size, unsafe-deps
    cargo test --locked --test budget -- --nocapture   # line count per module
    git diff --stat

## PR description must contain
- LOC delta per module and total (from `cargo test --test budget -- --nocapture`).
- For each new item: its name and the one-line "why".
- If a budget number changed: old → new and the reason.
