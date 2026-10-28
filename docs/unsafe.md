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
| `ahash` | Unaligned loads and `transmute`s of SIMD registers in its AES/fallback hash rounds | arrow-array's hasher (`runtime-rng` seed); bounded reads over its own input slices |
| `allocator-api2` | Raw allocation and pointer handling behind its `Allocator`-generic `Vec`/`Box` polyfill | hashbrown's dependency; widely audited std-equivalent code |
| `anstyle` | `str::from_utf8_unchecked` on its own ASCII escape-code buffer | clap's style type; one documented block over bytes it wrote itself |
| `arrow-array` | Unchecked element access and `ArrayData` construction from buffers it has validated; builders write into `MutableBuffer` spare capacity | The record/replay array layer; we never call its `unsafe fn`s (`new_unchecked`, `with_skip_validation`), so every decoded batch is validated |
| `arrow-buffer` | Raw aligned allocation, pointer arithmetic and `from_raw_parts` behind `Buffer`/`MutableBuffer` | The memory layer of every Arrow array; heavily tested and fuzzed upstream |
| `arrow-cmp` | Unchecked indexing in comparison kernels over lengths checked up front | Pulled in by arrow-select; we call no comparison kernel |
| `arrow-data` | `ArrayData::new_unchecked` and unchecked buffer views, guarded by its own `validate_full` | The IPC reader calls the validating path (validation stays on, §8.4) |
| `arrow-ipc` | Generated flatbuffer accessors (`gen/`), `with_skip_validation` (an `unsafe fn`), and `new_unchecked` constructions in the decoder that the skip-validation flag guards | We never call `with_skip_validation`; with validation on, the stream reader verifies every message (`root_as_message`) and validates the decoded arrays. Validation does not bound the LZ4 preallocation, so a crafted file can abort replay (§8.4): replay inputs must be trusted |
| `arrow-schema` | C Data Interface (`ffi.rs`) pointer handling, behind the off-by-default `ffi` feature | Feature not enabled; no unsafe reaches the binary |
| `arrow-select` | Unchecked `take`/`filter`/`coalesce` kernels over indices validated by the caller | arrow-ipc's dependency; we call no selection kernel |
| `bitflags` | `unsafe impl` of bytemuck `Pod`/`Zeroable` in macro output, behind the off-by-default `bytemuck` feature | Feature not enabled; no unsafe reaches the binary |
| `bytes` | Reference-counted shared buffers with raw pointers and vtables; `MaybeUninit` writes in `BufMut` | flatbuffers/arrow buffer dependency; widely audited (tokio) |
| `castaway` | `transmute`-based specialisation after a `TypeId` equality check | compact_str's dependency (ratatui); every cast is guarded by an exact type match |
| `chrono` | `from_utf8_unchecked` on ASCII it formatted itself; libc time-zone calls in `Local` | arrow-array's temporal types; we use no chrono API (timestamps are plain `i64`) |
| `clap_builder` | None in the library (`#![forbid(unsafe_code)]`); the hits are doc examples that call `std::env::set_var` | Doc text only |
| `clap_lex` | `OsStr::from_encoded_bytes_unchecked` to split arguments at ASCII boundaries | The std-sanctioned way to slice an `OsStr`; splits only at bytes it found |
| `compact_str` | Inline small-string representation: unchecked UTF-8 and raw pointer reads of its packed buffer | ratatui's cell text type; heavily fuzzed, invariants local to the crate |
| `crossterm` | `ioctl(TIOCGWINSZ)`, `tcgetattr`/`cfmakeraw`/`tcsetattr` and `close` on the tty fd | The terminal backend (ratatui feature `crossterm_0_29`); plain FFI on owned fds |
| `darling_core` | None; the hit is the string `"unsafe"` naming a syn expression kind | Compile-time only (instability macro); not in the binary |
| `derive_more` | None in the library; the hits are in its tests | crossterm's dependency; test text only |
| `derive_more-impl` | Generated `unreachable_unchecked` in a derived `PartialEq` match arm proven unreachable | Compile-time only; not used by crossterm's derives in our tree |
| `either` | `Pin::new_unchecked` projections in `as_pin_ref`/`as_pin_mut` | itertools' dependency; standard pin projection over owned variants |
| `errno` | Reads and writes the libc `errno` location through FFI | Via signal-hook-registry; the only way to access errno |
| `flatbuffers` | Unaligned little-endian reads in `follow` and table access after the `Verifier` has checked offsets | The IPC message format; arrow-ipc verifies each message before access |
| `foldhash` | Unaligned `read_unaligned` loads in the hash loop and a once-initialised global seed cell | hashbrown's default hasher; bounds-checked by construction |
| `getrandom` | `getrandom(2)` syscall FFI writing into a `MaybeUninit` buffer | Seeds ahash once (`runtime-rng`); plain syscall wrapper |
| `half` | SIMD intrinsics for f16 conversion and slice reinterpretation (`vec.rs`) | arrow-array's `Float16` type; we store no f16 columns |
| `hashbrown` | SwissTable: raw control bytes, SIMD group scans and unchecked bucket pointers | The std `HashMap` implementation itself; used by ratatui-core, ratatui-widgets, kasuari and lru |
| `iana-time-zone` | Platform FFI (CoreFoundation, Android, emscripten) to read the system time zone; Linux reads `/etc/localtime` safely | chrono `Local`; the platform-specific files are not compiled on Linux |
| `itertools` | `unreachable_unchecked` in `EitherOrBoth` helpers; most hits are benches | ratatui's dependency; two documented spots |
| `itoa` | Writes digits into a `MaybeUninit` buffer and returns it as `str` unchecked | Integer formatting in compact_str; small, fuzzed |
| `libc` | FFI declarations of the C library | Bindings only; every caller owns its own safety argument |
| `libm` | Architecture intrinsics (x86 FMA, i586 float control) in `math/arch` | num-traits' float fallback; std's float functions are used on our targets |
| `linux-raw-sys` | Generated kernel ABI structs and constants (`unsafe` in generated impls) | Bindings only, used by rustix |
| `lock_api` | `unsafe` raw-lock trait and guards that pair lock/unlock | parking_lot's lock frontend (crossterm) |
| `log` | `transmute` of the stored level and a `static mut`-style logger slot behind racy setters | mio's logging facade; we install no logger |
| `lru` | Intrusive doubly linked list over raw `NonNull` nodes in a HashMap | ratatui's layout cache; Miri-tested upstream |
| `lz4_flex` | Pointer-based fast copy and unchecked decode paths (`fastcpy_unsafe.rs`, `block/*`) | Our `safe-encode`/`safe-decode` features select the safe codec; the unsafe paths are compiled out but still flagged by the scan |
| `mio` | epoll/eventfd/pipe FFI and fd ownership | crossterm's event source (`event::read` in the input thread) |
| `num-bigint` | Slice reinterpretation in its digit arithmetic (`biguint/*`) | arrow-array's decimal support; we store no decimal columns |
| `num-complex` | `#[repr(C)]` layout casts between `Complex<T>` and `[T; 2]` | Via arrow-array; no complex values in our schema |
| `num-traits` | Float bit casts in `cast.rs` | The numeric trait layer under arrow; tiny audited crate |
| `once_cell` | Racy-init cells with atomics and `UnsafeCell` (`race.rs`, `imp_*`) | ahash's global seed cell; the reference implementation of std's `OnceLock` |
| `parking_lot` | Raw mutex/rwlock state machines and guard transmutes | crossterm's internal locks; widely used std alternative |
| `parking_lot_core` | Futex syscalls, thread-local parking slots and a global hash table of waiters | Under parking_lot; same justification |
| `proc-macro2` | `from_str_unchecked` for literals and a `ptr::read` out of a `ManuallyDrop` | Compile-time only (clap_derive); not in the binary |
| `rustix` | Raw Linux syscalls and fd handling over linux-raw-sys | crossterm's terminal I/O; safe wrappers are the crate's purpose |
| `rustversion` | None; the hits are the `unsafe` keyword it parses and its tests | Compile-time only; not in the binary |
| `ryu` | Float-to-string into a `MaybeUninit` buffer with unchecked UTF-8 and pointer writes | Via compact_str; formally verified algorithm, fuzzed |
| `scopeguard` | `ptr::read` out of a `ManuallyDrop` to defuse a guard; `unsafe impl Sync` | lock_api's dependency; tiny audited crate |
| `signal-hook` | Installs signal handlers through signal-hook-registry and writes to pipes in them | crossterm's SIGWINCH source; async-signal-safe by design |
| `signal-hook-registry` | `sigaction` FFI and a lock-free handler registry readable from signal context | Under signal-hook; same justification |
| `smallvec` | Inline/heap union with manual length and pointer management | parking_lot_core's dependency; widely audited |
| `static_assertions` | A never-called closure that `transmute`s to compare type sizes at compile time | Compile-time assertion only; no runtime code |
| `syn` | `unsafe impl Sync` for its thread-bound wrapper; most hits are the `unsafe` keyword it parses | Compile-time only (clap_derive); not in the binary |
| `thiserror-impl` | None; the hit is the keyword `unsafe` in its expression scanner | Compile-time only; not in the binary |
| `twox-hash` | Unaligned loads and SIMD in xxHash32/64/3 | Pulled in by lz4_flex's frame checksums; bounded by input length |
| `unicode-ident` | `get_unchecked` on its static lookup tables, indices bounded by construction | Compile-time only (via proc-macro2); not in the binary |
| `zerocopy` | Transmutes between types whose layout its derives prove compatible | ahash/half dependency; formally checked (Kani) upstream |
| `zerocopy-derive` | None in the library; the hits are expected-output test fixtures | Compile-time only; not in the binary |
