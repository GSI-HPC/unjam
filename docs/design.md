<!--
SPDX-License-Identifier: LGPL-3.0-or-later
SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany
-->

# unjam: architecture design

This is the living design of the repository. It started as the reviewed round-one design: the "minimal" design (the highest score, used as the base) with the grafts and must-fix items from three judge panels and the verified findings of a second review round. The owner's later decisions (D36 revised, D38 to D43) are folded into the text; §16 keeps the decision log with its review history. Facts checked against source, crates.io metadata, the sind docs (`docs/content/` in [GSI-HPC/sind](https://github.com/GSI-HPC/sind), v0.15.0), the sind-action README, or this repository are stated plainly. Anything not checked is marked **UNVERIFIED**. The implementation phases are in §19.

---

## 1. Summary

1. unjam is one Rust package that builds one binary. One viewer command takes its data from this host (`unjam`, the default), from a Slurm job (`unjam --slurm[=JOBID]`, "attach") or from recordings (`unjam --replay FILE…`). The subcommand `record` wraps a job step, and a hidden `agent` subcommand runs on each node. One sampler (`agent::Sampler`), one borrowed `Event<'a>` and one `State` serve every mode.
2. The view is the cgroup v2 tree. Processes come from each sampled cgroup's `cgroup.procs`. The Slurm job root is found by structure: strip the `step_*/user/task_*` suffix from `/proc/self/cgroup`. This works with both `job_<id>` directory names (25.11) and SLUID names (26.05).
3. Agents send self-describing tab-separated text lines. Every line is at most 1000 bytes and starts with the agent's node index. A `D` line announces each metric's name, unit and kind. Control lines go to every agent on srun's stdin. The sampler never blocks on stdout: a writer thread, a bounded queue, a per-agent byte budget, staggered emission and wall-clock phase-aligned ticks handle that.
4. Recording writes a crash-safe Arrow IPC stream with LZ4 compression. Each batch is a self-contained keyframe covering 10 s or 8,192 rows, so on-node memory stays at about 2 MB and at most about 10 s is lost on a crash. The schema is generated from the metric descriptors. There is no Parquet writer in the binary: a documented pyarrow one-liner converts a journal to Parquet, and IT 6 tests it (Q1 decides whether `unjam convert` comes back).
5. The TUI offers three layouts (step, node, flat), a self-monitoring header, a node heat strip, a CPU history bar per row and live recording of what it receives (D39 to D41).
6. Budget: about 3,400 LOC in 12 flat modules (hard cap 3,460, re-baselined from the planned 2,900 when record and replay measured about 580 lines against 330; see §10.2), 7 direct dependencies, no `unsafe` in production code (one ledgered use in a test-only allocator), and MSRV 1.88. Rust tests, scripts, mise tasks, CI gates and four Claude Code skills enforce these limits.

---

## 2. Requirements trace

| # | Requirement (brief) | Design element | § |
|---|---|---|---|
| R1 | top-like TUI whose top-level tree is the cgroup v2 tree, filled with processes from the cgroup nodes | `cgroup::walk` plus `cgroup.procs` on every cgroup at level `p`. `state::rows` places processes under their cgroup | 4, 5, 9 |
| R2 | Single-host base mode | `unjam [--root PATH]`: the sampler runs on its own thread and feeds the same text ingest path as remote agents | 3.4 |
| R3 | Attach via `srun --jobid --overlap`, one agent per node, streaming over srun stdio | `unjam --slurm[=JOBID]`; `slurm::srun_args` builds the exact argv (§3.2). The wire protocol is in §7 | 3, 7 |
| R4 | Show only the job's cgroups and processes | `cgroup::job_root`: strip `step_*/user/task_*` from `/proc/self/cgroup` and exclude the agent's own step. No `job_<id>` matching anywhere. IT 2 checks this against decoy jobs that run on the same nodes | 5.5, 14.2 |
| R5 | Up to 1k nodes and 100k processes in one TUI | Large-job defaults (cgroups only, step layout with folded node ranges), a per-agent byte budget, staggered emission, a bounded and tiered fd cache, O(visible) rendering, bounded decode work per loop, and self-CPU reporting. `tests` include a 1000-agent mux test | 5.2, 6, 7, 9 |
| R6 | Selective sampling | Rules `R <tasks> <pattern> <interval> <o\|c\|p>`, last match wins. `--slurm --nodelist` launches agents on a node subset only | 6 |
| R7 | Variable sample frequency per subset | Per-rule interval (100 ms to 60 s). Each distinct interval has its own phase-aligned timer | 6 |
| R8 | `srun unjam record -- ./app` uses the step as lifetime and process scope | Task with `SLURM_LOCALID=0` forks the app and samples `…/step_S/user`. Other tasks `exec` the app. It ends when the child has exited and the scope is empty | 3.5 |
| R9 | Arrow/Parquet record format, replay and offline analysis | Arrow IPC stream journal (`.arrows`), plus `unjam --replay`, plus a documented and IT-tested pyarrow conversion to Parquet | 8 |
| R10 | Abstract metric model; likwid and PAPI later | `trait Source` and `Probe` registry. Self-describing `D` lines on the wire. The record schema is generated from descriptors. A new source touches no wire, record or TUI code | 4.3, 4.4, 7, 8 |
| R11 | Round one: /proc, cgroup v2, Slurm job filter | `procfs::Proc`, `cgroup::Cgroup`, `cgroup::job_root` | 5 |
| R12 | mise for toolchain and tasks | `mise.toml`. CI runs the same tasks | 13.3, 18 |
| R13 | ratatui | `ratatui 0.30`, `default-features = false`, feature `crossterm_0_29` | 10 |
| R14 | Extremely small, efficient codebase; every item justified; codified as Claude Code skills | `docs/budget.txt` with `tests/budget.rs` (LOC per module, a `///` doc on every item, the dep ledger, the unsafe ledger), `scripts/size.sh`, `scripts/unsafe-deps.sh`, and four skills | 10, 11, 13, 17 |
| R15 | Minimise unsafe; justify every use in the docs | `unsafe_code = "deny"`, `docs/unsafe.md` ledger with a count check for our code, dependency rows checked mechanically against the resolved tree, and safe alternatives chosen on purpose | 12 |
| R16 | cgroup v2 only, Slurm 25.11+, MSRV chosen | v2 check at startup. Integration matrix covers 25.11, 26.05, and 26.05 with `CgroupJobIdPaths=yes`. MSRV 1.88 | 5.1, 13.1, 18 |
| R17 | Develop and integration-test on sind | `tests/sind/`, `mise run it`, and `GSI-HPC/sind-action@v2` in CI with pinned `version`, `wait` and `realm` | 14, 18 |
| R18 | GitHub Actions design | The existing `checks.yml`, `ci.yml`, `publish.yml` and `dependabot.yml` are extended, and `integration.yml` is new | 18 |
| R19 | Themes plug in (designed elsewhere) | `struct Theme` with semantic style slots and glyphs, plus a `const THEMES` table | 9.5 |
| R20 | REUSE, LGPL-3.0-or-later, cargo-release, dependabot, clap derive only | All kept. New files get SPDX headers. `REUSE.toml` covers `tests/fixtures/**` and `.claude/**`. cargo-release is pinned at 1.1.6 | 13, 18 |
| R21 | Owner review additions | Three layouts (D39), self-monitoring header (D40), heat strip, CPU history, `r`/`--record DIR`, folded node ranges (D41), built-in `--bcast` and static musl release binaries (D36 revised), one viewer CLI with source flags (D43) | 3, 9, 18 |

---

## 3. Process and runtime architecture

### 3.1 Modes

| Mode | Command | Processes | Threads |
|---|---|---|---|
| Local (default) | `unjam [--root /sys/fs/cgroup] [--interval 1s] [--rule R]… [--theme NAME] [--batch N]` | 1 | main (ingest, draw), input, sampler |
| Attach | `unjam --slurm[=JOBID] [--interval 2s] [--nodelist LIST] [--budget 4M] [--agent PATH] [--bcast[=DIR]] [--srun-arg ARG]… [--record DIR] [--rule R]… [--batch N] [--print-srun]`, plus the hidden `--agent-keepalive SECS` | unjam TUI, srun, one `unjam agent` per node | TUI: main, input, srun-stdout reader, srun-stderr reader. Agent: sampler, stdin reader, stdout writer |
| Record | `srun [opts] unjam record [-o PATTERN] [-i 5s] [--no-procs] -- CMD ARGS…` | Per node: one recorder plus the app. All other tasks are the app itself | Recorder: 1 |
| Replay | `unjam --replay FILE.arrows… [--batch N] [--at TIME]` | 1 | main, input |
| (internal) | `unjam agent [--root PATH] [--keepalive 120] [--unlink-self]` (hidden) | | |

- **CLI shape (D43).** One viewer command with a data-source flag; `record` and the hidden `agent` stay subcommands. `--slurm` and `--replay` form one mutually exclusive clap group. `--slurm` uses `num_args(0..=1)` with `require_equals(true)`: `--slurm=123` is valid, `--slurm 123` is an error. The Slurm-only flags (`--nodelist`, `--budget`, `--agent`, `--agent-keepalive`, `--srun-arg`, `--bcast`, `--record DIR`, `--print-srun`) use clap `requires = "slurm"`.
- **Job resolution** when `--slurm` has no value: 1. `$SLURM_JOB_ID`; 2. the only running job from `squeue --me`; 3. otherwise print the running jobs and exit 2. A future job picker replaces step 3 without a CLI change.
- **Input thread (D38).** In every viewer mode a dedicated thread blocks in `crossterm::event::read()` and queues `Msg::Key` and `Msg::Resize` on an unbounded key queue, then wakes the main thread through the channel it blocks on (§7.3). `q` (outside filter editing) and Ctrl-C set the quit flag in the input thread itself.
- `--rule` takes the text of a control-protocol `R` line (§6.2) **without** the leading `R\t`, for example `--rule $'*\t.\t1000\tp'`. It replaces the default rules. It is the hook the integration tests use.
- `--agent-keepalive SECS` (hidden, for tests) is forwarded to every agent as `--keepalive SECS`. The `srun_args` golden tests cover it.
- `--batch N` prints N text snapshots (`top -b`) using `state::rows` and exits. It replaces capture-and-assert scripts in the sind tests. Its footer reports `nodes=… stale=… errors=… dropped=…`.
- There is no async runtime: std threads plus `std::sync::mpsc` only (ADR D3).

### 3.2 Attach: launch sequence and exact srun line

1. Resolve the job (§3.1). Then `squeue --noheader --jobs=<J> --format="%A %D %T %U %j"` gives the numeric job id (array tasks resolve to the real id), the node count N, the state and the owner's uid of every matching row. An array job id (the id `sbatch` prints) lists every task plus the pending rest, which keeps the array id and sorts first (squeue sorts states as strings). unjam therefore takes the `RUNNING` row whose id is `<J>`, else the only `RUNNING` row; several `RUNNING` rows are listed like the jobs of §3.1 and unjam exits 2. With no `RUNNING` row, unjam exits 1 with the first row's state.
2. With `--nodelist LIST`, N becomes the line count of `scontrol show hostnames LIST`.
3. The TUI spawns srun with `Command::process_group(0)`, so terminal SIGINT and SIGTSTP never reach it. stdin, stdout and stderr are piped. The child environment is cleared of every `SLURM_*` and `SRUN_*` variable except `SLURM_CONF`, so input variables inherited from an enclosing `salloc` cannot add options we did not pass. This uses `Command::env_remove` on the child only, because `std::env::remove_var` is `unsafe` in edition 2024.

```
srun --jobid=<J> --overlap \
     --nodes=<N> --ntasks=<N> --ntasks-per-node=1 [--nodelist=<LIST>] \
     --cpu-bind=none --mpi=none \
     --input=all --quiet \
     --kill-on-bad-exit=0 --wait=0 \
     --job-name=unjam --chdir=/ \
     [--bcast=<DIR>/unjam.<J>.<uid>.<nonce>] \
     [<--srun-arg values>…] \
     <AGENT> agent [--keepalive <SECS>] [--unlink-self]
```

| Flag | Why |
|---|---|
| `--jobid=<J>` | Start a step inside the existing allocation. Slurm allows this only for the owner, an operator or root, which is exactly the authorization we want |
| `--overlap` | The 25.11 man page: "allows steps to share all resources (CPUs, memory, and GRES) with all other steps". The agent step never waits and never takes resources away from the job |
| `--nodes/--ntasks/--ntasks-per-node=1` | Exactly one agent per node, so `SLURM_PROCID` is the node index |
| `--nodelist` | Optional launch subset for very large jobs. Unselected nodes run no agent at all |
| `--cpu-bind=none` | Do not pin the agent onto one of the application's cores |
| `--mpi=none` | sind sets `MpiDefault=pmix` (sind `architecture/slurm-config.md`). The agent is not an MPI program and must not start a PMIx server |
| `--input=all` | Broadcast stdin to every task, which is the control channel. This is already the default and is stated for robustness |
| `--quiet` | Keeps srun's informational lines off stderr. Errors still reach the log pane |
| `--kill-on-bad-exit=0` | One failing agent must not kill the others (overrides a site `KillOnBadExit=1`) |
| `--wait=0` | Intended: unlimited wait after the first task exits, so a site `WaitTime` cannot kill the remaining agents when one dies. srun(1) 25.11 says `-W/--wait` "applies to job allocations", so its effect on a `--jobid` step is **UNVERIFIED**. IT 4 checks it: the IT cluster sets `WaitTime=10`, one agent is killed, and the others must keep ticking past 10 s. With 0, srun prints a warning after 60 s, which shows up in the log pane. If IT 4 shows the flag has no effect on steps, it is dropped, and `--kill-on-bad-exit=0` plus agents that exit 0 remain the mechanism |
| `--job-name=unjam` | Makes the step identifiable in `squeue -s` and `sacct` |
| `--chdir=/` | The TUI's working directory may not exist on compute nodes |
| **not** `-u/--unbuffered`, `--pty` | Both run tasks under a pty, which turns off `LAUNCH_BUFFERED_IO`, the line buffering §7 relies on, and adds CRLF translation |
| `--bcast=<DIR>/unjam.<J>.<uid>.<nonce>` | Only with `--bcast[=DIR]` (DIR defaults to `/tmp`): srun copies `<AGENT>` to every node. The path is explicit because srun(1) 25.11 says `--bcast` without a path defaults to the `--chdir` directory, which is `/` here. The nonce is 64 bits (16 hex digits) from std's `RandomState`, whose keys come from the OS random source; no file is read. It keeps concurrent viewers apart, even two of one user on one job, and keeps other users from guessing the name: srun always sets `BCAST_FLAG_FORCE`, so slurmd opens the destination with `O_CREAT|O_TRUNC`, not `O_EXCL`, and would write into, then exec, a file another user pre-created in a sticky `/tmp`. The nonce shows in srun's argv on the submit host only. `--bcast` into a job of another user (root or an operator attaching; `%U` of the squeue query in step 1 is the owner's uid) exits 1 and points to `--agent`: with `job_container/tmpfs` the job's `/tmp` belongs to its owner, who could replace the copy before srun execs it |
| **not** `--label` | Lines carry their own node index (ADR D6) |

- `<AGENT>` defaults to `std::env::current_exe()`, which assumes a shared filesystem. `--agent PATH` overrides it.
- `--print-srun` prints the argv and exits. A golden unit test pins `slurm::srun_args`.
- **Sites without a shared filesystem (D36 revised).** `--bcast[=DIR]` adds `--bcast=<DIR>/unjam.<J>.<uid>.<nonce>` (DIR defaults to `/tmp`) to the srun line and `--unlink-self` to the agent command. The agent removes its copy right after it starts (`std::fs::remove_file` on `current_exe()`; the running image stays mapped), so nothing is left on the nodes, even after a crash. A copy that is already gone (`NotFound`, for example when DIR is shared and another agent removed it first) counts as removed. About 30 LOC; IT 8 covers it. The static musl release binaries (§18) make the copy independent of the compute nodes' glibc. DIR must allow executables: sind (and sites that harden `/tmp`) mount `/tmp` `noexec`, where execve of the copy fails with "Permission denied" and the copy stays behind; IT 8 therefore uses `--bcast=/var/tmp`.

### 3.3 Attach: lifecycle and teardown

```
TUI ──spawn──> srun ──(slurmstepd per node)──> unjam agent (SLURM_PROCID = i)
TUI ──stdin pipe──> srun ──broadcast──> every agent's stdin            (control)
agent writer thread ──stdout──> slurmstepd (line-buffered) ──> srun ──> TUI stdout-reader thread
agent stderr ──> srun stderr ──> TUI stderr-reader thread ──> log ring / footer
```

- **Start.** The TUI writes the default rules to srun's stdin right after spawning it, and srun buffers them. Each agent first prints `H`, then `D` lines, then structure (`G`, `P`), all with a staggered delay (§7.5). After that it sends `T` ticks as the rules dictate. Before the first `R` arrives, every cgroup is `Off` (structure only).
- **Liveness.** The TUI sends `K` every 30 s. An agent exits on stdin EOF, on `EPIPE`, or after `--keepalive` seconds (default 120; set from `--agent-keepalive`) without any input. This guards against orphaned agents. Whether slurmstepd closes task stdin when srun is SIGKILLed is **UNVERIFIED**, and the keepalive makes the answer irrelevant.
- **One agent dies.** The other agents keep running (`--kill-on-bad-exit=0`, `--wait=0`, checked by IT 4). The dead node becomes stale in the TUI.
- **Quit (`q`, the end of `--batch`, SIGTERM or SIGHUP to the TUI):**
  1. Drop `ChildStdin`. srun forwards EOF, the agents exit 0, and srun exits.
  2. Poll `try_wait` every 50 ms for up to 3 s.
  3. Run `scancel --quiet <J>.<S>`. The step id comes from the agents' `H` lines.
  4. Wait 3 s more, then `Child::kill` (SIGKILL) and `wait`.
  5. `ratatui::restore()`.
- **Job ends while attached.** Slurm ends every step and srun exits. The TUI shows a `job ended` banner and keeps the last data until `q`.
- **srun fails** (policy, permissions). The terminal is restored, the stderr tail is printed, and unjam exits 1.

### 3.4 Local mode

- The local backend spawns a sampler thread that runs `agent::run(ctl_rx, out_tx, opts)`. This is the same function the remote agent runs, but with an `mpsc` control channel and a `sync_channel` of line buffers in place of stdin and stdout.
- The TUI therefore has a single ingest path, text lines, for local and remote alike.
- The UI thread never samples. A slow tick on a large host delays data, never keystrokes (ADR D15). Keys come from the input thread (D38).

### 3.5 Record (wraps a job step)

`#SBATCH -N 4 --ntasks-per-node=8` followed by `srun unjam record -o rec/%j.%s-%n.arrows -- ./app args`:

- **`SLURM_LOCALID != 0`.** `Command::new(cmd).args(..).exec()` (safe std). The task becomes the app, with the same pid and cgroup and no cost.
- **`SLURM_LOCALID == 0`, or no Slurm at all:**
  1. Spawn the app as a child with inherited stdio. The recorder never writes to stdout (`clippy::print_stdout = "deny"`).
  2. Determine the scope. If the own cgroup is `…/step_<S>/user/task_<T>`, the scope is `…/step_<S>/user` (every task of the step on this node). Otherwise (outside Slurm) the scope is the own cgroup. The docs recommend wrapping the command in `systemd-run --user --scope`.
  3. Sample the whole scope at `-i` (default 5 s) with processes on. Sampler events feed a local `state::State`, the viewer's store, and `record::Writer` writes each tick from it as a complete snapshot, with no wire encode or decode in the job. `--root DIR` records a cgroup subtree instead of the own scope (tests, and use outside Slurm); then only (a) applies.
  4. **Lifetime.** Stop when all of these hold: (a) the child has exited; (b) every other child directory of `user/` reports `populated 0` in `cgroup.events`; (c) the own task cgroup's `cgroup.procs` holds only our own pid and the pids that were there before the app started. Rule (c) catches daemonized descendants left in our own task cgroup. Outside Slurm, only (a) and (c) apply.
  5. Write the final batch and the end-of-stream marker, then exit with the child's code, or 128+signal.
- **Signals.** `signal_hook::flag::register` for SIGTERM, SIGINT and SIGHUP sets a flag. Signals are **not** forwarded: where Slurm signals every process in the step, forwarding would deliver signals such as USR1 twice. The recorder keeps sampling until the lifetime rule holds, so the app's shutdown phase is recorded too, but at most 5 s after the signal: `scancel --signal=TERM` reaches only the task leader (the recorder) on Slurm 26.05, so the app may never stop, and the journal must be finished before `KillWait` ends in SIGKILL. The recorder then exits with 128 + signal and Slurm reaps the rest of the step.
- **Output pattern.** Tokens `%j` (`SLURM_JOB_ID`), `%s` (`SLURM_STEP_ID`), `%n` (hostname), `%%`. A missing value becomes `na`. The default is `unjam-%j.%s-%n.arrows`. Files are created new with mode 0600 (they hold command lines) and are never overwritten.
- **Fallback.** If recording cannot start (the output file exists, its directory is missing, no cgroup2 scope), the recorder prints `unjam: record: <reason>; running unrecorded` to stderr and `exec`s the app, so the job still runs.

### 3.6 Replay and offline conversion

- **`--replay`** opens N `.arrows` files, one per node, builds a batch index for each, and moves a shared time cursor (§8.4).
- **Parquet** is produced offline with the documented pyarrow recipe (§8.1), never inside the job and never by unjam itself (Q1, ADR D35).

---

## 4. Data model

### 4.1 Entity tree

```
(virtual root: host | job J)
 └─ node i  (SLURM_PROCID; host name from H)       ← hidden in local mode
     └─ cgroup (id = directory inode, parent id, path relative to the agent root)
         ├─ cgroup …
         └─ process (pid, start)  ← from that cgroup's cgroup.procs
```

The precise cgroup v2 rule: a cgroup may hold processes and also have child cgroups unless domain controllers are enabled in its `cgroup.subtree_control` (the "no internal processes" rule). The root cgroup and threaded subtrees are exempt. unjam therefore reads `cgroup.procs` on **every** visited cgroup at level `p`, not only on leaves. In Slurm and systemd trees, processes normally sit in leaves.

### 4.2 Identities

| Entity | Live key (TUI) | Key in a recording | Reuse handling |
|---|---|---|---|
| Node | `u32` from the line prefix (`SLURM_PROCID`) | `unjam.host` schema metadata (one file per node) | n/a |
| Cgroup | `(node, ino)` | `(host, cg)` | A directory recreated under the same path gets a new inode, so the agent sends `X old` and then `G new`. Counters never go backwards within one id. `ino` comes from `DirEntryExt::ino()` (`d_ino` from getdents, no extra syscall). The root uses one `metadata()` call |
| Process | `(node, pid)` | `(host, pid, start)` | `start` is `/proc/<pid>/stat` field 22 (clock ticks since boot). A known pid with a new `start` means reuse: `x pid`, then `P pid …`. A cached `stat` fd returns `ESRCH` once its process has exited, which also catches reuse (§5.2). A process that moves to another cgroup gets a new `P` line with the new `cg` |

Paths are used for display and for rule matching only, never as identity.

### 4.3 Metric descriptors

```rust
pub struct Metric { pub name: &'static str, pub unit: &'static str, pub kind: Kind }
pub enum Kind { Counter, Gauge }   // Counter → displayed as rate
pub const NA: u64 = u64::MAX;      // missing value; keeps values at 8 B, no Option
```

- **Units** are strings from a small grammar: `us`, `B`, `1`, `char`. A future source may use any other unit string, and the TUI shows it raw with the unit appended. A string, not an enum, means a new source never needs a core edit.
- **Names** match `[a-z0-9_]+`. They start with the entity prefix (`cg_` or `p_`), and third-party sources add their own name after it (`p_likwid_flops_dp`). Names are **append-only and never renamed**. They are the wire, TUI and Arrow column identity.
- **No per-metric cost class** (ADR D12, revised). Reading `cpu.stat` and `io.stat` already triggers an rstat flush every tick (Linux v6.12: `cgroup_base_stat_cputime_show` calls `cgroup_rstat_flush_hold`, and `blkcg_print_stat` calls `cgroup_rstat_flush`). Gating `memory.stat` alone therefore saved little. The rule interval is the only cost knob. Phase 3 measures the real per-file cost (§5.4).

**Round one, cgroup (`cg_`):**

| Name | File | Unit / kind |
|---|---|---|
| `cg_cpu_usage_usec`, `cg_cpu_user_usec`, `cg_cpu_system_usec` | `cpu.stat` usage_usec, user_usec, system_usec | us / Counter |
| `cg_cpu_throttled_usec` | `cpu.stat` throttled_usec (only with the cpu controller) | us / Counter |
| `cg_mem_current_bytes` | `memory.current` | B / Gauge |
| `cg_mem_peak_bytes` | `memory.peak` (kernel ≥ 5.19, otherwise NA) | B / Gauge |
| `cg_oom_kill` | `memory.events` oom_kill | 1 / Counter |
| `cg_mem_anon_bytes`, `cg_mem_file_bytes` | `memory.stat` anon, file | B / Gauge |
| `cg_io_read_bytes`, `cg_io_write_bytes` | `io.stat`, rbytes and wbytes summed over devices, except disks below a dm or md device: blk-cgroup charges the cloned bio again on the lower disk, so LVM, dm-crypt and md would count twice. IO to a plain partition of such a disk is then not counted. In Slurm job cgroups only with `EnableExtraControllers=io` | B / Counter |
| `cg_pids_current` | `pids.current`. In Slurm job cgroups only with `EnableExtraControllers=pids` | 1 / Gauge |
| `cg_psi_cpu_some_usec`, `cg_psi_mem_some_usec`, `cg_psi_io_some_usec` | `{cpu,memory,io}.pressure` `some … total=` | us / Counter |

**io and pids under Slurm.** Slurm's cgroup/v2 plugin treats io, pids, hugetlb, rdma and misc as "extra controllers not explicitly tracked by Slurm" and leaves them disabled in the job subtree unless `cgroup.conf` sets `EnableExtraControllers` (slurm-25.11 `src/plugins/cgroup/v2/cgroup_v2.c:85-97, 505-517`; `cgroup.conf(5)` 25.11). Without that setting, `io.stat` and `pids.current` do not exist in job, step and task cgroups, so the three IO/PIDS metrics are NA in attach and record mode. The design handles this as follows:

- There is no site requirement (D42). The README's "Site notes" explain that `EnableExtraControllers=io,pids` enables the IO and PIDS columns in Slurm mode.
- Without it, the IO-R/s and IO-W/s columns hide (all-NA default columns are hidden, §9.2).
- The sind CI matrix runs 25.11 without the setting and both 26.05 entries with it (`EXTRA`). IT 2 and IT 6 assert non-NA io and pids values only where `EXTRA` is set, and the hidden IO columns and `~` PIDS elsewhere.
- Where `cg_pids_current` is NA, the TUI's PIDS column falls back to the summed `p_threads` of the processes sampled in that subtree, marked `~` (`State::derive`, §4.5). The fallback exists only where processes are sampled.
- A default column that is NA on every visible row is hidden (§9.2).

**Round one, process (`p_`):**

| Name | Source | Unit / kind |
|---|---|---|
| `p_cpu_user_usec`, `p_cpu_system_usec` | `stat` fields 14 and 15 × 1e6 / CLK_TCK | us / Counter |
| `p_rss_bytes` | `stat` field 24 × page size | B / Gauge |
| `p_threads` | `stat` field 20 | 1 / Gauge |
| `p_state` | `stat` field 3 (ASCII) | char / Gauge |
| `p_last_cpu` | `stat` field 39 | 1 / Gauge |
| `p_minflt`, `p_majflt` | `stat` fields 10 and 12 | 1 / Counter |
| `p_io_read_bytes`, `p_io_write_bytes` | `/proc/<pid>/io` read_bytes, write_bytes | B / Counter |

### 4.4 Source trait (the whole plug-in surface)

```rust
pub enum Ent { Cgroup, Proc }
pub enum Target<'a> { Cgroup { dir: &'a Path, files: &'a mut [Option<File>] }, Proc { pid: u32, files: &'a mut [Option<File>] } }
pub trait Source {
    fn entity(&self) -> Ent;
    fn metrics(&self) -> &'static [Metric];          // ordered; becomes this source's slice of columns
    fn sample(&mut self, t: Target<'_>, io: &mut Files, out: &mut [u64]) -> io::Result<()>;
}
pub type Probe = fn(&Env) -> Result<Box<dyn Source>, String>;   // Err(reason) → one W line, source skipped
pub const SOURCES: &[(&str, Probe)] = &[("cgroup", cgroup::probe), ("proc", procfs::probe)];
```

- `Sampler::new` probes every source and concatenates their metrics per `Ent`. The resulting offsets give each source its slice of the entity's value vector.
- A probe failure, for example a future likwid source blocked by `perf_event_paranoid`, becomes a `W` line. It never fails the agent.
- `Box<dyn Source>` costs one indirect call per source per entity, which is negligible.
- **Adding a source** may touch exactly: a new `src/<name>.rs`, one line in `SOURCES`, one line in `docs/budget.txt`, fixtures under `tests/fixtures/`, and a `docs/deps.md` row if it adds a dependency. It must not touch `wire.rs`, `record.rs`, `state.rs` or `tui.rs`. A new entity kind (Cpu, Device) is the one sanctioned core change (§17.1).

### 4.5 In-memory representation

- **Agent.** `HashMap<u64, CgEnt>` and `HashMap<u32, ProcEnt>`.
  - Each entry holds its metadata, a generation counter (to detect removed entities) and its cached fds (`[Option<File>; N]`).
  - Values are not stored, because every tick sends absolute values (ADR D8).
  - Reused buffers: one `Vec<u64>` for values, one `Vec<u8>` for reads, and line buffers recycled through the writer channel.
- **`Event<'a>`.** The interchange type. It borrows its data, so the hot path allocates nothing per event.

```rust
pub enum Event<'a> {
    Hello { proto: u32, version: &'a str, host: &'a str, step: &'a str, ntasks: u32, btime: u64 },
    Desc  { ent: Ent, idx: u16, name: &'a str, unit: &'a str, kind: Kind },
    Tick  { ts_ms: u64, self_cpu_us: u64, out_bytes: u64, dropped: u32, self_rss: u64 },
    Cg    { id: u64, parent: u64, path: &'a str },
    CgVals   { id: u64, first: u16, v: &'a [u64] },
    CgGone(u64),
    Proc  { pid: u32, start: u64, cg: u64, ppid: u32, comm: &'a str, cmd: &'a str },
    ProcVals { pid: u32, first: u16, v: &'a [u64] },
    ProcGone(u32),
    Warn(&'a str),
}
```

- **TUI `State`.**
  - `cols: Columns` interns `(Ent, name) → ColId` plus each column's unit and kind.
  - `nodes: Vec<Node>`, where `Node { host, cmap: [Vec<ColId>; 2] /* local idx → ColId */, cgs: HashMap<u64, Cg>, procs: HashMap<u32, Proc>, last_tick, stats }`.
  - `Cg` and `Proc` hold `v: Vec<u64>` and `pv: Vec<u64>` (indexed by `ColId`, grown lazily) plus `t` and `pt` (unix ms). `Cg` also holds `threads_sum: u64`, filled by `State::derive`.
  - A rate is `(v − pv) / (t − pt)` using `checked_sub`. A counter that goes backwards gives NA.
  - `State::derive(node)` runs at most at 4 Hz, before a row rebuild, for nodes that changed. It zeroes `threads_sum`, then adds each process's `p_threads` to its cgroup and every ancestor: O(processes × depth), with depth ≤ about 8. The PIDS column uses this value only when `cg_pids_current` is NA.
  - `Cg` and `Proc` also hold `hist: [u16; 16]`, a ring of CPU per mille per tick for the row's history bar (D41): 32 B per entity, about 3.2 MB at 100k.
  - At 100k processes with about 12 columns × 2 × 8 B plus about 100 B of strings, State is about 33 MB.

---

## 5. Collection

### 5.1 Startup checks and constants (std only, no libc in our code)

| File | Purpose |
|---|---|
| `/proc/self/cgroup` | Its `0::<path>` line gives the own cgroup. Hybrid hosts list v1 lines too; they are ignored, and the sampled root must be a cgroup2 directory. No `0::` line, or no cgroup2 at `/sys/fs/cgroup` without `--root`: exit with "cgroup v2 only" |
| `<cgroot>/cgroup.controllers` | Its existence proves the mount is cgroup2 |
| `/proc/self/auxv` | `AT_PAGESZ` (6) and `AT_CLKTCK` (17) as u64 pairs. Replaces `sysconf` |
| `/proc/sys/kernel/hostname` | Host name, sent in `H` |
| `/proc/stat` `btime` | Lets `start` be turned into wall time (sent in `H`, stored in record metadata) |
| `/proc/self/limits` `Max open files` (soft) | Sizes the fd cache without `getrlimit` |
| `/sys/class/block/*/holders`, `dev` | Disks that are, or have a partition that is, held by a dm or md device; `io.stat` lines of these `MAJ:MIN` are left out of the IO sums. Read once by `cgroup::probe` |

### 5.2 Read primitive and the bounded fd cache

- `Files::read(&mut self, slot: &mut Option<File>, hot: bool, dir: &Path, name: &str) -> io::Result<&[u8]>` is the single hot-path read routine:
  - If `slot` holds a `File`, it calls `FileExt::read_at(buf, 0)` (std `pread`), one syscall. The buffer doubles and the read repeats while a read fills it.
  - Otherwise it opens the file, reads it, and keeps the `File` only if a slot is free under the tier rule below. If not, it closes the file, giving three syscalls.
  - `ESRCH` from a cached `/proc/<pid>/stat` means the process is gone: close the fd and emit `x`.
- **Cap.** `cap = min(soft_nofile − 64, 1024)` in `agent` and `record` mode, and `min(soft_nofile − 64, 2048)` in local mode. The 64 fds of headroom cover stdio, `read_dir` and transient opens.
- **Tiers (priority policy, no eviction).**
  - **Hot files** are `cpu.stat`, `memory.current`, `cgroup.procs` and `/proc/<pid>/stat`. They are cached while `open_fds < cap`.
  - **Every other file** is cached only while `open_fds < cap − cap/8`.
  - The top eighth of the cap is reserved for hot files. Slots are otherwise first come, first served, and a slot is freed only when its entity goes away. This costs one comparison per open.
- **Typical job node** (70 cgroups at `p`: 10 value files plus `cgroup.procs` each, and 100 processes × 2 files ≈ 970 files):
  - With cap 1024: about 920 files are cached, and about 50 cold files (`memory.events`, pressure files) take the uncached path.
  - With a soft limit of 1024 (cap 960, a common job limit): about 880 files are cached.
  - Either way every hot file is cached. These numbers are estimates (**UNVERIFIED**). IT 1 and IT 5 record the real hit ratio in `docs/perf.md`.
- **memcg cost.** Each cached fd keeps a seq_file buffer of at least 4 KiB, allocated with `GFP_KERNEL_ACCOUNT` and charged to the agent's memcg, which in attach mode is under the **job's** memcg. 1024 fds is therefore about 5 MiB of job memory, an explicit and bounded cost. No `setrlimit` is needed.
- **Prompt close.** A cgroup or process's fds are dropped the moment its `X` or `x` is emitted (at the next discovery walk or the next `cgroup.procs` diff). Open fds on a removed cgroup keep it alive as a dying cgroup until they are closed, which is why this matters.
- **Startup self-check** (proves that `pread` at offset 0 regenerates content):
  - procfs: `pread` the cached `/proc/self/io` twice with a 4 KiB read of `/proc/self/stat` in between. `rchar` must increase.
  - kernfs: `pread` the own cgroup's `cpu.stat` twice around a busy loop of about 2 ms. `usage_usec` must increase.
  - If either check fails, `cap = 0` (always open, read, close) and one `W` line is sent. This is verified in sind (IT 1).
- Parsing works on `&[u8]` with byte-level number parsers. The `stat` line is split at the **last** `)`, because `comm` may contain spaces or `)`. `comm` and `cmdline` are converted lossily to UTF-8 when the process is first seen, and again when the `comm` in the `stat` read each tick no longer matches (an exec: wrapper scripts, container starters, the slurmstepd fork before the app). The process is then re-announced with the same pid, start and cgroup, so its values are kept. A `comm` that is not valid UTF-8 is not compared.

### 5.3 Files read, cadence and syscalls

Level semantics (§6): `o` reads nothing, `c` reads cgroup files, `p` adds processes.

| When | File | Syscalls (cached / uncached) |
|---|---|---|
| Discovery walk: each due tick of a sampled subtree (at most 1 Hz); `Off` subtrees every 10 s | `read_dir(<cg>)`, keeping directories, id from `ino()` | openat + 2 getdents + close |
| Cgroup, level ≥ `c` | `cpu.stat`, `memory.current`, `memory.peak`, `memory.events`, `memory.stat`, `io.stat`, `pids.current`, `cpu.pressure`, `memory.pressure`, `io.pressure` | 10 / 30 |
| Cgroup, level `p` | `cgroup.procs`, sorted and merge-diffed against the previous list into P and x lines | 1 / 3 |
| Process, level `p`, each tick | `/proc/<pid>/stat`, `/proc/<pid>/io` | 2 / 6 |
| New process, or `comm` changed (exec) | `/proc/<pid>/cmdline` once (NUL becomes space, cut to fit the line) | 3 |
| Agent self, each tick | `/proc/self/stat` (utime + stime for self-CPU reporting) | 1 |
| Record mode, each tick | `cgroup.events` of each `user/*` sibling, own `task_*/cgroup.procs` | 1 each |

A missing file (controller not enabled, old kernel) gives NA. The file is retried by the first discovery walk of each 10 s period, not by every walk, so a node without the io and pids controllers does not pay a failed open per absent file per tick.

### 5.4 Cost per tick (estimates, **UNVERIFIED** until measured)

The hot-path skill requires measuring these with `perf stat -e task-clock` and `strace -c -f` on a sind worker in phase 3 and keeping them in `docs/perf.md`. Phase 3 also measures each file's read cost on its own, so the per-file rstat assumptions below can be confirmed or corrected.

**rstat flushes.** `cpu.stat`, `io.stat` and `memory.stat` each request a flush. On v6.12, the `cpu.stat` flush also runs every subsystem's `css_rstat_flush`, memcg included. The walk is top-down, so the first read in a subtree is expected to pay for the flush and later reads to find little left to do. That expectation is **UNVERIFIED**.

| Item | Estimate |
|---|---|
| Cgroup at `c`, cached | ≈ 20–30 µs (10 small preads plus parsing, `memory.stat` the largest) |
| Cgroup at `c`, uncached | ≈ 50 µs |
| Process (stat + io), cached / uncached | ≈ 4 µs / ≈ 10 µs |
| Discovery walk | ≈ 8 µs per directory |

| Case | Per tick | CPU at 1 Hz |
|---|---|---|
| Typical job node: 70 cgroups at `p`, 100 processes, ≈ 90 % of files cached (§5.2) | ≈ 1.75 + 0.4 + 0.56 + 0.2 (uncached remainder) ≈ 2.9 ms | ≈ 0.3 % of one core |
| Large-job default: about 7 cgroups at `c` every 2 s, process subtrees `Off` | ≈ 0.2 ms | < 0.02 % |
| Busy local host: 300 cgroups, 2,000 processes, partly uncached | ≈ 30 ms | ≈ 3 % on the sampler thread; the UI stays responsive |

**Self-CPU accounting.** No claim is made that `nice` protects the job: the agent's overlap step competes with the application's steps through `cpu.weight`, so nice values do not help across cgroups.

- The agent reads its own utime, stime and rss from `/proc/self/stat` every tick and reports them in `T`. The TUI header shows the busiest node's CPU and RSS (D40, §9.1).
- The bounds on agent work come from the rule intervals, the byte budget and the large-job defaults, plus the hot-path budget of ≤ 0.5 % of one core on a typical node.
- An automatic interval stretch was cut in review round 2 (ADR D14). It is added only if phase-5 measurements show the agent above 1 % of a core under default rules (Q11). `T` parsing tolerates extra trailing fields, so adding a stretch field later needs no proto bump (§7.2).

### 5.5 Slurm job filter

The cgroup/v2 plugin layout, checked in the slurm-25.11 and slurm-26.05 copies of `doc/html/cgroup_v2.shtml`:

```
/sys/fs/cgroup/system.slice/[<nodename>_]slurmstepd.scope/   (slice configurable; IgnoreSystemd setups differ)
├── system/                          "slurmstepd infinity", stepd waiting area
└── job_<id>/   (25.11; 26.05 with CgroupJobIdPaths=yes)
    <SLUID>/    (26.05 default, CgroupJobIdPaths=no)
    ├── step_extern/{slurm, user/task_*}   pam_slurm_adopt'ed processes
    ├── step_batch/…                       batch script (first node)
    ├── step_interactive/…
    └── step_<n>/
        ├── slurm/          that step's slurmstepd (root-owned)
        └── user/
            ├── task_0/ … task_k/
            └── task_special/
```

The io and pids controllers are present in this subtree only with `EnableExtraControllers=io,pids` (§4.3).

**Algorithm (`cgroup::job_root`, about 25 LOC). It never searches for `job_<SLURM_JOB_ID>`:**

1. Read `/proc/self/cgroup`, which gives `0::<prefix>/<J>/step_<S>/user/task_<T>`.
2. Check that the last three path segments match `step_*`, `user` and `task_*`. Otherwise exit with "not inside a Slurm step (or the step runs in a cgroup namespace rooted below the job)".
3. The job root is `/sys/fs/cgroup` joined with `<prefix>/<J>`. The excluded subtree is `<J>/step_<S>`, the agent's own overlap step. That step directory must exist, otherwise exit with "own step not found (cgroup namespace)": in a cgroup namespace rooted at or above the job, with a host-view `/sys/fs/cgroup`, the path would be relative to the namespace, and the job root would silently become the whole mount.
4. `record` uses the same parse but takes `…/step_<S>/user` as its root.

Correction to the base design: if a site places steps in a cgroup namespace rooted at the task cgroup, `/proc/self/cgroup` reads `0::/` and step 2 fails, as it should. The base design's claim that the strip "still works" in that case was wrong. In sind, each node container has a **private cgroup namespace at the container root** (sind `architecture/overview.md`: `--cgroupns=private`). There, `/proc/self/cgroup` is relative to the container root and matches the container's `/sys/fs/cgroup`, so the strip works (expected; verified by IT 2 on all three matrix entries).

**Local mode on a compute node:** `unjam --root /sys/fs/cgroup/system.slice/slurmstepd.scope/<jobdir>`. There is no `--job J` lookup. The docs show how to find `<jobdir>` with `scontrol listpids J` and `/proc/<pid>/cgroup` (ADR D4).

**Permissions.** cgroupfs interface files are world-readable. `/proc/<pid>/io` needs ptrace-read access, which holds for the user's own processes. Otherwise the value is NA, and the first denied open sends one `W` line per agent (`<path>: permission denied, NA`, from `Files::denied`); later denials stay quiet.

---

## 6. Selective sampling and variable frequency

### 6.1 Rules (`model::effective`, shared by agent and TUI)

```rust
pub enum What { Off, Cg, Procs }                   // wire: o | c | p
pub struct Rule { pub tasks: TaskSet, pub pattern: Box<str>, pub interval_ms: u32, pub what: What }
```

- **Pattern.** A list of `/`-separated segments relative to the agent root. Each segment is a literal or `*`. A pattern matches a cgroup when it matches the cgroup path's **first k segments**, so a pattern always covers a whole subtree. `.` matches everything.
- **Resolution.** The **last** matching rule wins, like a firewall. With no match, a cgroup is `Off`. A process follows its cgroup's rule.
- **`Off` does not depend on the interval.** An `o` rule carries interval `-`. The parser rejects `-` with any other level, and rejects a numeric interval with `o`. This resolves the base design's contradiction between "interval 0 = off" and the clamp. `Off` keeps structure (`G`/`X`, rescanned every 10 s) and sends no values.
- **Interval.** Rounded to the nearest 100 ms and clamped to 100 ms…60 s (`model::interval_ms`, shared by `--interval`, `--rule`, `record --interval` and the `+`/`-` keys).
- **Phase alignment.** Deadlines are wall-clock multiples: `next = (now_ms / interval + 1) * interval`, using integer division (`model::next_due`, one unit-tested formula). The `+ 1` form makes sure that a tick which fires exactly on a multiple schedules the *next* multiple rather than itself, which a `ceil` form would not. All nodes therefore sample at the same instant, and sums across nodes compare like with like. This assumes NTP. The header shows the largest `T` skew across nodes. If the wall clock steps back, a deadline more than one interval ahead is re-phased to the new time (`Sampler::next_due`), so sampling does not pause for the size of the step. The keepalive is timed on the monotonic clock, so a forward step does not end the agent.
- **Timers.** The agent keeps one entry per distinct effective interval (usually 1–3) and sleeps with `recv_timeout` on its control channel until the earliest deadline. Missed deadlines are skipped, never caught up in a burst.

### 6.2 Control protocol (TUI → agents, srun stdin broadcast; local: mpsc)

```
R <TAB> tasks <TAB> pattern <TAB> interval_ms|- <TAB> o|c|p      add or replace the rule with this pattern
C <TAB> tasks                                                     clear all rules
S <TAB> tasks                                                     resync: resend H, D, all G and P (staggered)
B <TAB> tasks <TAB> bytes_per_s                                   per-agent output byte budget
K                                                                 keepalive
```

- `tasks` is `*` or ranges such as `0-15,32,40-41`, matched against `SLURM_PROCID` (0 locally). Agents ignore lines whose set does not include them.
- An `R` with the task set and pattern of an existing rule replaces that rule in place, so each agent's rule list stays bounded. Agent and TUI apply `R` with the same `model::upsert`, so an agent's list is the TUI's list restricted to the rules that apply to it. The TUI keeps the authoritative list and re-sends it after `S`.
- `--rule TEXT` is parsed as `R\t` + TEXT by the same `Rule::parse`.
- Malformed lines produce one `W` line and change nothing.

### 6.3 Defaults chosen by the TUI

| Job size | Rules | Budget |
|---|---|---|
| Local, or ≤ 16 nodes | `R * . <interval> p` | `B * 4194304/N` |
| > 16 nodes | `R * . <2×interval> c`, `R * */user - o` | `B * max(4096, 4194304/N)` |

For a large job this leaves about 7 cgroups per node (root, steps, `slurm/`) at 0.5 Hz: 1k nodes × 7 × ≈150 B / 2 s ≈ 0.5 MB/s. The kernel's hierarchical accounting means step cgroups already include their tasks.

The user marks nodes or paths and presses `p` to turn processes on where needed. On a node row, or on any row below one, a rule targets that node (`tasks = i`); on a cross-node row (the job or a step in the step layout) it targets `*`. To read a subtree more cheaply or more often, the user changes its interval with `+`/`-`.

---

## 7. Wire protocol (agent → TUI over srun stdout)

### 7.1 The transport facts it relies on

Verified in Slurm 25.11 source:

- `SLURM_IO_MAX_MSG_LEN 1024` (`src/common/io_hdr.h:75`).
- With buffered stdio (no `-u`), slurmstepd builds messages with `cbuf_read_line(…, SLURM_IO_MAX_MSG_LEN, …)`, so only whole lines are sent. Lines of 1024 bytes or more are cut (`must_truncate`, `src/slurmd/slurmstepd/io.c:1864-1901`).
- srun writes each message completely before starting the next (`src/api/step_io.c`, from reading the code). Whether this holds under 1k-task load is **UNVERIFIED**; it is checked by IT 5 and by the phase-5 throughput measurement.

### 7.2 Framing

- One event per line: fields separated by `\t`, ending in `\n`. **Each line is at most 1000 bytes including `\n`**, which leaves 24 bytes of margin under the 1024-byte limit. The cap is enforced in `wire::write`.
- **Every line starts with `<node>\t`**, the decimal `SLURM_PROCID`. Demultiplexing does not depend on srun's `--label` formatting. A line whose prefix is not a number goes to the log pane as `junk`.
- Strings are cleaned: bytes below 0x20 (including tab and newline) become `?`, and invalid UTF-8 is replaced lossily. `path` and `cmd` are cut at a char boundary to fit the cap. That is safe because paths are display only.
- NA is written as `-`.
- **Value lines carry an offset** (`first`). If a future source pushes an entity past the cap, `wire::write` splits its values over several `g`/`p` lines with increasing `first`. Old TUIs handle this without change.

| Line | Fields after `<node>\t` |
|---|---|
| `H` | proto (=1), version, host, step (`<job>.<step>` or `-`), ntasks, btime |
| `D` | ent (`c`/`p`), idx, name, unit, `counter`\|`gauge` |
| `T` | unix_ms, self_cpu_us, out_bytes, dropped_ticks, self_rss_bytes |
| `G` | id, parent_id (0 for root), rel_path |
| `g` | id, first, v… (descriptor order) |
| `X` | id |
| `P` | pid, start, cg_id, ppid, comm, cmdline |
| `p` | pid, first, v… |
| `x` | pid |
| `W` | text |

**Compatibility.**

- `proto` governs only the line grammar. A TUI refuses a node whose `H.proto` differs and shows the reason on that node's row.
- Different unjam **versions** with the same proto interoperate, because columns are matched by `D` names. Adding metrics never bumps `proto`.
- Parsers ignore extra trailing fields on `H`, `D` and `T` lines. A later field (such as a self-CPU stretch, or a cost class in `D`) can therefore be appended without a proto bump. `self_rss_bytes` on `T` was added this way (D40).

### 7.3 TUI receive path

- **One channel (D38).** The main thread blocks on a single `sync_channel(64)` of `Msg::{Key, Resize, Lines, Stderr, Tick}`. One blocking `recv` is simpler than polling with timeouts. Keys travel on a separate unbounded queue that the main thread drains before each decode batch; the input thread only `try_send`s a wake-up into the channel, so a key never waits behind 64 queued line chunks and the input thread never blocks.
- **Input thread.** Blocks in `crossterm::event::read()` and sends `Msg::Key` and `Msg::Resize`. A draw that blocks on a slow terminal (ssh) does not delay key reading, and `q` or Ctrl-C set a quit flag at once.
- **stdout reader thread.** `read()` into a 64 KiB buffer, split at the last `\n`, keep the remainder, and send whole-line chunks as `Msg::Lines`. A remainder that fills the whole buffer is dropped; the rest of that overlong line then arrives as one junk line. Unit tests drive the reader with short reads of varying size, and the 1,000-agent mux test feeds its interleaved stdout through it. It counts bytes for the header's `in` rate. The bounded channel makes srun's pipe fill up, which pushes back to the agents, whose writer threads absorb it (§7.5).
- **Main thread.** Handles keys first. Then, for each line: `wire::split_node` gives the node (and counts the line), `wire::parse(line, &mut vals)` gives an `Event`, and `State::apply` stores it. Decode work per batch is capped at 10 ms, after which pending keys are handled again, so key latency stays under about 50 ms even during an 8 MB attach burst. `Msg::Tick` (1 Hz) drives the self-monitoring sample and staleness.
- **stderr reader thread.** Sends `Msg::Stderr`. The last line goes to the footer, and the last 200 lines are kept in a ring for the log pane.

### 7.4 Error handling

| Situation | Response |
|---|---|
| Malformed line, or a value count that does not fit that node's `D` table | Increment the node's error counter, send `S <node>` (at most once per 10 s per node), show it in the status line. The resync re-sends only what the agent has, so the next `T` drops the cgroups and processes the node held at that `H` and did not re-announce (a lost `X`/`x`) |
| No `T` for 3 × the node's longest interval | Node shown as stale (dimmed) |
| Agent exits | srun's stderr says so, then the node becomes stale. The other agents keep running (§3.3) |
| srun exits | Every node becomes "ended"; data is kept |
| Agent write fails (`EPIPE`) | Agent exits 0 |

### 7.5 Agent output path: never block the sampler

- **Writer thread.** The sampler fills a line buffer per tick and calls `try_send` on a `sync_channel(4)` to a writer thread, which does blocking `write_all` to stdout. Buffers are recycled through a return channel, so there is no allocation in steady state.
- **Lossless drop policy.** Values are absolute (ADR D8), so a dropped tick only lowers time resolution. Rates over the next interval stay correct. Structure lines (`H D G X P x W`) are never dropped: when the queue is full or the budget is exhausted, they are kept in `pending` and sent ahead of the next tick's values. `dropped_ticks` reports the losses in `T`.
- **Byte budget.** A token bucket per agent, refilled at the `B` rate and capped at 2 s worth of tokens. A tick's value lines are sent only if tokens ≥ their size. The default total is 4 MiB/s divided by N.
- **Staggered emission.** Sampling stays phase-aligned, but the writer delays each send by `procid × min(interval/2, 250 ms) / ntasks`. srun and the TUI see a ramp, not 1k simultaneous bursts. The same spread applies to the start-up snapshot and to `S` resyncs.
- **Bounded memory.** The queue holds at most 4 buffers. `pending` grows only with structural churn and is capped at 1 MiB. Past the cap, the agent emits `W` and applies `R * . <longest active interval> c` to itself. That rule is valid under the §6.1 grammar, turns processes off everywhere, and bounds a fork storm. The TUI's next `C`/`R` from the user, or `0`, replaces it.

### 7.6 Bandwidth (estimates)

| Scenario | Rate into the TUI |
|---|---|
| Process line ≈ 100 B, cgroup line ≈ 160 B | — |
| Typical: 4 nodes × (100 processes + 70 cgroups) at 1 Hz | ≈ 85 KB/s |
| Large-job default at 1k nodes | ≈ 0.5 MB/s |
| Extreme: 100k processes at 1 Hz, all at `p` | ≈ 10 MB/s raw, capped by the 4 MiB/s default budget to ≈ 0.4 effective Hz per node, with drops reported |
| Attach start at 100k processes | ≈ 100 B × 100k ≈ 10 MB of P lines, spread over ≥ 250 ms by stagger and then limited by the budget |

srun's I/O thread throughput and CPU at 1k tasks are **UNVERIFIED**. They will be measured in phase 5 before any 1k-node numbers are claimed.

---

## 8. Recording and replay

### 8.1 Format decision

- **Journal (the one on-disk format):** an Arrow IPC **stream** (`.arrows`, `application/vnd.apache.arrow.stream`) with LZ4_FRAME body compression (`arrow-ipc` feature `lz4`, pure-Rust lz4_flex). It is crash-safe: every complete batch is readable.
- **Parquet, offline, outside unjam.** The README's "Recording" section documents this conversion, and IT 6 runs it:
  ```
  uv run --with pyarrow==21.0.0 python -c "import sys,pyarrow as pa,pyarrow.parquet as pq; \
    pq.write_table(pa.ipc.open_stream(sys.argv[1]).read_all(), sys.argv[2], compression='lz4')" X.arrows X.parquet
  ```
- **Why not Parquet in-job.** Parquet writes its footer only on close, so a recorder SIGKILLed after `KillWait` would leave an unreadable file. The IPC stream keeps everything up to the last batch.
- **Why no `unjam convert`** (cut in review round 2, ADR D35). The `parquet` crate with its `arrow` feature costs about +3 MB of binary, about 30 crates (arrow-select, arrow-cast, base64, num-bigint, twox-hash, seq-macro and more) and about 110 LOC. The Arrow IPC journal already meets the brief's "Arrow/Parquet" requirement, and pyarrow converts in one line. Q1 asks the owner whether to bring it back.

### 8.2 Schema (one stream per node, generated by `record::schema(&[Metric])`)

| Column | Arrow type | Null | Meaning |
|---|---|---|---|
| `ts` | `Timestamp(Millisecond, "UTC")` | no | Tick time (phase-aligned wall clock) |
| `kind` | `UInt8` | no | 1 = cgroup, 2 = process |
| `cg` | `UInt64` | no | Cgroup id. For a process row, the process's cgroup |
| `pid` | `UInt32` | yes | Process rows only |
| `start` | `UInt64` | yes | Process start, ticks since boot |
| `ppid` | `UInt32` | yes | Keyframe column |
| `path` | `Utf8` | yes | Cgroup path relative to the root; keyframe column |
| `comm`, `cmdline` | `Utf8` | yes | Keyframe columns |
| one `UInt64` per descriptor, named by `Metric.name` (`cg_cpu_usage_usec`, `p_rss_bytes`, …) | `UInt64` | yes | Field metadata `unjam.unit`, `unjam.kind` (`counter` or `gauge`), `unjam.source` (`cgroup` or `proc`, from the entity the metric belongs to). NA becomes null |

- **Schema metadata:** `unjam.schema=1`, `unjam.version`, `unjam.host`, `unjam.job`, `unjam.step`, `unjam.root`, `unjam.interval_ms`, `unjam.btime`.
- **Evolution rules.**
  - A new source appends metric columns automatically, with no code change in `record.rs`.
  - `unjam.schema` covers only the fixed columns. It is bumped only when a fixed nullable column is appended.
  - Readers select by name, ignore unknown columns, and treat missing ones as null, so older recordings stay readable.
  - Metric names are never renamed.
- **Semantics.** Each record-mode tick is a **complete snapshot** of the scope (fixed interval, processes on). An entity missing from a tick has gone. No gone-rows are written. Keyframe columns are filled only on the first row of each entity in a batch. Analysts who want them on every row forward-fill per entity: `cg` for cgroup rows, `pid` for process rows (a reused pid starts a new entity, whose first row is a keyframe again). `start` cannot be part of the key, because it is itself a keyframe column. The README shows the pandas form.

### 8.3 Lifecycle and crash safety

1. Create new with mode 0600 (`create_new`, `OpenOptionsExt::mode`) → `BufWriter` → `StreamWriter::try_new_with_options(…, IpcWriteOptions::default().try_with_compression(Some(CompressionType::LZ4_FRAME)))`. The schema message is written at once.
2. Rows go into arrow-array builders. A batch is cut every **10 s or 8,192 rows**, whichever comes first, then written and flushed.
   - Value memory at the row cap: 8,192 rows × about 30 UInt64 columns × 8 B ≈ 2.0 MB. Validity bitmaps, builder over-allocation and the Utf8 keyframe columns add about 20–50 %, so the bound is about 3 MB.
   - Typically about 100 KB. At 1,000 entities and a 5 s interval, a 10 s batch holds 2,000 rows.
   - A scope larger than 8,192 entities spreads one tick over several batches. Each batch is still self-contained under the keyframe rule.
3. **Keyframe rule.** The first row of each entity in a batch carries `path`, `comm`, `cmdline`, `ppid` and `start`, so every batch decodes on its own.
4. Normal end or SIGTERM: write the last batch, then `finish()` (end-of-stream marker).
5. SIGKILL, OOM or a node crash loses at most the open batch (≤ 10 s). Readers return every complete batch, then report "truncated at byte N".
6. There is no `fsync` on the hot path. It is expensive on Lustre and GPFS, and the page cache survives a process kill. Node-crash durability is Q4.

### 8.4 Replay and seeking (`.arrows` only, one reader path)

- **Open.** Wrap the buffered file in `Counting`, a `Read` that counts bytes. `StreamReader::try_new(counting, Some(vec![0]))` decodes only `ts`. `try_new` is unbuffered in arrow-ipc 60, so the count is exact. Keep the schema message bytes, and record `index: Vec<(offset, first_ts)>`.
- **Seek(t).** Backward, rebuild each node from the batch holding t − 120 s (twice the longest rule interval), so every entity's previous sample is applied and its rates match forward play: at most about 13 batches per file, as batches are cut every 10 s. Binary-search the index, then `StreamReader::try_new(Cursor::new(schema_bytes).chain(file_at(offset)), None)`, decode that one batch, and apply ticks up to t. Validation stays on; `with_skip_validation` (an `unsafe fn`) is never called.
- **Step.** Forward moves to the next tick and loads the next batch when needed. Back is `seek(t − interval)`.
- **Several files** share one cursor over sampling rounds, and each file shows its latest tick ≤ t. Agents stamp a round a few ms after its due time, differently on each node, so ticks closer than half the shortest tick gap in any file form one round, and the cursor stands at the round's latest tick: one step advances every node by one tick. Rates use tick t and the tick before it, across batch boundaries. Applying a snapshot tick is `State::apply` per row followed by `State::retain_tick(node, ts)`.
- **Malformed files.** A row whose parent path resolves to the cgroup itself or to one below it (possible only in a crafted or corrupted file) is ignored by `State`, which also moves a re-parented cgroup out of its old parent's kids, so the layouts always walk a tree. Replay inputs must still be trusted: arrow-ipc 60 preallocates an LZ4 buffer at the 8-byte uncompressed length the file states (`decompress_lz4`, `Vec::with_capacity`) before any check, so a crafted length aborts replay with an allocation failure, past the panic hook. A pre-scan of every buffer prefix would cost more lines than it is worth; the fix belongs upstream (a capped preallocation), followed by a dependency bump.
- **Memory.** Only the current decoded batch per file is held: typically about 250 KB per node and at most about 3 MB (the row cap), so about 250 MB at 1k nodes in the typical case (K8).

### 8.5 Recording in the TUI (D41)

- `r` starts or stops recording in a live viewer; `--slurm --record DIR` starts it at launch. Without `--record`, `r` records into `unjam-rec-<secs>` in the current directory.
- The TUI feeds the events it receives into `record::Writer`, one `.arrows` file per node (`DIR/<host>.arrows`, `/` in the host name replaced by `_`, created new with mode 0600; a missing `DIR` is created with mode 0700, since the files hold other users' command lines when root records). The schema metadata carries `unjam.source=tui`, so a reader can tell such a file from a `record` journal. Batching, keyframes and crash safety are those of §8.3; replay reads both.
- A node's tick is written when its next tick starts, at that node's first structure or `T` line after it: an agent sends a tick's structure before its `T`, so writing at `T` would mix in the next tick's structure (entities that just exited would lose their last sample).
- Values arrive at the rule intervals that are active, so a TUI recording is as sparse or dense as the view was. About 50 LOC.

### 8.6 Interop claims (each tested in IT 6)

- **pyarrow:** `pa.ipc.open_stream` reads `.arrows`, including field metadata, and the §8.1 recipe writes a Parquet file that `pq.read_table` reads back with the same row count.
- **Not claimed** (**UNVERIFIED**): polars reading LZ4 IPC streams, DuckDB reading `.arrows` directly, and DuckDB on the converted Parquet (expected to work, but not tested, so not stated).
- uv and pyarrow stay in the toolchain even though `convert` is gone. They test the exact recipe that replaces `convert`, and they cost nothing in the binary (justification for keeping that part of the bloat item; DuckDB was dropped).

---

## 9. TUI

### 9.1 Layout (80×24 minimum, grows with the terminal)

```
 unjam --slurm=4821937 · 1024 nodes (2 stale) · 98,304 procs · 2s · step · sort CPU% ▼ · 12:00:01Z
 tui 1.2% 38M · in 410 KB/s 3.1k msg/s · out 12 B/s · agents max 0.3% 12M (n0417) · drop 0 err 0
 NODES ███▓▓▒▒░░·····▓▓▒▒░░░░········▓▓▒░░░░░░·····   cell = 16 nodes, colour = CPU band, · = off
 NAME                              CPU history        CPU%    MEM  IO-R/s IO-W/s  PIDS  INT  W
 ▼ job 4821937                     ▆▆▆▆▅▆▆▆▆▆▆▆▆▆▆▆  62.4%   9.8T   38G/s   3.1G  98304   2s  c
   ▼ step_0                        ▆▆▆▆▅▆▆▆▆▆▆▆▆▆▆▆  62.4%   9.8T   38G/s   3.1G  98304   2s  c
     ▼ n0001                       ▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇  88.0%   9.6G   90M/s    2M     96   2s  p
       ▼ task_0                    ▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇  91.5%   9.5G   88M/s    2M     34   2s  p
           31002 R python train.py ▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇  91.0%   9.1G   88M/s    2M      ·
     ▶ n0417                       ▂▂▂▁▁▁▁▁▁▁▁▁▁▁▁▁  12.3%   8.9G    2M/s     0     96   2s  p
     ▶ n0418 … n1024 (606 folded)  ▆▆▆▆▆▆▆▆▆▆▆▆▆▆▆▆  61.0%   5.9T   23G/s   1.8G  58000   4s  c
 ────────────────────────────────────────────────────────────────────────────────────────────
 ↑↓ move ←→ fold ␣ mark p what +/- rate v layout s sort / filter r record t theme ? help q quit │ srun: …
```

- **Header, line 1:** the source (`unjam`, `--slurm=J` or `--replay`), node count with stale nodes, process count, interval, layout, sort column and clock. Replay shows the cursor time and `[▶]` or `[⏸]`.
- **Header, line 2: self-monitoring (D40, always shown, local mode too).**
  - `tui`: CPU and RSS of the whole TUI process. The main thread `pread`s a cached `/proc/self/stat` (utime, stime, rss) once per second; CPU% is over wall time and covers all TUI threads.
  - `in` (bytes and lines per second into the TUI) and `out` (control bytes to the agents): the stdout reader counts bytes, `split_node` counts lines per node, `Remote::send` counts control bytes. Plain u64 counters, rate over the last second.
  - `agents max`: the highest agent CPU (rate of `T.self_cpu_us`) and RSS (`T.self_rss_bytes`), with that node's name. In local mode the sampler is a TUI thread and counts under `tui`.
  - `drop` and `err`: summed dropped ticks and decode errors.
  - About 30 LOC. The help pane adds a per-node table: CPU, RSS, out B/s, msg/s, drops, errors and clock skew.
- **Node heat strip (D41, Slurm mode).** One cell per ⌈N / width⌉ nodes. A cell's colour is the `theme.heat` band of the highest CPU% among its nodes; a cell whose nodes are all `Off` is dotted. About 40 LOC.
- **CPU history bar (D41).** Every row draws its last 16 CPU samples from `hist` (§4.5) as a bar sparkline in `theme.heat` colours. About 35 LOC.

The mock assumes `EnableExtraControllers=io,pids`. Without it, IO-R/s and IO-W/s are hidden and PIDS shows `~`-marked summed threads where processes are sampled (§4.3, §9.2).

### 9.2 Layouts and views

- **Three layouts (D39)** are level orders in `View`, walked by the one `state::rows`. `v` cycles step → node → flat. Fold state is keyed by `(level, key)`.
  - **step** (the Slurm default): job → step → node → task → process. A node row under a step is that node's `step_N` cgroup, measured by the kernel. Only the job and step rows are computed across nodes: Counter rates and Gauges are summed, and `p_state`/`char` columns show `*` when the nodes differ. Phase alignment means the sums compare samples from the same instant.
  - **node**: node → cgroup tree → processes.
  - **flat**: one row per process, or per task cgroup where processes are `Off`, with a NODE column and one global sort. In local mode this is the classic `top`.
  - Local mode has no node level: `v` toggles between the cgroup tree and flat.
- **Sort.** The sort column orders siblings only; a row never leaves its parent. Tree layouts sort every sibling set at every level by the row's own value; flat has one sibling set, so its sort is global. Counter → rate, Gauge → value, NA always last in both directions, NAME and NODE in natural order (NODE sorts by host, then NAME), ties in natural order. The cursor follows its entity across redraws.
- **Folded node ranges (D41, step layout).** Under a step, the top nodes by the sort column fill the window, and the remaining nodes collapse into one aggregate row such as `n0418 … n1024 (606 folded)`. Unfolding it lists them. About 25 LOC.
- **Not in round one:** a "by command" layout and a RANK column. Rank labels ("rank 0-95") need `SLURM_PROCID` from `/proc/<pid>/environ` once per new task, which is not designed yet.
- **Detail** (`Enter`): every column the node announced, including future-source metrics, shown raw and as a rate, plus the full path or cmdline. Where a value is NA because a controller is absent, it says so.
- **Columns.** The table shows a fixed default list of metric names. Unknown metrics appear only in Detail. There is no column chooser in v1 (ADR D22).
  - A default column that is NA on every row in the visible window is hidden for that frame. This check runs only over the visible slice.
  - PIDS uses `cg_pids_current` and falls back to `~` plus the summed `p_threads` from `State::derive`.
- **Help/log** (`?`): key bindings, the last 200 stderr lines, and the per-node table of §9.1.

### 9.3 Key bindings

| Key | Action |
|---|---|
| `↑/↓ k/j`, `PgUp/PgDn`, `Home/End g/G` | Move |
| `←/→ h/l` | Fold / unfold (also a folded node range) |
| `Enter` | Detail popup |
| `space` | Mark or unmark the row (node or cgroup); `a` clears marks |
| `p` | Cycle the level of the marked rows (or the cursor row): `p` → `c` → `o` → `p`. Sends `R` |
| `+` / `-` | Halve / double the interval of the marked or cursor rows. Sends `R` |
| `0` | Reset rules to the defaults (`C *` plus the defaults) |
| `v` | Next layout: step → node → flat (local: tree ↔ flat) |
| `s` / `S` | Next sort column (NAME, the value columns, then NODE when attached) / reverse |
| `/` | Substring filter on name, comm or path; `Esc` clears |
| `r` | Start or stop recording what the TUI receives (§8.5) |
| `t` | Next theme |
| `?` | Help and log |
| `q` | Quit (teardown in §3.3) |
| Replay: `space`, `,` / `.`, `<` / `>`, `g` / `G` | Play/pause, ±1 tick, ±60 s, start/end |

### 9.4 Rendering cost

- `state::rows(&State, &View, height, &mut Vec<Row>)` walks only unfolded subtrees, with `Row = (node, kind, key, depth)` in a reused `Vec`.
  - Sibling sets of at most `height` rows get a full sort.
  - Larger sets (process lists) use `select_nth_unstable_by` on the first `scroll + height` rows.
  - The walk stops once `scroll + height` rows have been produced.
- `draw` builds ratatui `Row`s and `Cell`s **only for the visible slice**. Numbers are formatted into one reused `String` arena per frame.
- The flat layout at 100k processes costs about 1 ms per frame (`select_nth_unstable_by`). Folded node ranges keep the step layout's row count bounded at any node count.
- Rows are rebuilt and redrawn at most at 4 Hz for data-driven changes, and immediately on a key press. `State::derive` runs in the same 4 Hz slot. ratatui's buffer diff writes only changed cells.
- `--batch N` renders the same `rows` as text.

### 9.5 Themes

```rust
pub struct Theme {
    pub name: &'static str,
    pub base: Style, pub header: Style, pub dim: Style, pub accent: Style,
    pub selected: Style, pub marked: Style,
    pub warn: Style, pub crit: Style,          // stale nodes, errors, OOM, drops
    pub tree: Style,                           // fold glyphs / guides
    pub heat: [Color; 4],                      // value bands: idle, low, high, saturated
    pub glyphs: Glyphs,
}
pub struct Glyphs { pub open: &'static str, pub closed: &'static str, pub guide: [&'static str; 2] }
pub const THEMES: &[Theme] = &[NORD, NIGHT, MOCHA, GRUVBOX, SOLAR, AMBER, MONO];
```

- **Themes:** `nord` ("Nord Frost", the owner's choice and the default, so first), `night`, `mocha`, `gruvbox`, `solar`, `amber` and `mono`.
- **Selection order:** `--theme NAME`, then `UNJAM_THEME`, then `THEMES[0]`. If `NO_COLOR` is set, `mono` is forced and must exist (a unit test checks this). `t` cycles at runtime.
- Themes are compiled-in `const` data: no theme files and no parser dependency (Q5). The table sits between `// budget: data` and `// budget: end`, outside the theme.rs line budget.
- The renderer reads styles only from these slots, so a new theme is one array entry.

---

## 10. Crate, modules and dependencies

### 10.1 Layout

One package, one binary. No lib target, no workspace, no cargo features of our own: one build configuration means one test matrix.

```
src/main.rs      CLI (clap derive: viewer flags with the source group, record, agent), dispatch, exit codes, parse_dur
src/model.rs     Metric/Kind/Ent/NA, Event, What/Rule/TaskSet/Control, effective(), next_due()
src/source.rs    trait Source, Target, Env, Probe, SOURCES, Files (tiered fd cache + self-check), kv(), kv_eq()
src/cgroup.rs    Cgroup source, walk(), procs(), io_sum(), psi(), self_cgroup(), job_root()/record_root(), v2 check
src/procfs.rs    Proc source, stat(), cmdline(), auxv(), hostname(), btime(), nofile()
src/agent.rs     Sampler (rules, timers, discovery, emit, self-CPU/RSS accounting), run(), Out (writer, budget, stagger), --unlink-self
src/wire.rs      Event and Control text codec, split_node(), line cap and value splitting
src/slurm.rs     job resolution, squeue/scontrol queries, srun_args() (incl. --bcast), spawn and reader threads, Remote teardown
src/record.rs    run_record(), schema(), Writer (also for TUI recording), Reader (index, seek), Counting
src/state.rs     State/Node/Cg/Proc/Columns, apply() (incl. CPU history), retain_tick(), derive(), rate(), rows() (3 layouts, sort, filter, folded node ranges)
src/tui.rs       App, Feed, Msg, input thread, event loop, keys, draw (header, self-monitoring, heat strip, history bars), fmt, batch output, recording toggle, default_rules()
src/theme.rs     Theme, Glyphs, THEMES, pick()
```

### 10.2 LOC budget

Counted as code lines, excluding blanks, comments and `#[cfg(test)]` modules. Recorded in `docs/budget.txt`. `tests/budget.rs` fails above +10 %.

| Module | LOC | Module | LOC |
|---|---|---|---|
| main.rs | 125 | wire.rs | 200 |
| model.rs | 160 | slurm.rs | 175 |
| source.rs | 95 | record.rs | 330 |
| cgroup.rs | 190 | state.rs | 360 |
| procfs.rs | 130 | tui.rs | 600 |
| agent.rs | 275 | theme.rs | 60 (+ theme data) |
| | | **Total** | **≈ 2,700** (hard total cap 2,900) |

The table is the plan. As built, the hard total cap is **3,460** (owner decision pending): record and replay measured about 580 lines against the planned 330, and the sampler (agent.rs) about 490 against 275, so `docs/budget.txt` holds the measured module budgets with a one-line reason each and is the authoritative gate. Modules that do not exist yet are skipped by the gate, and every file in `src/` needs a line. Theme data sits between `// budget: data` and `// budget: end` and is not counted. A budget may be re-baselined with a one-line reason in `docs/budget.txt`.

Changes from the owner review: the three layouts (+50 in state.rs), self-monitoring (+30), the heat strip, CPU history, TUI recording and folded node ranges (+150, D41), `--bcast` with `--unlink-self` (+30) and the input thread (+10).

Changes from review round 2:

- Removed: `convert.rs` (−110), the Cost machinery (about −15), the self-CPU stretch (about −30) and `--bcast` (about −5).
- Added: tiered fd cache (+5), `State::derive` PIDS fallback (+15), all-NA column hiding (+5) and the keepalive forward (+2).

Tests: about 900 LOC in modules, plus `tests/budget.rs` (≈ 120 LOC) and fixtures.

### 10.3 Direct dependencies

Versions as recorded in the judges' crates.io checks of 2026-10-27. Each line in `Cargo.toml` carries its justification as a comment, and `docs/deps.md` has the full row.

```toml
[dependencies]
# CLI; repo convention "clap derive only". No color/suggestions/wrap_help → no anstream/strsim/terminal_size.
clap = { version = "4.6", default-features = false, features = ["std", "derive", "help", "usage", "error-context"] }
# TUI (owner-accepted). Only the crossterm 0.29 backend; no all-widgets/macros/layout-cache of our own choosing.
# NOTE: ratatui's crossterm backend pulls ratatui-crossterm with default features, whose defaults are
# crossterm_0_29 + underline-color (→ ratatui-core/underline-color), so underline-color IS enabled; counted in budgets (D27).
ratatui = { version = "0.30", default-features = false, features = ["crossterm_0_29"] }
# Record + replay: builders/RecordBatch, Schema+field metadata, IPC stream (lz4 = pure-Rust lz4_flex, not C zstd).
arrow-array  = { version = "60", default-features = false }
arrow-schema = { version = "60", default-features = false }
arrow-ipc    = { version = "60", default-features = false, features = ["lz4"] }
# NOT imported. Feature unification: arrow-ipc enables lz4_flex {std, frame} (unsafe codec);
# adding safe-encode/safe-decode switches lz4_flex to its safe implementation.
lz4_flex = { version = "0.14", default-features = false, features = ["safe-encode", "safe-decode"] }
# SIGTERM/SIGINT/SIGHUP → flag (recorder finalizes, TUI tears down). Same 0.3 that crossterm 0.29 requires (^0.3.17)
# → 0 new crates. Held below 0.4 by a dependabot ignore until crossterm moves (docs/deps.md).
signal-hook = { version = "0.3", default-features = false }
```

- crossterm is used through the `ratatui::crossterm` re-export, not as a direct dependency. `ratatui-crossterm` pulls crossterm with **default features** (`default_features = true`, confirmed by a judge), which brings `bracketed-paste`, `events` and `derive-more`. Its own defaults also turn on `underline-color`. That cost is counted in the size and crate budgets; a `[patch]` is not worth it (ADR D27).
- **No dev-dependencies.** Tests use std, `ratatui::backend::TestBackend`, fixture trees and an xorshift loop.
- **Rejected (as direct dependencies):**

| Crate(s) | Reason |
|---|---|
| tokio / async | Three threads per process suffice; about +20 crates |
| serde, bincode, postcard, base64 | The wire is text |
| anyhow, thiserror | `io::Error` plus `Box<dyn Error>` in `main`. thiserror is in the tree anyway through ratatui-core, which is acceptable; we do not use it ourselves |
| rustix, libc, nix (direct) | auxv, `/proc/self/limits`, `FileExt::read_at`, `process_group`, `scancel` and `Child::kill` cover every need without our own FFI. libc and rustix are in the tree through signal-hook, mio, parking_lot_core, getrandom and crossterm; they are ledgered (§12) |
| chrono, time (direct) | About 10 LOC of civil-date math. chrono reaches the tree only through arrow |
| log, tracing | `W` lines plus the TUI log ring |
| The `arrow` umbrella crate | Pulls in arith, string and regex |
| parquet (`unjam convert`) | Cut in review round 2: about +3 MB, about 30 crates, about 110 LOC. The pyarrow recipe is documented and tested (Q1, D35) |
| zstd | C library through `zstd-sys`, FFI unsafe, breaks clean musl cross builds |
| mimalloc, itoa, memchr | No measured need |
| procfs, sysinfo | Allocate per read; no fd caching |

- **Measured tree.** A probe of exactly this dependency set (ratatui + clap + arrow-ipc with LZ4 + signal-hook) had 91 unique crates, 74 of them from ratatui, and a **2.76 MB** stripped binary. Gates are re-baselined from measurement at each phase (§13.3): ≤ 3.25 MiB and ≤ 96 crates after record and replay. Measured with our code: 3,466,568 bytes and 90 crates (§13.3).

---

## 11. Type and function inventory

Every item carries a `///` doc stating why it exists, enforced by `tests/budget.rs` (ADR D25). The tables below seed those docs. Struct fields, enum variants and trivial accessors are left out.

### 11.1 Types

| Type | Module | Why it must exist |
|---|---|---|
| `Cli`, `Cmd`, `SlurmArgs`, `RecordArgs` | main | clap derive targets; the only CLI surface. `Cli` holds the viewer flags and the `--slurm`/`--replay` group; `Cmd` is `record` or the hidden `agent` |
| `Metric`, `Kind`, `Ent`, `NA` | model | One descriptor table drives wire `D` lines, rate math and the Arrow schema (R10) |
| `Event<'a>` | model | The single interchange type between sampler, wire, record, replay and state. Borrowing avoids allocation per event |
| `What`, `Rule`, `TaskSet`, `Control` | model | Selective sampling and frequency semantics, shared by agent and TUI so the INT and W columns match agent behaviour |
| `trait Source` | source | The required extension point (R10). Two implementations exist from day one. Stateful (`&mut self`) because likwid and PAPI handles need state |
| `Target<'a>` | source | Tells a source what to sample and hands it the entity's fd slots, without generics |
| `Env` | source | Injected `proc_root`/`cg_root`, page size, CLK_TCK. Makes fixture tests possible |
| `Probe`, `SOURCES` | source | Compile-time registry. Probe errors fail soft as `W` lines. No dynamic loading |
| `Files` | source | Bounded, tiered fd cache, read buffer and self-check result. The single hot-path read policy |
| `Cgroup` | cgroup | cgroup v2 source |
| `Proc`, `Stat<'a>` | procfs | /proc source. `Stat` parses `stat` once for both discovery and values |
| `Sampler`, `CgEnt`, `ProcEnt` | agent | Sampling state machine (rules, timers, discovery), used by local, agent and record modes |
| `Out` | agent | Writer-thread handle, token bucket, pending structure buffer and stagger: the backpressure point |
| `Job` | slurm | squeue/scontrol result (id, nodes, state) needed to build the srun argv |
| `Remote` | slurm | Owns the srun child, its stdin and the step id, for control and staged teardown |
| `Writer` | record | Arrow builders plus per-batch keyframe tracking |
| `Reader`, `Counting` | record | Multi-file replay with seek index and exact byte offsets |
| `State`, `Node`, `Cg`, `Proc` | state | The TUI data store filled by every feed |
| `Columns` | state | Interns `(Ent, name)` → `ColId` from `D` lines. This is how future metrics reach the TUI without code changes |
| `View`, `Layout`, `Row` | state | Rows to render: one of three level orders (step, node, flat), fold, sibling sort, filter, folded node ranges |
| `App`, `Feed` | tui | UI state. `Feed` is `Live { ctl }` or `Replay(Reader)`: two variants, no trait |
| `Msg` | tui | The one channel the main thread blocks on: `Key`, `Resize`, `Lines`, `Stderr`, `Tick` (D38) |
| `Theme`, `Glyphs` | theme | The theme plug-in contract (R19) |

### 11.2 Non-trivial functions

| Function | Why |
|---|---|
| `main() -> ExitCode` | Dispatch; passes the child's exit code through in record mode |
| `parse_dur(&str) -> Result<Duration, String>` | `500ms`, `2s`, `1m` for clap `value_parser`, without humantime |
| `model::effective(&[Rule], task, path) -> (u32, What)` | The one resolver used by agent and TUI |
| `model::pattern_match(pat, path) -> bool` | Segment-wise prefix match with `*` |
| `model::next_due(now_ms, interval_ms) -> u64` | Wall-clock phase alignment, one formula (`(now / i + 1) * i`) |
| `TaskSet::parse / contains`, `Rule::parse` | `*`, `0-3,8`, and the `R` text, also used by `--rule` (prefixed with `R\t`) |
| `source::sources(&Env) -> (Vec<Box<dyn Source>>, Vec<String>)` | Runs the probes; collects failures as warnings |
| `Files::read(slot, hot, dir, name) -> io::Result<&[u8]>` | Cached `pread`, tiered cap policy, `ESRCH` handling |
| `Files::self_check(&Env) -> bool` | Proves `pread`@0 regeneration on procfs and kernfs; otherwise disables caching |
| `source::kv(text, key)`, `kv_eq(text, key)` | `key value` and `key=value` formats (cpu.stat, memory.events, memory.stat, /proc/pid/io; io.stat, PSI) |
| `Cgroup::sample`, `Proc::sample` | Fill the metric slice |
| `cgroup::walk(root, exclude, f)` | Discovery; reports `(path, ino, parent)` |
| `cgroup::procs(buf, out: &mut Vec<u32>)` | Parses and sorts `cgroup.procs` for the merge-diff |
| `cgroup::io_sum(text) -> (u64, u64)` | Sums rbytes and wbytes over devices |
| `cgroup::job_root(self_cg) -> Result<(PathBuf, PathBuf), String>` | Structural Slurm detection: root plus excluded own step (§5.5) |
| `cgroup::record_root(self_cg)` | `…/step_S/user` for record mode |
| `procfs::stat(buf) -> Option<Stat>` | Splits at the last `)`; safe for any comm |
| `procfs::auxv`, `hostname`, `btime`, `nofile`, `cmdline` | Constants and metadata without libc |
| `Sampler::new / tick / control / next_due / snapshot` | Sampling core, including the fork-storm self-rule |
| `agent::run(ctl: Receiver<String>, out: Out, opts)` | Shared loop for remote and local; keepalive |
| `Out::spawn(w: impl Write + Send) / send_tick / set_budget` | Writer thread, try_send, drop and pending policy, stagger |
| `wire::write(buf, node, &Event)` / `wire::parse(line, vals) -> Option<Event>` | Codec with the 1000-byte cap, value splitting and tolerance for extra trailing fields |
| `wire::write_control / parse_control` | Control codec |
| `wire::split_node(&[u8]) -> Option<(u32, &[u8])>` | Demultiplexes by the self-written node prefix |
| `wire::sanitize(&str, &mut String)` | Makes strings tab- and newline-safe |
| `slurm::resolve(Option<&str>) -> Result<String, String>` | Job resolution: value, `$SLURM_JOB_ID`, the only running job of `squeue --me`, else the job list and exit 2 (D43) |
| `slurm::job(id) -> io::Result<Job>`, `slurm::count_nodes(list)` | squeue and scontrol calls |
| `slurm::srun_args(&Job, &SlurmArgs) -> Vec<OsString>` | The one place the srun argv is built, including the forwarded `--keepalive` and `--bcast`/`--unlink-self` (golden-tested, `--print-srun`) |
| `slurm::spawn(.., tx: SyncSender<Msg>) -> io::Result<Remote>` | Starts srun and the two reader threads, which send `Msg::Lines` and `Msg::Stderr` |
| `Remote::send(&Control)`, `Remote::stop(self)` | Control and the staged teardown |
| `record::run_record(args) -> io::Result<ExitCode>` | `exec` for LOCALID≠0; recorder loop and lifetime rule otherwise |
| `record::schema(&[Metric], meta) -> Schema` | Generated schema |
| `record::expand(pattern) -> PathBuf` | `%j %s %n %%` |
| `Writer::push(&Event) / cut() / finish()` | Rows, keyframed batch boundary (10 s or 8,192 rows), end of stream |
| `Reader::open(paths) / seek(ts, f) / step(±1, f)` | Replay |
| `State::apply(node, Event)`, `State::retain_tick(node, ts)` | The single state mutation path |
| `State::derive(node)` | PIDS fallback: subtree sum of `p_threads` where `cg_pids_current` is NA |
| `state::rate(kind, v, pv, dt) -> Option<f64>` | Counter → rate, Gauge → value |
| `state::rows(&State, &View, h, &mut Vec<Row>)` | Visible-window walk over the layout's level order with partial sibling sort, cross-node sums and folded node ranges |
| `tui::input(keys: &Sender<Msg>, wake: &SyncSender<Msg>)` | Input thread: blocking `event::read()`, queues keys and wakes the main thread, sets the quit flag on `q`/Ctrl-C (D38) |
| `tui::run(App, rx: Receiver<Msg>) -> io::Result<()>` | Loop: block on `Msg`, keys first, bounded decode, replay step, draw at ≤ 4 Hz |
| `tui::on_key(&mut App, KeyEvent)` | Bindings → view changes or `Control` |
| `tui::draw(&mut Frame, &App)` | Header with self-monitoring, heat strip, table with history bars (hiding all-NA default columns), footer, overlays |
| `tui::batch(&App, &mut impl Write)` | `--batch` text snapshot (shared `rows`) |
| `tui::fmt(unit, f64, &mut String)` | Human-readable units; unknown units shown raw |
| `tui::default_rules(nodes, interval) -> Vec<Rule>` | §6.3 defaults |
| `theme::pick(Option<&str>) -> usize` | Flag, env and NO_COLOR resolution |

---

## 12. unsafe policy

- **Production code has zero `unsafe`.** Lints: `unsafe_code = "deny"` (not `forbid`, ADR D24), `unsafe_op_in_unsafe_fn = "deny"`, `clippy::undocumented_unsafe_blocks = "deny"`, `clippy::multiple_unsafe_ops_per_block = "deny"`.
- **Any use** needs a local `#[allow(unsafe_code, reason = "…")]`, a `// SAFETY:` comment and an entry in `docs/unsafe.md`. `tests/budget.rs` fails when the number of `allow(unsafe_code)` sites in `src/` differs from the number of `## ` entries in the ledger's "Our code" part.
- **The one planned use** is test-only: `src/main.rs` `#[cfg(test)] mod alloc_count` implements `unsafe impl GlobalAlloc`. It delegates to `System` and counts in a `const`-initialised `thread_local!` `Cell<u64>`, which has no destructor and does not allocate. It lets a test assert zero allocations per steady-state tick. Implementing `GlobalAlloc` is unsafe by definition. Ledger entry #1.

| Where unsafe would usually appear | What we do instead |
|---|---|
| `sysconf(_SC_PAGESIZE/_SC_CLK_TCK)`, `uname`, `getrlimit` | `/proc/self/auxv` (AT_PAGESZ=6, AT_CLKTCK=17), `/proc/sys/kernel/hostname`, `/proc/self/limits` |
| `pread` via libc | `std::os::unix::fs::FileExt::read_at` |
| `CommandExt::pre_exec(setpgid)` | `Command::process_group(0)` (stable since 1.64) |
| `std::env::set_var/remove_var` (unsafe in edition 2024) | `Command::env_remove` on the child only; we never mutate our own environment |
| `execvp` for LOCALID≠0 | `CommandExt::exec` (safe) |
| Signal handlers (`sigaction`) | `signal_hook::flag::register` (safe API; the unsafe lives in signal-hook-registry) |
| `kill(2)` to srun | stdin EOF, then `scancel`, then `Child::kill` |
| `O_NONBLOCK` on stdout via `fcntl` | A writer thread with `try_send` (§7.5) |
| `setrlimit(RLIMIT_NOFILE)` to grow the fd cache | Size the cache from the soft limit, with hot-file tiering (§5.2) |
| pidfd for reuse-safe handles | Cached `/proc/<pid>/stat` fd returns `ESRCH`, plus a `start` comparison |
| `from_utf8_unchecked`, `get_unchecked` in parsers | Byte parsers with bounds checks; lossy conversion once per new entity |
| mmap of record files | Buffered `Read` |
| arrow-ipc `with_skip_validation` (`unsafe fn`) | Never called |
| lz4_flex's unsafe codec | Safe codec forced by feature unification. Throughput cost is **UNVERIFIED**; it is measured in phase 6 and expected to be negligible at ≤ 1 MB/s |
| Future likwid/PAPI FFI | Isolated in `src/<name>.rs`. Every block ledgered. Prefer an existing safe wrapper crate |

**Dependencies with unsafe: generated and checked, not hand-written.**

- `docs/unsafe.md` has a "Dependencies" table with one row per crate: `| crate | what its unsafe does | why accepted |`.
- `scripts/unsafe-deps.sh` (POSIX sh, about 20 lines, run by `mise run unsafe-deps`, part of `mise run check` and the CI `size` job) does the following:
  1. Lists every crate in `cargo tree -e normal --prefix none --format '{p}' --target x86_64-unknown-linux-gnu`, de-duplicated, root excluded.
  2. For each one, greps its source under `$CARGO_HOME/registry/src/*/<name>-<version>/` for `\bunsafe\b`. This is source text, so cfg'd-out code counts too, which is deliberately conservative.
  3. Fails if a crate with hits has no row, or if a row names a crate that is no longer in the tree.
  4. `--print` emits stub rows for missing crates, and the developer fills in the reason.
- The expected set when this was written (the script is authoritative): crossterm, rustix, linux-raw-sys, mio, libc, signal-hook, signal-hook-registry, signal-hook-mio, parking_lot, parking_lot_core, lock_api, compact_str, lru, hashbrown, arrow-buffer, arrow-data, arrow-array, arrow-ipc, flatbuffers, ahash, getrandom, once_cell, zerocopy, half, bytes, lz4_flex (safe mode selected; unsafe paths compiled out but still flagged).
- **libc is in the tree** through signal-hook, signal-hook-registry, mio, parking_lot_core and getrandom 0.3 (ahash `runtime-rng`). The rule is "no libc in our code", not "no libc".

Policy:

- Only widely used crates, pinned in `Cargo.lock`, updated through grouped dependabot PRs.
- `cargo-deny` bans tokio and the named C-building `-sys` crates (§13.4). `linux-raw-sys` is pure-Rust bindings with no C build and is explicitly exempt.
- A new transitive crate with unsafe fails `mise run unsafe-deps` until its row is written. The `lean-deps` skill requires the row's reason, and a reviewer checks it.

---

## 13. Toolchain

### 13.1 MSRV: `rust-version = "1.88"`

- **The floor.** ratatui 0.30.2 declares `rust-version = "1.88"` (0.30.1+), and arrow-array, arrow-schema and arrow-ipc 60.0.0 declare `1.88`. Correction to the base design: lz4_flex 0.14.0 declares **1.81**, not 1.88. Edition 2024 alone needs 1.85, as does clap 4.6.
- **Enough.** 1.88 stabilizes let-chains in edition 2024, which flattens parser code. Nothing newer is needed.
- **Dev pin.** `rust-toolchain.toml` stays at `channel = "1.93.1"` and is the single source for the dev toolchain. Add `components = ["rustfmt", "clippy"]`. mise does not manage Rust.
- **Checks.** Resolver 3 (the edition-2024 default) prefers MSRV-compatible versions. CI runs `cargo +1.88 check --locked --all-targets`.
- **Risk.** arrow ships majors roughly **quarterly** (57 in 2025-10 through 60 in 2026-09; correction to the base design's "monthly"), and a major can raise the MSRV. Policy: MSRV = max(direct deps), bumped only by a deliberate PR.
- **Release binaries** are static `*-unknown-linux-musl` builds (`mise run build:musl`), which rules out C-building `-sys` crates.

### 13.2 Cargo.toml additions, lints, formatting, release profile

```toml
[package]
rust-version = "1.88"
exclude = ["tests/**", ".claude/**", ".github/**", "docs/**", "scripts/**", "CLAUDE.md", "mise.toml", "deny.toml"]

[lints.rust]
unsafe_code = "deny"
unsafe_op_in_unsafe_fn = "deny"

[lints.clippy]
all = { level = "deny", priority = -1 }
pedantic = { level = "warn", priority = -1 }
undocumented_unsafe_blocks = "deny"
multiple_unsafe_ops_per_block = "deny"
allow_attributes_without_reason = "deny"   # every #[allow] states its justification
print_stdout = "deny"                      # agent stdout is the protocol; recorder stdout is the app's
print_stderr = "warn"                      # go through the one log helper
dbg_macro = "deny"
todo = "deny"
unimplemented = "deny"
unwrap_used = "warn"
expect_used = "warn"
mem_forget = "deny"
too_many_lines = "warn"                    # threshold in clippy.toml
missing_errors_doc = "allow"               # binary crate, no public API
missing_panics_doc = "allow"
module_name_repetitions = "allow"

[profile.release]
opt-level = 3          # agent/TUI parse hot paths; probe size at 3 was 2.76 MB, "s" saves too little to matter
lto = "fat"
codegen-units = 1
panic = "abort"        # no unwind tables; ratatui::init's panic hook restores the terminal before abort
strip = "symbols"
debug = false
incremental = false

[profile.dev.package."*"]
opt-level = 1          # arrow record/replay tests are very slow at opt-level 0
```

- `tests/**` is excluded as a whole: the tests read `docs/`, which is not packaged either.
- `clippy::missing_docs_in_private_items` is **not** enabled. Item-level "why" docs are enforced by `tests/budget.rs` instead (ADR D25).
- `clippy.toml`: `allow-unwrap-in-tests = true`, `allow-expect-in-tests = true`, `too-many-lines-threshold = 60`. No `msrv` key, because clippy reads `rust-version`.
- `rustfmt.toml` (stable options only): `style_edition = "2024"`, `max_width = 100`, `use_small_heuristics = "Max"`, `use_field_init_shorthand = true`, `use_try_shorthand = true`, `newline_style = "Unix"`.
- Every new file gets the repo's SPDX header. `REUSE.toml` adds `[[annotations]]` for `tests/fixtures/**` (captured kernel file contents that cannot carry headers) and `.claude/**` (SKILL.md must start with YAML frontmatter), both `LGPL-3.0-or-later` with the GSI copyright, so no new license file is needed.

### 13.3 `mise.toml`

Tasks use inline tables under `[tasks]`. A table header such as `[tasks.fmt]` cannot carry a key on the same line; that form fails TOML parsing. The file as it stands:

```toml
# SPDX-License-Identifier: LGPL-3.0-or-later
# SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany

# Development tools and tasks; CI runs the same tasks. Rust itself comes from rust-toolchain.toml
# through rustup, not from mise. Every tool is pinned.
[tools]
"cargo:cargo-release" = "1.1.6"  # 1.1.7 declares rust_version 1.97 > pinned 1.93.1
"cargo:clippy-sarif" = "0.8.0"
"cargo:sarif-fmt" = "0.8.0"
"cargo:cargo-deny" = "0.20.2"    # declares rust_version 1.88 ≤ pinned 1.93.1
"pipx:reuse" = "6.2.0"
# sind (integration tests) is not a mise tool: the github backend needs the GitHub API to
# resolve releases, and every `mise run` would fail where that is unreachable. Install sind
# v0.15.0 yourself (CONTRIBUTING.md); CI takes it from sind-action.

[env]
# mise [env] overrides the process environment, so a plain value would hide CI's realm. The
# template keeps an inherited SIND_REALM and defaults to unjam-it locally (isolates the test
# cluster from other sind clusters on the same Docker daemon).
SIND_REALM = "{{ get_env(name='SIND_REALM', default='unjam-it') }}"

# Tasks use inline tables: a [tasks.<name>] header cannot carry a key on its own line.
[tasks]
fmt = { run = "cargo fmt --check" }
lint = { run = "cargo clippy --all-targets --locked -- -D warnings" }
"lint:sarif" = { run = "cargo clippy --all-targets --locked --message-format=json -- -D warnings | clippy-sarif | tee clippy-results.sarif | sarif-fmt" }
test = { run = "cargo test --locked" }  # includes tests/budget.rs
build = { run = "cargo build --release --locked" }
# Static release binary for this machine's architecture (publish.yml `binaries` job).
"build:musl" = { run = [
  "rustup target add \"$(uname -m)-unknown-linux-musl\"",
  "cargo build --release --locked --target \"$(uname -m)-unknown-linux-musl\"",
] }
size = { depends = ["build"], run = "scripts/size.sh target/release/unjam" }
"size:self-test" = { depends = ["build"], run = "scripts/size.sh --self-test target/release/unjam" }
unsafe-deps = { run = "scripts/unsafe-deps.sh" }
"unsafe-deps:self-test" = { run = "scripts/unsafe-deps.sh --self-test" }
msrv = { run = [
  "rustup toolchain install 1.88 --profile minimal",
  "cargo +1.88 check --locked --all-targets",
] }
reuse = { run = "reuse lint" }
deny = { run = "cargo deny --locked check" }
bench = { run = "cargo test --release --locked -- --ignored bench_ --nocapture" }  # report only
check = { depends = ["fmt", "lint", "test", "reuse", "deny", "size", "unsafe-deps"] }
# Integration on sind; SLURM, EXTRA and JOBIDPATHS select the cluster (tests/sind/cluster.sh).
"it:cluster" = { run = "mkdir -p target && tests/sind/cluster.sh > target/sind-cluster.yaml" }
"it:up" = { depends = ["it:cluster"], run = "sind create cluster --config target/sind-cluster.yaml --wait 10m" }
"it:test" = { depends = ["build:musl"], run = "tests/sind/run.sh" }
"it:down" = { run = "sind delete cluster unjam" }
it = { run = "tests/sind/it.sh" }  # up, test, and down even on failure
release = { run = "cargo release" }
```

- **sind is not a mise tool.** The `github:GSI-HPC/sind` backend resolves releases through the GitHub API (or mise's version mirror). Where neither is reachable, every `mise run` fails while it tries to install the tool, not only the integration tasks. Developers install sind v0.15.0 themselves (CONTRIBUTING.md); CI gets it from sind-action (§18).
- **uv** (`uv = "0.9.5"`, pinned, never "latest") is added together with IT 6, the only user of the pyarrow interop check.
- **`build:musl`** builds the static binary for the host architecture. The release job runs it on an x86_64 and an aarch64 runner, and the integration tests copy nothing: the nodes run it from `/data`.
- **Gate self-tests** are separate tasks (`size:self-test`, `unsafe-deps:self-test`) that the CI `size` job runs.

**`scripts/size.sh`** (POSIX sh, about 15 lines). It reads its limits from `docs/budget.txt` (`binary_bytes`, `crates`).

- Checks `stat -c %s` of the binary against `binary_bytes`.
- Counts crates with `cargo tree -e normal --prefix none --format '{p}' --target x86_64-unknown-linux-gnu | sed 's/ (\*)$//; s/ (proc-macro)$//' | grep -v '^unjam v' | sort -u | wc -l`, which excludes the root crate (correction to the base design's count), and checks it against `crates`.
- Prints `cargo tree -d -e normal` for information.
- Its `--self-test` flag runs each check against a limit of 0, which must fail, before the real check, so the script itself is tested in CI.

**`scripts/unsafe-deps.sh`**: see §12. It has a `--self-test` that removes one row from a temporary copy of the ledger and expects failure.

**Phase limits** (re-baselined from measurement, then +5 %; `docs/budget.txt` starts at the phase-3 limits, measured with clap alone at 556,672 bytes and 10 crates):

| After | Binary | Crates |
|---|---|---|
| Phase 3 | ≤ 2 MiB | ≤ 80 |
| Phase 6 | ≤ 3.25 MiB | ≤ 96 |

Measured after record and replay: 3,466,568 bytes (3.31 MiB, 2 % over the target) and 90 crates. The gate in `docs/budget.txt` is that measurement + 5 % (3,639,896 bytes, 95 crates).

### 13.4 `deny.toml`

- `[graph] targets = ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu", "x86_64-unknown-linux-musl", "aarch64-unknown-linux-musl"]`. Without this, cargo-deny checks every target. arrow-array enables ahash `compile-time-rng` for wasm32, which pulls const-random-macro → tiny-keccak (CC0-1.0, not on the allow-list), and `mise run deny` would fail.
- `[licenses] allow = ["MIT", "Apache-2.0", "Apache-2.0 WITH LLVM-exception", "BSD-2-Clause", "BSD-3-Clause", "ISC", "Zlib", "Unicode-3.0", "LGPL-3.0-or-later"]`
- `[bans] multiple-versions = "warn"`, `deny = [{ name = "tokio" }, { name = "openssl-sys" }, { name = "zstd-sys" }, { name = "libz-sys" }, { name = "serde_json" }]`. The rule is "no C-building `-sys` crates (a `cc` or `pkg-config` build)". It is enforced for the known offenders by name and for the rest by the `lean-deps` review. `linux-raw-sys` (pure Rust, via rustix) is explicitly exempt.
- `[advisories]` defaults. `[sources] unknown-registry = "deny"`, `unknown-git = "deny"`.

---

## 14. Testing

### 14.1 Unit tests (in-module `#[cfg(test)]`, about 900 LOC)

**Fixture trees** in `tests/fixtures/`, captured from sind workers by `tests/fixtures/capture.sh` (tars selected files via `sind ssh worker-0 --`). Every reader takes `proc_root` and `cg_root` through `Env`. Contents:

- `proc/<pid>/{stat,io,cmdline}`, including comm values `a b`, `x) (y` and a zombie.
- `cgroup/25.11`: `system.slice/slurmstepd.scope/job_42/{step_extern,step_batch,step_0/{slurm,user/task_0,user/task_1}}`, captured with `EnableExtraControllers=io,pids`.
- `cgroup/26.05-sluid`: the same tree with a SLUID job directory.
- `cgroup/no-controllers`: a job tree as a default Slurm leaves it: no `io.stat`, no `pids.current`, no `memory.peak`, no `*.pressure`.

| Area | What is covered |
|---|---|
| Parsers | `procfs::stat`, `kv`, `kv_eq` (PSI), `io_sum` on multi-device `io.stat`, `memory.stat` keys, `auxv`, `nofile`, NA on missing files |
| Slurm layout | `job_root` and `record_root` on 25.11, 26.05-SLUID, `node1_slurmstepd.scope` and a `0::/` namespace root (must fail); rejection of non-step paths and of v1 format |
| Rules | `pattern_match`, `effective` and `TaskSet` tables. `Rule::parse` rejects `- p`, `500 o` and the removed level `d`. `next_due` table including `now` on an exact multiple, which must return the *next* multiple. `--rule` text is parsed with an `R\t` prefix |
| Sampler | Timer grouping. Pid reuse: rewriting `start` gives `x` then `P`. Cgroup recreation: new inode gives `X` then `G`. Structure-only `Off`. `Files` tiers: past `cap − cap/8`, cold files take the uncached path while hot files are still cached, and at `cap` both are uncached. Self-check failure gives cap 0 (forced via `Env`). The fork-storm fallback applies `R * . <longest> c`, which `Rule::parse` accepts |
| Zero allocation | `alloc_count`: after a warm-up tick, 100 samples (`Sampler::sample`, a tick without its discovery walk) over the 25.11 fixture allocate 0 times in the sampler and encoder. The walk, which runs on nearly every due tick, allocates for std `read_dir` and its entry names only; the same test bounds it at 2 per directory entry |
| Writer path | A blocked writer drops values and keeps structure; the budget limits bytes per second; the stagger delay formula |
| Wire | Round trip for every variant; the 1000-byte cap; value splitting with `first`; sanitizing; extra trailing fields on `H`/`D`/`T` ignored |
| Mux (graft) | 1000 simulated agents over fixture data. Output is cut into ≤ 1024-byte whole-line chunks (slurmstepd semantics), interleaved randomly into one reader, and fed through `split_node`, `parse` and `State`. Each node's `State` must equal the reference exactly. Also: one injected corrupt line triggers exactly one `S` |
| `srun_args` | Golden argv for: default; `--nodelist`; `--srun-arg`; `--agent-keepalive 5` (forwards `agent --keepalive 5`) |
| Record | Round trip to a temporary file: generated schema and field metadata; keyframe metadata on the first row per batch; a batch is cut at 8,192 rows; seek to the middle equals linear replay; a journal truncated at 200 random offsets always yields its valid prefix, never a panic |
| State and TUI | `state::rows` for each of the 3 layouts (golden rows; fold, sibling-only sort with NA last, cross-node sums, folded node ranges). CPU history ring wrap-around. Heat strip cell grouping. `State::derive` over the `no-controllers` fixture gives `~` PIDS equal to summed threads. All-NA default columns are hidden. `tui::draw` against `TestBackend` with theme `mono`, compared to stored buffers. `NO_COLOR` gives `mono`. `--batch` text against a golden file |
| Benchmarks | `bench_*` (`#[ignore]`, `mise run bench`): ns per cgroup and process sample, ns per file for `cpu.stat`/`io.stat`/`memory.stat` (fixture parse only; kernel cost is measured in sind), bytes per tick, TUI apply ns per value, on fixtures replicated to 100, 1k and 10k processes. Report only |

**`tests/budget.rs`** (integration test, std only, does not import the crate; modules listed in `docs/budget.txt` but not yet written are skipped):

1. LOC per module and total against `docs/budget.txt` (+10 %).
2. Every `fn`, `struct`, `enum`, `trait`, `type`, `const`, `static` and `mod` item in `src/` is preceded by a `///` doc line (attributes may sit in between). The `#[cfg(test)]` module, by convention the last item of a file, is exempt.
3. Every `[dependencies]` key appears in `docs/deps.md`.
4. The count of `allow(unsafe_code)` sites equals the number of entries in the "Our code" part of `docs/unsafe.md`. Dependency rows are checked by `scripts/unsafe-deps.sh`.

Its own line counter is unit-tested inside the file.

### 14.2 Integration on sind (`tests/sind/`)

**`tests/sind/cluster.sh`** prints the cluster YAML (schema from sind `configuration/cluster-config.md`; `slurm.main` goes into `slurm.conf`, `slurm.cgroup` into `cgroup.conf`):

```yaml
kind: Cluster
name: unjam
defaults:
  image: ghcr.io/gsi-hpc/sind-node:v0.15.0-${SLURM}   # pinned sind-release image of that Slurm line
  cpus: 2
  memory: 1g
users:
  - alice
  - bob
nodes:
  - role: controller
  - role: submitter
  - role: worker
    count: 3
slurm:
  main: |
    WaitTime=10
  cgroup: |
    ConstrainRAMSpace=yes
    ${EXTRA:+EnableExtraControllers=io,pids}
    ${JOBIDPATHS:+CgroupJobIdPaths=yes}
```

- `EXTRA` adds `EnableExtraControllers=io,pids`, which makes the IO and PIDS metrics testable in job cgroups (§4.3, D42). Without it, the same tests check the hidden IO columns and the `~` PIDS fallback.
- `JOBIDPATHS` is accepted only with `SLURM=26.05`; `cluster.sh` rejects other combinations.
- `WaitTime=10` lets IT 4 check that the agents survive another agent's exit past the site wait time.
- Resources: sind sets `DefMemPerCPU = memory / cpus` (512 MB) with `CR_Core_Memory` (sind `architecture/slurm-config.md`). Each worker therefore fits exactly two 1-CPU jobs. The ITs size their jobs so that decoys share nodes with the target (IT 2).
- `/home` is shared on every node (sind `usage/node-access.md`). The workspace is mounted at `/data` on every node. The nodes run the static binary `/data/target/<arch>-unknown-linux-musl/release/unjam` (`mise run build:musl`), because the Rocky Linux node images may have an older glibc than the host that builds it.
- Commands run as alice through `sind exec unjam --user alice --` (on the submitter) or `sind ssh alice@worker-N.unjam --`.
- Record output goes to `/home/alice`.

**`tests/sind/run.sh`** (`bash`, `set -euo pipefail`). It starts with the smoke tests of the scaffold: `sinfo`; `srun -N3 hostname` as alice gives three hosts; the job cgroup's `cgroup.subtree_control`, found by stripping `/step_*` from the task's `/proc/self/cgroup`, lists `io` and `pids` where `EXTRA` is set; and the static binary runs on a worker. Assertions of the tests below use `--batch` output. A helper `wait_running JOBID… [timeout 60s]` polls `squeue -h -j … -o %T` and fails unless every job is `RUNNING`.

| # | Test | Assertion |
|---|---|---|
| 1 | Local: `sind ssh alice@worker-0 -- /data/…/unjam --batch 2` | Tree contains `system.slice` and processes. No `W` about pread caching (the self-check passed on real procfs and kernfs). The fd-cache hit ratio from the footer is recorded into `docs/perf.md` |
| 2 | Attach isolation. Target as alice: `sbatch -N3 --ntasks-per-node=1 --cpus-per-task=1 --mem=256M --wrap 'srun sleep 600'`. Decoys on the **same** nodes: alice `sbatch -N1 -w worker-0 -n1 --mem=256M --wrap 'srun sleep 601'`, bob `sbatch -N1 -w worker-1 -n1 --mem=256M --wrap 'srun sleep 602'`. `wait_running $J $A $B`, then `unjam --slurm=$J --batch 3` | 3 nodes. A `step_0/user/task_<N>` cgroup with the `sleep 600` process on each node (`task_<N>` is the global task id). No path, pid or `sleep 601`/`sleep 602` command line from either decoy, although they run on worker-0 and worker-1. Own step absent. The job directory is `job_$J` on 25.11 and 26.05+JobIdPaths, and **not** `job_*` on 26.05 default, which proves the SLUID path. With `EXTRA`, IO-R/s and PIDS on `step_0` are not `-`; without it, IO-R/s on `step_0` is `-` (the column still shows the processes' `/proc/<pid>/io`) and PIDS carries `~`. `ConstrainRAMSpace=yes` exercises memory limits on the overlap step |
| 3 | Selective: `unjam --slurm=$J --batch 3 --rule $'0\tstep_0\t500\tp' --rule $'1-2\t.\t2000\tc'` (R-line text without the leading `R\t`, §3.1) | Node 0 shows process rows at INT 500 ms. Nodes 1 and 2 show cgroups only |
| 4 | Teardown and survival | (a) After `--batch` exits, `squeue -s -j $J -h` shows no `unjam` step within 5 s and the job is still `RUNNING`. (b) `unjam --slurm=$J --interval 1s --batch 25`. After 3 s, `sind ssh worker-2 -- pkill -u alice -f 'unjam agent'`. The last snapshot (≥ 15 s after the kill, past `WaitTime=10`) shows node 2 stale and nodes 0 and 1 with fresh `T`. This checks `--kill-on-bad-exit=0` and `--wait=0` on a `--jobid` step (§3.2). (c) `unjam --slurm=$J --agent-keepalive 5` is started and SIGKILLed: the step is gone within 15 s |
| 5 | Framing stress: 3 nodes × 500 sleeper processes, `--rule $'*\t.\t100\tp' --budget 64M --batch 30` (30 s; one snapshot per second) | Footer `errors=0`. Bytes per second and fd-cache hit ratio recorded into `docs/perf.md`. Agent RSS < 16 MiB (`T` and `ps`) |
| 6 | Record: `sbatch -N2 --ntasks-per-node=1 --mem=256M --wrap 'srun /data/…/unjam record -i 200ms -o /home/alice/rec-%j-%n.arrows -- sh -c "dd if=/dev/zero of=/dev/null bs=1M count=20000; exit 3"'` | 2 files. Job exit code 3. `uv run --with pyarrow==21.0.0 tests/sind/check_record.py` (pinned version **UNVERIFIED**): columns and units metadata present; rows > 0; `comm == "dd"` rows exist; `cg_cpu_usage_usec` non-decreasing per `cg`; with `EXTRA`, `cg_io_read_bytes` and `cg_pids_current` non-null on cgroup rows (null without it); the §8.1 Parquet recipe runs and `pq.read_table` gives the IPC row count. `unjam --replay … --batch 1 --at end` lists `dd` |
| 7 | Record kill, twice, 13 s into `record -- sleep 300`: (TERM) `scancel --signal=TERM $J`; (KILL) `pkill -KILL` of the recorder on its node | TERM: `check_record.py` reads a complete journal, replay warns nothing. KILL: `check_record.py --truncated` reads ≥ 1 complete batch, and `unjam --replay … --batch 1 --at end` succeeds and warns "truncated at byte N" once |
| 8 | Broadcast: copy the binary to `/var/tmp/unjam-local` on the submitter only (node-local, exec allowed; sind mounts `/tmp` `noexec`), then `/var/tmp/unjam-local --slurm=$J --bcast=/var/tmp --batch 2` as alice | 3 nodes report. After teardown, no `/var/tmp/unjam.$J.*` file is left on any worker (`--unlink-self`) |

1k-node fan-out is covered synthetically by the mux unit test. The real srun throughput at many tasks is measured once on a production cluster in phase 5 (Q10).

---

## 15. Risks and open questions

### Risks

| # | Risk | Mitigation |
|---|---|---|
| K1 | srun keeping each message whole at high task counts is **UNVERIFIED** (the slurmstepd side is verified in 25.11 source) | 1000-byte cap, node prefix, `S` resync, IT 5 on 25.11 and 26.05, mux test |
| K2 | srun I/O throughput and CPU at 1k tasks on a shared login node is **UNVERIFIED** | Byte budget (4 MiB/s total), large-job defaults (about 0.5 MB/s), stagger. Measure before claiming 1k-node numbers |
| K3 | Site policy: job_submit or cli_filter on `--overlap` steps, `MaxStepCount`, SPANK, overlap steps under `ConstrainRAMSpace` | `--srun-arg` escape hatch. IT 2 runs with `ConstrainRAMSpace=yes` |
| K4 | `pread`@0 regeneration on procfs or kernfs | Startup self-check with automatic fallback (§5.2) |
| K5 | memcg-charged fd cache memory | Cap `min(soft_nofile − 64, 1024)` fds (≈ 5 MiB) in job context; prompt close on removal; hot tier keeps the most-read files cached when the cap binds |
| K6 | Kernel and site features: `memory.peak` needs ≥ 5.19 (RHEL 9 backport **UNVERIFIED**); PSI may be disabled (`psi=0`); `io.stat` and `pids.current` need the io and pids controllers, which Slurm job cgroups lack unless `EnableExtraControllers=io,pids` | NA values; all-NA columns hidden; PIDS fallback to summed threads; README "Site notes"; no site requirement (D42) |
| K7 | Clock skew between nodes affects aligned ticks | NTP assumed; header shows the maximum `T` skew |
| K8 | Replay memory at 1k nodes: one batch per file | Typically ≈ 250 MB, at most ≈ 3 GB at the 8,192-row cap. `--nodes` filter for replay is deferred (Q8) |
| K9 | arrow MSRV or API churn (quarterly majors) | Grouped dependabot PRs plus the `msrv` job |
| K10 | Utime/stime tick resolution makes process CPU% noisy at 100 ms | Documented. Cgroup `cpu.stat` (µs) is precise |
| K11 | Cgroup namespace rooted below the job on some sites | Clear error message (§5.5); Q9 |
| K12 | `--wait=0` on a `--jobid` step: srun(1) says `-W` "applies to job allocations", so its effect on steps is **UNVERIFIED** | IT 4 kills one agent with `WaitTime=10` and checks that the others survive. If the flag does nothing on steps, it is dropped (§3.2) |
| K13 | rstat flush cost of `cpu.stat`, `io.stat` and `memory.stat` per cgroup per tick | Measured per file in phase 3 (§5.4). Rule intervals are the knob; a cost class is reintroduced only on measured need (D12) |

### Questions for the owner

Answered (decisions in §16):

- **Q1** (Parquet writer in the binary): no. D35 stands.
- **Q2** (built-in `--bcast` and static musl release binaries): yes. D36 revised.
- **Q3** (record what the TUI receives): yes, with Q13. D41.
- **Q10** (real-cluster measurement): at the largest available scale; 1k-node numbers are marked as extrapolated.
- **Q12** (`EnableExtraControllers=io,pids` at sites): no site requirement. D42.
- **Q13** (mock features in v1): all of them. D41.
- **Q14** (CLI shape): one viewer command with source flags. D43.

Open:

- **Q4.** Should the recorder `fsync` each batch (node-crash durability against shared-filesystem cost)? The default is no.
- **Q5.** Are compiled-in themes enough (no theme files, no parser dependency)?
- **Q6.** Is 16 nodes the right threshold for turning processes off by default? Is attaching as operator or root wanted? srun's permissions decide either way.
- **Q7.** Heterogeneous jobs and multiple clusters are out of scope for v1. Is that acceptable?
- **Q8.** There are no host-level metrics (`/proc/stat`, `meminfo`) in round one, because the cgroup root covers totals. Is that acceptable?
- **Q9.** Do any target sites run Slurm steps in a cgroup namespace (`namespace/linux` or a container plugin)? If so, a resolution via `scontrol listpids` would be needed.
- **Q11.** Default record interval of 5 s: acceptable? And if measurements show the agent above 1 % of one core under default rules, should an automatic interval stretch be added (about 30 LOC)?

---

## 16. Decision log (ADR style)

| # | Decision | Alternatives | Rationale |
|---|---|---|---|
| D1 | Use the "minimal" design as the base | scale, extensible | Highest total score. Fewest moving parts. Its job filter is the only one that works with 26.05 SLUIDs |
| D2 | One package, one binary, no lib, no workspace, no own cargo features | lib+bin; `record` feature | One build configuration is one test matrix. A lib target would exist only for tests |
| D3 | std threads plus mpsc; no async | tokio | A few fds per process. tokio adds about 20 crates and buys nothing here |
| D4 | Job root by structural strip of `step_*/user/task_*`; never match `job_<id>`; local mode uses `--root` only | `job_<id>` search; `--job J` glob | 26.05 defaults to SLUID directory names. A `--job` lookup would need scontrol plus pid mapping for a rare use |
| D5 | Text TSV wire with self-describing `D` lines and value offsets | Positional fixed columns; binary varint with escapes; base64-armored binary | Delivers R10 with no codec dependency. Readable by eye. Version-tolerant. About 120 LOC less than a binary codec |
| D6 | Agent writes its own node prefix; no `--label` | srun `--label` | Independent of srun's label format. Same code for local mode and future transports. About 2–5 bytes per line |
| D7 | Line cap of 1000 bytes | 1023 | 24 bytes of margin under `SLURM_IO_MAX_MSG_LEN`; no off-by-one exposure |
| D8 | Absolute values each tick; no delta encoding | `cur − sent` varint deltas | Dropped ticks coalesce losslessly for free. Simpler receiver. Bandwidth is bounded by the budget, not by the encoding |
| D9 | Sampler never blocks: writer thread, bounded `try_send`, keep structure and drop values, per-agent token bucket | Blocking stdout; `O_NONBLOCK` via rustix | Meets the perf must-fix with std only and bounded memory |
| D10 | Phase-aligned sampling with staggered emission | Unaligned ticks; aligned ticks without stagger | Coherent cross-node sums without 1k simultaneous bursts |
| D11 | Bounded fd cache via `FileExt::read_at`: `min(soft_nofile − 64, 1024)` in job context, 2048 locally; hot tier (`cpu.stat`, `memory.current`, `cgroup.procs`, `/proc/<pid>/stat`) gets the top eighth reserved; no eviction; startup self-check | No cache; 512 FCFS cap; LRU; RLIMIT raise | A cached read is 1 syscall instead of 3. The typical node (≈ 970 files) gets ≈ 90 % cached and every hot file cached, for ≈ 5 MiB of memcg charge. No libc or rustix. Exact `ESRCH` death detection. Revised in round 2: the old 512 cap could not hold the stated typical case |
| D12 | No per-metric cost class; `memory.stat` read at level `c` | `Cost::Costly` gated at level `d` with Detail-popup escalation | Round-2 review: `cpu.stat` and `io.stat` already trigger rstat flushes every tick (v6.12 `rstat.c`, `blk-cgroup.c`), so gating one file saved little and cost an enum, a level, a wire field and popup logic. Phase 3 measures per-file cost; a cost field can be appended to `D` without a proto bump if a measured source needs it |
| D13 | Cgroup PSI (`some total=`) in round one | Defer (base Q4) | Pressure is the core "jam" signal. 3 cheap files per cgroup and about 10 LOC |
| D14 | Self-CPU measured and reported (`T`, header); no automatic throttle in v1; no `nice` claims | `nice 10`; interval stretch with hysteresis | nice has no effect across cgroups. Rule intervals, the byte budget and large-job defaults already bound the work. The stretch overlapped with them and is added back only if phase-5 measurements show a need (Q11) |
| D15 | Sampler always on its own thread; one text ingest path | Local sampling on the UI thread | Bounds key latency on large hosts. Local mode exercises the remote code path |
| D16 | Record writes an IPC journal (10 s or 8,192-row keyframed batches); replay reads `.arrows` only | Parquet only (segmented); 65,536-row batches; in-job conversion | Crash-safe with about 2–3 MB of in-job memory (65,536 rows would be about 16 MB). One replay path. Never extends the step |
| D17 | Wide schema generated from descriptors; names append-only | Fixed wide columns; narrow EAV | No code change per source, analyst-friendly, good compression. EAV bloats IPC (Utf8 name per value) |
| D18 | Record lifetime: child exited, siblings `populated 0`, own task cgroup holds only self | Scan for leftover processes; child exit only | Cheap and exact; catches daemonized descendants |
| D19 | No signal forwarding in `record`; finish at most 5 s after a signal | Forward TERM, USR1 and others | Where Slurm signals the whole step, forwarding would deliver signals twice; where it signals only the task leader, the 5 s grace still finishes the journal before `KillWait` |
| D20 | Teardown: EOF, then `scancel` step, then SIGKILL; agent keepalive | SIGTERM via rustix | No libc. Orphan-proof even if srun dies uncleanly |
| D21 | Last matching rule wins; `Off` takes interval `-` | Most-specific match; interval 0 = off | Predictable and cheap. Removes the clamp contradiction |
| D22 | No metric masks (`cols`) and no column chooser in v1 | extensible's masks and chooser | Per-subtree rule intervals bound cost; the Detail view shows everything; all-NA columns hide themselves; YAGNI |
| D23 | MSRV 1.88 | 1.85; 1.93 | Floor set by ratatui 0.30.2 and arrow 60. Let-chains |
| D24 | `unsafe_code = "deny"`, not `forbid` | `forbid` | `forbid` applies to every target and would block the test-only counting allocator |
| D25 | "Why" docs enforced by `tests/budget.rs` on items only; `missing_docs_in_private_items` off | Clippy lint on every field and variant | The clippy lint forces boilerplate on every field; the rule is about items |
| D26 | Budget gates as a Rust test plus self-tested `scripts/size.sh` and `scripts/unsafe-deps.sh`; crate count excludes the root | Inline mise shell blob; hand-written dependency ledger | Tested and deterministic. Fixes the counting error. The round-2 review found the hand-written ledger incomplete (libc, getrandom, lru, …) |
| D27 | Accept crossterm default features and ratatui-crossterm's `underline-color` default, counted in budgets | `[patch]` crossterm | A patch adds maintenance for a small gain |
| D28 | `lz4_flex` forced to safe mode; LZ4 rather than zstd | zstd (C); unsafe lz4 | Pure Rust, clean musl builds, no FFI |
| D29 | cargo-deny with `[graph] targets` (4 Linux targets), bans on C-building `-sys` crates (linux-raw-sys exempt) and tokio | None (base Q8); all-target graph | Cheap guard rails for licenses, advisories and minimality. The target list keeps wasm-only crates (tiny-keccak, CC0) out of the license check |
| D30 | Compiled-in `const` themes | Theme files | No parser dependency (Q5) |
| D31 | `--batch N` and `--rule` as the scripting and IT hooks | Capture-and-assert of raw protocol | About 50 LOC reused from `rows`; tests exercise the real TUI logic |
| D32 | Every tool pinned; cargo-release 1.1.6; sind-action `version: v0.15.0`, `wait: 10m` | `latest`; 1.1.7; action defaults (`latest`, 5m) | Reproducible. 1.1.7 needs rustc 1.97 |
| D33 | CI integration matrix: 25.11, 26.05, 26.05 with `CgroupJobIdPaths=yes`, on pinned `v0.15.0-*` images | 25.11 and 26.05 only | Tests both job-directory naming schemes on 26.05 (must-fix) |
| D34 | Reject a TUI-side ingest guard | extensible's guard | The per-agent budget is authoritative; the header shows the ingest rate |
| D35 | No in-tree Parquet writer; documented pyarrow recipe, tested in IT 6 | `unjam convert` with `parquet` + `arrow` feature | Saves about 3 MB, about 30 crates and about 110 LOC. The brief's "Arrow/Parquet" is met by the Arrow IPC journal. Q1 can bring it back |
| D36 | **Revised by the owner (Q2):** built-in `--bcast` (srun `--bcast=/tmp/unjam.<J>.<uid>`, agent `--unlink-self` removes the copy at start; about 30 LOC) and static musl release binaries: publish.yml job `binaries` on x86_64 (`ubuntu-latest`) and aarch64 (`ubuntu-24.04-arm`), target `*-unknown-linux-musl`, `mise run build:musl`, `actions/attest-build-provenance`, assets `unjam-linux-{amd64,arm64}` plus `checksums.txt`. New IT 8 | Originally deferred, with `--srun-arg=--bcast=…` as the escape hatch | Sites without a shared filesystem get a working default, and the copy is never left behind. Static binaries run on any compute node's libc |
| D37 | Single CI realm `unjam-it`, passed to the action's `realm` input; mise `[env]` uses a template default | Per-matrix `unjam-<n>` realms | Each matrix job has its own runner and Docker daemon, so realms isolate nothing there (sind `guides/ci-cd.md`). mise `[env]` would override a per-job value anyway |
| D38 | Dedicated input thread blocks in `crossterm::event::read()` and sends `Msg::Key`/`Msg::Resize` into the same channel as the srun readers. The main thread blocks on that one channel (`Msg::{Key, Resize, Lines, Stderr, Tick}`), handles keys first, decodes lines for at most 10 ms per batch, then checks for keys again (§3.1, §7.3) | Keys polled in the main loop with `event::poll(timeout)` | A draw can block on a slow terminal (ssh); keys still arrive and `q`/Ctrl-C set a quit flag at once. One blocking `recv` is simpler than poll plus timeouts. Cost: 1 thread, about 10 LOC |
| D39 | Exactly 3 layouts as level orders in `View`, walked by one `state::rows`: step (Slurm default: job → step → node → task → process), node (node → cgroup tree → processes), flat (NODE column, global sort; classic `top` locally). `v` cycles. Fold state keyed by `(level, key)`. The sort column orders siblings only. No "by command" layout and no RANK column in round one (§9.2) | Fixed by-node / by-path pivot | Each layout answers a different question. As data, cost ≈ +50 LOC in state.rs and 3 golden tests. Flat sort at 100k processes ≈ 1 ms per frame (select_nth). Replaces the generic by-path view |
| D40 | Self-monitoring header: TUI CPU and RSS, bytes and messages per second in, control bytes out, the busiest agent's CPU and RSS with its node, drops and errors. `T` gains a 5th field `self_rss_bytes`; no proto bump (§9.1) | Self-CPU of the agents only | The tool must show its own cost on 1k-node jobs. About 30 LOC |
| D41 | All mock features in v1 (Q13): node heat strip, CPU history bar per row (16 × u16 ring), `r` key and `--record DIR` (TUI writes received events via `record::Writer`, one `.arrows` per node, `unjam.source=tui`), folded node ranges in the step layout (§8.5, §9) | Defer them | Owner decision. About 150 LOC in total; the LOC budget was raised to ≈ 2,700 (cap 2,900) |
| D42 | No site requirement for `EnableExtraControllers` (Q12). IO and PIDS columns hide when the controllers are absent; PIDS falls back to summed threads marked `~`. CI matrix: 25.11 without `EXTRA`, 26.05 with `EXTRA`, 26.05 with `EXTRA` and `JOBIDPATHS`; ITs assert io and pids values only where `EXTRA` is set (§4.3, §14.2, §18) | Ask sites to set it | Sites cannot always change `cgroup.conf`; the tool degrades instead of failing. The matrix tests both cases |
| D43 | One viewer command with source flags: `unjam`, `unjam --slurm[=JOBID]` (`num_args(0..=1)`, `require_equals`), `unjam --replay FILE.arrows…`; `--slurm`/`--replay` mutually exclusive, Slurm-only flags `requires = "slurm"`; `record` and hidden `agent` stay subcommands. Job resolution without a value: `$SLURM_JOB_ID`, else the only running job from `squeue --me`, else print the jobs and exit 2 (§3.1) | `unjam attach JOBID` and `unjam replay` subcommands | Subcommands would repeat the viewer flags per source and leave no room for a job picker. Cost: 0 LOC |

**Judge must-fix resolution:**

| Must-fix | Where resolved |
|---|---|
| Structural job root; tests on both naming schemes | D4, §5.5, fixtures, D33 |
| cargo-release pin | 1.1.6 (§13.3) |
| MSRV justification | §13.1 |
| sind `--wait` | `--wait 10m` locally (§13.3), `wait: 10m` on sind-action in CI (§18) |
| Interval 0 vs clamp | D21 |
| R10 on the wire and in records | D5, D17 |
| Unpinned tools | All pinned, including uv and the sind-action `version` |
| Fragile budget shell | D26 |
| `missing_docs_in_private_items` | D25 |
| UI stalls in local mode | D15 |
| Arrow vs Parquet (Q1) | D16, D35 plus Q1 |
| `forbid` vs test unsafe | D24 |
| fd cache memcg bound | D11, §5.2 |
| nice claim | D14 |
| `memory.stat` per tick | D12 (revised in round 2: measured, not gated) |
| 24-byte margin | D7 |
| Never block the sampler | D9 |
| Stagger keyframes | D10 |
| TUI visible-only rendering and bounded decode | §7.3, §9.4 |
| Bounded recorder memory | §8.3 (8,192-row cap), D16 |
| pread validation | §5.2 |
| No double signals | D19 |
| Interop claims tested | §8.6, IT 6 |
| `--label` atomicity | D6, IT 5 |
| crossterm defaults counted | D27 |
| Precise cgroup v2 rule | §4.1 |
| arrow release cadence | §13.1 |
| Cgroup namespace claim | §5.5 |

**Review round 2 resolution:**

| Finding | Where resolved |
|---|---|
| sind-action `clusters` must be a YAML list; `version`/`wait`/`realm` unwired | §18 (file path as list item, `version: v0.15.0`, `wait: 10m`, `realm`), D32 |
| mise `[env]` overrides CI's `SIND_REALM`; per-matrix realms pointless | §13.3 template default, §18 single realm, D37 |
| io/pids controllers absent in Slurm job cgroups | §4.3, §5.5, §9.1–9.2, §14.2 cluster config + IT 2/6, K6, Q12 |
| fd cap 512 below the typical 900+ files | §5.2, §5.4, D11, hot-path skill |
| Cheap/Costly premise false (rstat flush on `cpu.stat`/`io.stat`) | §4.3, §5.3, §5.4, D12, K13 |
| Recorder memory arithmetic | §8.3 (8,192 rows ≈ 2 MB), D16, phase 6 |
| IT 2 decoys pending, so isolation untested | §14.2 job sizing plus `wait_running` |
| Invalid TOML in mise tasks | §13.3 inline tables |
| Incomplete unsafe dependency ledger | §12 `scripts/unsafe-deps.sh`, D26 |
| deny.toml graph targets; `-sys` wording | §13.4, D29, lean-deps skill |
| Fork-storm rule contradicts grammar | §7.5 |
| `ceil` phase alignment double-samples | §6.1 |
| `--rule` text and `--agent-keepalive` inconsistencies | §3.1, §3.2, §6.2, §14.1 golden tests |
| `--wait=0` semantics for steps | §3.2, IT 4, K12 |
| ratatui `underline-color` comment | §10.3, D27 |
| signal-hook 0.4 via dependabot | §10.3, §18 dependabot ignore and group |
| Skill rules contradicting accepted code | §17.1 (`Box<dyn Error>` in main), §17.2 (bans are direct-only) |
| Resolvable UNVERIFIED markers | `--bcast` default (§3.2), cargo-deny rust_version (§13.3), `MISE_DISABLE_TOOLS` (§18) |
| Bloat: parquet + `convert` | Cut (D35, Q1) |
| Bloat: Cost enum, level `d`, Detail escalation | Cut (D12) |
| Bloat: self-CPU stretch | Cut (D14, Q11). Kept: token bucket, stagger and drop/pending, which bound different things (bytes, burst shape, memory) |
| Bloat: `--bcast` and musl binaries | Deferred (D36, Q2); later reinstated by the owner (D36 revised) |
| Bloat: per-matrix realm | Cut (D37) |
| Bloat: uv + pyarrow + duckdb | DuckDB cut. uv + pyarrow kept: they test the documented recipe that replaces `convert` and add nothing to the binary (§8.6) |

---

## 17. Agent skills for hygiene

The rules of §10 to §13 are codified as Claude Code skills. The files are authoritative; this section only says what each covers.

| File | Loaded when | Enforces |
|---|---|---|
| `.claude/skills/lean-code/SKILL.md` | Any item under `src/` changes | A `///` "why" doc per item, module line budgets, no speculative generality (traits, generics, builders), functions ≤ 60 lines, the file allowlist for new metric sources, and the fixed program shape: one viewer CLI with source flags (D43), three layouts with sibling-only sort (D39), missing controllers as NA (D42) |
| `.claude/skills/lean-deps/SKILL.md` | `[dependencies]` or `Cargo.lock` changes | The admission rule (std first, features, Δ size and crates, MSRV, unsafe rows, licenses), no C-building `-sys` crates (static musl builds), grouped upgrades, signal-hook held on 0.3 |
| `.claude/skills/hot-path/SKILL.md` | agent, source, cgroup, procfs, wire, state, or the input, row and draw code of tui change | Zero allocation per tick, syscall and fd-cache budgets, agent CPU and RSS, render budgets (O(visible), ≤ 4 Hz, ≤ 10 ms decode per batch, flat sort, history ring, self-monitoring cost), the input-thread rule, and the measurement commands |
| `.claude/skills/unsafe-policy/SKILL.md` | Any `unsafe`, FFI or raw syscall, or a dependency with unsafe | The decision tree (std, in-tree safe API, drop the feature, then our own unsafe), the `#[allow(unsafe_code, reason)]` plus `// SAFETY:` plus ledger entry rule |
| `CLAUDE.md` | Always | The program's shape, which skill to load, `mise run check` before done, and the "never" list |

**Gates behind the skills:** `cargo test --test budget` (lines, docs, dependency rows, unsafe entries), `mise run size`, `mise run unsafe-deps`, `mise run deny`, `mise run msrv`, and clippy with the §13.2 lints.

---

## 18. CI and GitHub Actions design

Every job runs `mise run <task>` through `jdx/mise-action@v5` (current major, checked with `git ls-remote`), so CI matches local runs. Each job installs only the mise tools it needs (`install_args`, or `install: false`) and sets `MISE_TASK_RUN_AUTO_INSTALL=false`, so no task installs the rest. Rust comes from `rust-toolchain.toml` through rustup, which is preinstalled on the runner. `Swatinem/rust-cache@v2` runs on every cargo job that compiles. Existing pins are kept: `actions/checkout@v6`, `github/codeql-action/upload-sarif@v4`.

| File | Trigger | Jobs |
|---|---|---|
| `.github/workflows/checks.yml` (reusable) | `workflow_call` | `reuse` (`mise run reuse`, reuse 6.2.0, replacing `fsfe/reuse-action`); `fmt`; `clippy` (`mise run lint:sarif` with the pinned clippy-sarif and sarif-fmt, then `upload-sarif` with `if: always()`, `security-events: write` kept); `build`; `test` (includes the budget test); `msrv` (`mise run msrv`); `deny` (`mise run deny`); `size` (`mise run size`, `size:self-test`, `unsafe-deps`, `unsafe-deps:self-test`; `size.sh` writes its numbers to `$GITHUB_STEP_SUMMARY`) |
| `.github/workflows/integration.yml` (reusable) | `workflow_call`, `workflow_dispatch` | `sind`, see below |
| `.github/workflows/ci.yml` | `push: [main]`, `pull_request`, `schedule: cron "0 4 * * 1"` (catches moving sind images) | `checks` and `integration` in parallel. Permissions `contents: read`, `security-events: write` |
| `.github/workflows/publish.yml` | tag `v*` | `checks`; `integration`; `publish` (needs both; tag/version check and `cargo publish` kept); `binaries` (needs both; see below); `release` (needs `publish` and `binaries`) |
| `.github/dependabot.yml` | monthly (kept) | cargo: `groups: { arrow: ["arrow-*"], tui: ["ratatui*", "crossterm", "signal-hook"] }`, `ignore: [{ dependency-name: "signal-hook", versions: [">=0.4"] }]` (crossterm 0.29 requires `^0.3.17`; a 0.4 bump would add a second signal-hook; recorded in `docs/deps.md` and lifted when crossterm moves); github-actions: one group for all actions |
| cargo-release (`[package.metadata.release]`) | — | Unchanged (`publish = false`, signed commit and tag, `push-remote = "upstream"`). The tool is pinned via mise at 1.1.6; `CONTRIBUTING.md` installs it with `mise install` |

**`integration.yml`, job `sind`.**

- **Matrix (D42):** `25.11` (no `EXTRA`), `26.05` with `extra: yes`, and `26.05` with `extra: yes` and `jobidpaths: yes`. `fail-fast: false`, `runs-on: ubuntu-latest`, `timeout-minutes: 30`.
- **Job `env`:** `SIND_REALM: unjam-it` (the local default too; the mise template passes it through and it goes to the action's `realm` input, D37); `SIND_PROGRESS: plain`; `SIND_PROGRESS_LOG: ${{ github.workspace }}/sind-events.jsonl`; `MISE_TASK_RUN_AUTO_INSTALL: false`; `SLURM`, `EXTRA` and `JOBIDPATHS` from the matrix. sind is not a mise tool (§13.3), so nothing has to be disabled.
- **Steps:**
  1. `actions/checkout@v6`
  2. `jdx/mise-action@v5` with `install: false`
  3. `Swatinem/rust-cache@v2`
  4. `mise run build:musl` (the nodes run the static binary from `/data`)
  5. `mise run it:cluster`, which writes `target/sind-cluster.yaml`
  6. `GSI-HPC/sind-action@v2` with the inputs below. The `clusters` input is a YAML list, and each entry is a file path or an inline config mapping (action README). `pull` and `verify` stay at their defaults (`true`).
     ```yaml
     with:
       version: v0.15.0
       wait: 10m
       realm: ${{ env.SIND_REALM }}
       clusters: |
         - ${{ github.workspace }}/target/sind-cluster.yaml
     ```
  7. `mise run it:test`
  8. On failure, `actions/upload-artifact@v7` with `target/sind-*` and `sind-events.jsonl`; later tests add their captures, srun stderr and records.

Integration needs no cleanup step: runners are ephemeral.

**`publish.yml`, job `binaries` (D36 revised).**

- Matrix: `amd64` on `ubuntu-latest` (`x86_64-unknown-linux-musl`) and `arm64` on `ubuntu-24.04-arm` (`aarch64-unknown-linux-musl`). A guarded step installs rustup where the runner image lacks it.
- Steps: `mise run build:musl`; copy the binary to `unjam-linux-<arch>`; `actions/attest-build-provenance@v4` on it (permissions `id-token: write`, `attestations: write`); `actions/upload-artifact@v7`.
- Job `release` (permissions `contents: write`) downloads both with `actions/download-artifact@v8`, writes `checksums.txt` with `sha256sum`, and runs `gh release create <tag> unjam-linux-amd64 unjam-linux-arm64 checksums.txt --verify-tag --generate-notes`. It needs `publish`, so a tag whose version does not match `Cargo.toml` gets no release. `gh` is preinstalled, so no third-party release action is needed.

---

## 19. Implementation phases

### Phase 2: scaffold, mise, skills, CI

**Deliverables:**

- `Cargo.toml`: rust-version, lints, profiles, exclude.
- `clippy.toml`, `rustfmt.toml`, `deny.toml` (with `[graph] targets`), `mise.toml`, `rust-toolchain.toml` components.
- `REUSE.toml` annotations for `tests/fixtures/**` and `.claude/**`.
- `CLAUDE.md` and the four skills (§17).
- `docs/{design.md, budget.txt, deps.md, unsafe.md, perf.md}`. README sections "Site notes" (cgroup v2, Slurm 25.11+, optional `EnableExtraControllers=io,pids` for the IO and PIDS columns, overlap-step policy) and "Recording" (the pyarrow Parquet recipe and forward-fill example).
- `tests/budget.rs`, `scripts/size.sh`, `scripts/unsafe-deps.sh`.
- `tests/sind/{cluster.sh, run.sh, it.sh}` with a smoke test only: `sinfo`, `srun -N3 hostname` as alice, and the job cgroup's `cgroup.subtree_control` (found by structure) containing `io` and `pids` where `EXTRA` is set.
- Workflows from §18 (including the release `binaries` job and `mise run build:musl`), and the `CONTRIBUTING.md` update.

**Acceptance:**

1. `mise install && mise run check` passes on a clean clone with toolchain 1.93.1.
2. `cargo release patch` (dry run, no `--execute`) runs.
3. CI is green: checks, the SARIF upload still appears in code scanning, and all 3 integration matrix entries pass the smoke test through `sind-action@v2` with the pinned `version`, `wait` and `realm`.
4. `size.sh --self-test`, `unsafe-deps.sh --self-test` and the budget test fail on purpose when limits are lowered or a row is removed.
5. `reuse lint` passes.

### Phase 3: core host view

**Deliverables:** `model`, `source`, `cgroup`, `procfs`, `agent` (local thread), `wire`, `state` (the local layouts: tree and flat; CPU history), `tui` (input thread, self-monitoring header, history bars, `r` recording toggle once `record.rs` exists), and a `theme` placeholder (`mono` plus `nord`), `--batch`, `--rule`.

**Acceptance:**

1. `unjam` shows the cgroup tree with processes on a developer host and on a sind worker. `unjam --batch 2` output passes the golden test.
2. All unit tests in §14.1 for parsers, sampler, wire (including mux at 1000 agents), state and draw pass. The `alloc_count` test shows 0 allocations per steady-state sample and at most 2 per directory entry for the discovery walk.
3. On a sind worker, `strace -c` and bench numbers are within the hot-path budgets and recorded in `docs/perf.md`. That includes the per-file read cost of `cpu.stat`, `io.stat` and `memory.stat` (which settles K13 and D12) and the fd-cache hit ratio. The fd-cache self-check passes on real procfs and kernfs, and the forced-failure fallback is tested.
4. Key latency stays below 50 ms while sampling 2,000 synthetic processes (manual check plus the bounded-decode unit test).
5. LOC within budget (≈ 1,700 for the phase-3 modules). Binary and crate count gates are re-baselined.

### Phase 4: TUI themes (separate thread)

**Deliverables:** `THEMES = [nord (default), night, mocha, gruvbox, solar, amber, mono]`, using only the §9.5 slots. The mocks of each theme are the reference.

**Acceptance:**

1. No new dependencies and no renderer changes beyond slot usage.
2. A `TestBackend` snapshot test exists per theme.
3. `NO_COLOR` forces `mono`. `--theme`, `UNJAM_THEME` and `t` cycling work.

### Phase 5: Slurm attach

**Deliverables:** `slurm.rs` (job resolution, job query, `srun_args` with keepalive forwarding and `--bcast`, spawn, teardown), `cgroup::job_root`, agent mode (stdin and writer threads, keepalive, budget, stagger, self-CPU and RSS reporting, `--unlink-self`), `--slurm[=JOBID]` with `--nodelist`, `--bcast`, `--print-srun` and hidden `--agent-keepalive`, the step and node layouts with folded node ranges, and the node heat strip.

**Acceptance:**

1. IT 2, 3, 4, 5 and 8 pass on all 3 matrix entries. Together they prove:
   - `job_<id>` and SLUID handling;
   - isolation from co-located decoy jobs of the same user and of another user (all asserted RUNNING);
   - non-NA io and pids values where `EXTRA` is set, hidden IO columns and `~` PIDS where it is not;
   - selective rules;
   - teardown within 5 s and 15 s;
   - agent survival past `WaitTime` after another agent is killed;
   - `--bcast` runs agents without a shared filesystem and leaves no copy behind;
   - `errors=0` under the 100 ms × 1,500-process flood.
2. The `srun_args` golden tests and the mux test pass.
3. Agent CPU stays ≤ 1 % and RSS ≤ 16 MiB under IT 5. If CPU exceeds the budget under default rules, Q11 (automatic stretch) is raised with numbers.
4. srun throughput and login-node CPU are measured once at ≥ 256 tasks on a real cluster (Q10) and recorded in `docs/perf.md`. The budget defaults are adjusted if needed.

### Phase 6: record and replay

**Deliverables:** `record.rs` (`run_record`, generated schema, `Writer` with the 8,192-row cap, `Reader`, `Counting`), the `--replay` source, TUI recording (`r`, `--record DIR`), `tests/sind/check_record.py`, uv in `mise.toml`, and the README "Recording" section checked against the implementation. Adds arrow-array, arrow-schema, arrow-ipc and lz4_flex with deps rows and the generated unsafe rows.

**Acceptance:**

1. IT 6 and 7 pass on all matrix entries:
   - exit code pass-through;
   - pyarrow reads `.arrows` with field metadata;
   - io and pids columns are non-null where `EXTRA` is set;
   - the documented Parquet recipe gives the same row count;
   - `--replay … --batch` lists `dd`;
   - a TUI recording (`--record DIR`) replays like a `record` journal;
   - a killed job leaves a readable prefix.
2. Unit tests pass: 200-offset truncation, seek equals linear replay, the batch cut at 8,192 rows.
3. Recorder RSS ≤ 8 MiB at 1,000 entities and when the row cap is reached, with a loss window of at most 10 s (one batch).
4. Lifetime rule verified with a daemonizing child: the recorder waits for the descendant.
5. Size and crate gates hold at their re-baselined phase-6 limits (target ≤ 3.25 MiB, ≤ 96 crates). The safe-lz4 throughput cost is measured.

---

### Sources

- Slurm source: `src/common/io_hdr.h` (`SLURM_IO_MAX_MSG_LEN 1024`), `src/slurmd/slurmstepd/io.c` (buffered `cbuf_read_line`, `must_truncate`), `src/api/step_io.c`, `src/plugins/cgroup/v2/cgroup_v2.c:85-97, 505-517` (extra controllers io, pids, hugetlb, rdma, misc disabled unless `EnableExtraControllers`) — https://github.com/SchedMD/slurm
- Slurm docs, 25.11 and 26.05: `doc/html/cgroup_v2.shtml` (`job_<id>` vs SLUID, `CgroupJobIdPaths`); `cgroup.conf(5)` 25.11 (`EnableExtraControllers`); srun(1) 25.11 (`--overlap`, `--input`, `-W/--wait` "applies to job allocations" and the 60 s warning for 0, `--kill-on-bad-exit`, `--bcast` defaulting to the `--chdir` directory, `-u`)
- Linux v6.12: `kernel/cgroup/rstat.c` (`cgroup_base_stat_cputime_show` → `cgroup_rstat_flush_hold`; flush loop over `rstat_css_list`), `block/blk-cgroup.c` (`blkcg_print_stat` → `cgroup_rstat_flush`, also in v5.14)
- crates.io metadata as checked by the design authors, judges and the round-2 review:
  - ratatui 0.30.2 (→ ratatui-crossterm with default features; ratatui-crossterm 0.1.2 defaults `crossterm_0_29`, `underline-color`)
  - ratatui-core 0.1.2 (→ lru, compact_str, thiserror)
  - crossterm 0.29 (→ parking_lot, mio, rustix, signal-hook ^0.3.17, signal-hook-mio)
  - arrow-* 60.0.0 (ahash `runtime-rng` on non-wasm → getrandom 0.3, once_cell; `compile-time-rng` on wasm32 → const-random-macro → tiny-keccak, CC0-1.0)
  - clap 4.6; lz4_flex 0.14.0 (rust-version 1.81)
  - cargo-release 1.1.6 (1.92) and 1.1.7 (1.97); cargo-deny 0.20.2 (rust_version 1.88)
  - signal-hook 0.3 and 0.4.5
- `GSI-HPC/sind-action` README: inputs `version` (default `latest`), `verify`, `token`, `clusters` (YAML list of file paths or inline configs), `pull`, `wait`, `realm`
- mise: `[env]` values override the inherited process environment (local test with mise 2026.10.21); `get_env` template function; `disable_tools` / `MISE_DISABLE_TOOLS` setting
- sind docs, `docs/content/` in https://github.com/GSI-HPC/sind at v0.15.0:
  - `guides/ci-cd.md` (realms only matter on a shared Docker daemon) and `guides/monitoring.md` (sind-action@v2 `clusters:` list)
  - `configuration/cluster-config.md` (`slurm.main` → slurm.conf, `slurm.cgroup` → cgroup.conf)
  - `usage/cluster-lifecycle.md` (`--wait DURATION`, default 5m)
  - `usage/node-access.md` (shared `/home`, `/data` mount, `sind exec -u`)
  - `architecture/overview.md` (`--cgroupns=private`)
  - `architecture/slurm-config.md` (`MpiDefault=pmix`, `proctrack/cgroup`, `task/cgroup`, `CR_Core_Memory`, `DefMemPerCPU = memory/cpus`, `CgroupPlugin=autodetect` only)
  - `container-images/building-images.md` (tags `25.11`, `26.05`, `vX.Y.Z-<YY>.<MM>`)
  - `configuration/cluster-config.md` users section (bare names; `sind exec CLUSTER --user U`, `sind ssh U@worker-N.CLUSTER`)
- GitHub Actions majors checked with `git ls-remote --tags`: `jdx/mise-action@v5`, `actions/attest-build-provenance@v4`, `actions/upload-artifact@v7`, `actions/download-artifact@v8`; `GSI-HPC/sind-action@v2`
