<!--
SPDX-License-Identifier: LGPL-3.0-or-later
SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany
-->

# unsafe ledger

Production code has no `unsafe` (`unsafe_code = "deny"`). Policy and decision tree:
`.claude/skills/unsafe-policy/SKILL.md` and design §12.

# Our code

One `## <n>. <file>:<item>` entry per `#[allow(unsafe_code, reason = "…")]` site in `src/`:
location, invariants, rejected safe alternative, and the test that exercises it.
`tests/budget.rs` checks that the number of entries equals the number of sites.

## 1. src/main.rs:alloc_count

- **Location.** `#[cfg(test)] mod alloc_count` in `src/main.rs`: `unsafe impl GlobalAlloc for
  Counting`, installed with `#[global_allocator]` in test builds only. The release binary has
  no unsafe code of ours.
- **Invariants.** `alloc` and `dealloc` forward their arguments unchanged to `System`, which
  upholds the `GlobalAlloc` contract. The counter is a `const`-initialised `thread_local!`
  `Cell<u64>` without destructor, so counting neither allocates nor re-enters the allocator;
  `try_with` skips counting during thread teardown. `realloc` and `alloc_zeroed` keep their
  default implementations, which call `alloc` and so are counted too.
- **Rejected safe alternative.** None exists: implementing `GlobalAlloc` is unsafe by
  definition, and no std API reports allocation counts. Measuring RSS instead cannot prove
  "zero allocations per tick".
- **Test.** `agent::tests::zero_allocations_per_steady_state_tick`: after a warm-up tick,
  100 steady-state sampling passes over the 25.11 fixture, encoded with `wire::write`,
  allocate 0 times.

# Dependencies

One row per crate in the resolved normal dependency tree whose source contains `unsafe`.
`scripts/unsafe-deps.sh` checks this table against the tree (no missing and no stale rows);
`scripts/unsafe-deps.sh --print` prints stub rows for missing crates.

| Crate | What its unsafe does | Why accepted |
|---|---|---|
| `anstyle` | `str::from_utf8_unchecked` on its own ASCII escape-code buffer | clap's style type; one documented block over bytes it wrote itself |
| `clap_builder` | None in the library (`#![forbid(unsafe_code)]`); the hits are doc examples that call `std::env::set_var` | Doc text only |
| `clap_lex` | `OsStr::from_encoded_bytes_unchecked` to split arguments at ASCII boundaries | The std-sanctioned way to slice an `OsStr`; splits only at bytes it found |
| `proc-macro2` | `from_str_unchecked` for literals and a `ptr::read` out of a `ManuallyDrop` | Compile-time only (clap_derive); not in the binary |
| `syn` | `unsafe impl Sync` for its thread-bound wrapper; most hits are the `unsafe` keyword it parses | Compile-time only (clap_derive); not in the binary |
| `unicode-ident` | `get_unchecked` on its static lookup tables, indices bounded by construction | Compile-time only (via proc-macro2); not in the binary |
