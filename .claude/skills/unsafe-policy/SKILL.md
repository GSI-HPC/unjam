---
name: unsafe-policy
description: Use whenever code would need `unsafe`, an FFI binding, raw syscalls, or when a dependency containing unsafe is added or bumped. Applies the decision tree and keeps docs/unsafe.md in sync.
---

# unsafe-policy

## Decision tree (stop at the first "yes")
1. Is there a safe std API? (`FileExt::read_at`, `CommandExt::{exec, process_group}`, `Command::env_remove`,
   `std::fs::remove_file` for `--unlink-self`, `/proc/self/{auxv,limits,stat}` instead of
   sysconf/getrlimit/getrusage)
2. Does an already-in-tree crate offer a safe API? (`signal_hook::flag`, `ratatui::crossterm`)
3. Can the feature be dropped or deferred? (Ask the owner.)
4. Only then: our own `unsafe`.

## If you must write unsafe
- Smallest possible block; one unsafe operation per block (`multiple_unsafe_ops_per_block = deny`).
- `#[allow(unsafe_code, reason = "<why the safe alternatives fail>")]` on the item.
- `// SAFETY:` comment stating every invariant relied upon (`undocumented_unsafe_blocks = deny`).
- New `## <n>. <file>:<item>` entry in the "Our code" part of `docs/unsafe.md`: location, invariants,
  rejected safe alternative, test that exercises it.
- Never `unsafe_code = "forbid"` in Cargo.toml (it would also bind test targets).
- Never `std::env::set_var`/`remove_var` (unsafe in edition 2024): change the child's environment only.
- Never call `unsafe fn` escape hatches in deps (e.g. arrow-ipc `with_skip_validation`).

## Check
    cargo test --locked --test budget      # allow(unsafe_code) count == "Our code" entries in docs/unsafe.md
    mise run unsafe-deps                   # every in-tree crate with unsafe has a dependency row, no stale rows
    grep -rn "unsafe" src/                 # every hit is under a ledgered allow
For dependency bumps: refresh the crate's row in docs/unsafe.md (see lean-deps step 6).
