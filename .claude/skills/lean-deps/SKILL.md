---
name: lean-deps
description: Use before adding, removing, upgrading, or changing features of any entry in Cargo.toml [dependencies] (or when Cargo.lock gains a crate). Enforces the dependency admission rule, minimal features, size/crate budgets, MSRV, licenses, static musl builds, and the unsafe ledger for dependencies.
---

# lean-deps: dependency admission

## Admission rule: answer all in the PR, in this order
1. Can std do it in < 50 LOC we would maintain? If yes → no dependency.
2. Is it already in the tree? `cargo tree -e normal -i <crate>`. Already-present crates cost 0 crates but still need a row.
3. Exact features: `default-features = false` is mandatory; list each enabled feature and why. Check what
   transitive default features come along (e.g. ratatui-crossterm's `underline-color`) and count them.
4. Measured cost: run `mise run size` before and after; report Δ binary bytes and Δ crate count.
   Limits live in `docs/budget.txt` (`binary_bytes`, `crates`); exceeding them requires owner approval.
5. MSRV: its `rust-version` must be ≤ our `rust-version` (1.88). Check: `mise run msrv`.
6. unsafe/FFI: run `mise run unsafe-deps` (fails on any in-tree crate with `unsafe` lacking a `docs/unsafe.md`
   row, and on rows for crates no longer in the tree; `scripts/unsafe-deps.sh --print` emits stubs).
   Fill in what the unsafe does and why it is accepted.
   Any C-building `-sys` crate (a `cc`/`pkg-config` build script) → rejected unless the owner approves:
   it breaks the static musl release binaries (`mise run build:musl`).
   `linux-raw-sys` (pure-Rust bindings via rustix) is exempt.
7. License: must pass `mise run deny` (allow-list and `[graph] targets` in deny.toml).

## Hard rules
- Banned **as direct dependencies** (transitive presence is fine, but ledgered): tokio/async runtimes,
  serde/serde_json, anyhow/thiserror, libc/nix/rustix, log/tracing, zstd, the `arrow` umbrella crate,
  parquet (no Parquet writer in the binary, D35). deny.toml enforces part of this.
- Every `[dependencies]` line has a `#` comment with its justification, mirrored by a row in `docs/deps.md`
  (checked by `cargo test --test budget`).
- Feature-unification-only deps (e.g. `lz4_flex` safe mode) say "NOT imported" in the comment.
- No dev-dependencies: tests use std, `ratatui::backend::TestBackend`, fixtures and an xorshift loop.
- Grouped upgrades only: `arrow-*` together; `ratatui*` + `crossterm` + `signal-hook` together.
  signal-hook stays on 0.3 (dependabot ignore ≥ 0.4) until crossterm requires 0.4.

## Commands
    mise run size         # size + crate gates, prints duplicates
    mise run unsafe-deps  # dependency rows of docs/unsafe.md vs the resolved tree
    mise run deny         # licenses, bans, advisories
    mise run msrv
    mise run build:musl   # the release binary still links statically
    cargo tree -d -e normal
