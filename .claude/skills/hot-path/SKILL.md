---
name: hot-path
description: Use when editing src/agent.rs, src/source.rs, src/cgroup.rs, src/procfs.rs, src/wire.rs, src/state.rs, or the input, row or draw code in src/tui.rs. Enforces allocation, syscall, CPU and render budgets with measurements.
---

# hot-path: measured, allocation-free sampling and rendering

## Budgets (steady state, per tick)
| Item | Budget |
|---|---|
| Heap allocations in Sampler::tick + wire::write | 0 (test `alloc_count` must pass) |
| Syscalls per cgroup at level c (all files cached / none cached) | ≤ 10 / ≤ 30 |
| Extra syscalls per cgroup at level p (`cgroup.procs`) | ≤ 1 / ≤ 3 |
| Syscalls per process at level p (cached / uncached) | ≤ 2 / ≤ 6 |
| fd cache, job context | ≤ min(soft_nofile − 64, 1024) fds; hot files (cpu.stat, memory.current, cgroup.procs, /proc/pid/stat) always cached on the typical node |
| fd-cache hit ratio, typical node (70 cgroups at p, 100 procs) | ≥ 85 % of reads (IT 1/IT 5 record it) |
| Agent CPU, typical node (70 cgroups, 100 procs, 1 Hz) | ≤ 0.5 % of one core |
| Agent RSS | ≤ 16 MiB (cached-fd kernel memory, ≈ 5 MiB, is charged to the memcg, not RSS) |
| Recorder RSS at 1,000 entities | ≤ 8 MiB (batch cap 8,192 rows) |
| Output line | ≤ 1000 bytes incl. `\n` |
| TUI draw | O(visible rows); Rows built only for offset..offset+height; heat strip O(width) |
| TUI redraw on data | ≤ 4 Hz; decode work per batch ≤ 10 ms, then keys again; `State::derive` in the same slot |
| Flat layout at 100k processes | ≤ 1 ms sort per frame (`select_nth_unstable_by`, never a full sort) |
| CPU history | 16 × u16 per entity (32 B), updated in `State::apply`, no allocation per tick |
| Self-monitoring (header) | one cached `/proc/self/stat` pread per second; u64 counters, no per-line work beyond an add |

## Rules
- Reuse buffers (`Vec::clear`, never re-create); borrow via `Event<'a>`; no `format!`/`to_string` in loops: `write!` into a reused buffer.
- No `String` keys in per-tick maps; integer keys only. No per-row `HashMap` lookups in draw.
- Parse `&[u8]`; split `/proc/<pid>/stat` at the last `)`.
- Never block the sampler on output: only `Out::send_tick` (try_send). Never drop structure lines.
- The input thread only reads terminal events and sends `Msg::Key`/`Msg::Resize`; the main thread
  blocks on the one `Msg` channel and handles keys before lines. Never sample or decode elsewhere.
- A new per-tick file read needs a row in §5.3 of docs/design.md, a hot/cold tier decision in `Files`, and a
  measured per-file cost in `docs/perf.md` (rstat-flushing files: measure on a busy sind worker).

## Measure before and after (paste numbers into the PR)
    cargo test --locked alloc_count
    mise run bench                                       # ns/cgroup, ns/process, bytes/tick, apply ns/value
    mise run build:musl && mise run it:up                # nodes run the static binary from /data
    B=/data/target/x86_64-unknown-linux-musl/release/unjam
    sind ssh alice@worker-0 -- strace -c -f -o /tmp/s.txt $B --batch 20 >/dev/null
    sind ssh alice@worker-0 -- perf stat -e task-clock $B --batch 60 >/dev/null   # if perf is in the image (UNVERIFIED)
A regression > 10 % in any bench number or a budget breach blocks the PR unless the owner accepts it.
Update `docs/perf.md` with the new numbers.
