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

No entries yet.

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
