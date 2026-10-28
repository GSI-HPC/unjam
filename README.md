# `unjam` - Utility for Node/Job Analysis and Monitoring

unjam is a `top`-like terminal UI for Linux hosts and Slurm jobs. Its tree is the cgroup v2
hierarchy, filled with the processes of each cgroup. For a Slurm job it starts one small agent
per node in an overlapping job step and shows all nodes in one view. It can also record a job
step to Arrow files and replay them.

unjam is under development; [docs/design.md](docs/design.md) describes where it is going.

## Usage

```sh
unjam                          # this host
unjam --slurm                  # $SLURM_JOB_ID, or your only running job
unjam --slurm=4242             # job 4242 (the = is required)
unjam --replay rec/*.arrows    # replay recordings, one file per node
srun unjam record -o rec/%j.%s-%n.arrows -- ./app args   # record a job step
```

Keys: `↑↓` move, `←→` fold, `v` layout (step, node, flat), `s` sort, `/` filter, `space` mark,
`p` sampling level, `+`/`-` sampling rate, `r` record, `t` theme, `?` help, `q` quit.
`--batch N` prints N text snapshots instead of starting the TUI.

## Site notes

- **cgroup v2 only.** A cgroup2 mount at `/sys/fs/cgroup` is required (or `--root` pointing at
  one); pure v1 setups are refused at startup.
- **Slurm 25.11 or later** with the cgroup/v2 plugin (`proctrack/cgroup`, `task/cgroup`). The job
  cgroup is found by structure, so both `job_<id>` directories and the SLUID directories of 26.05
  work.
- **IO and PIDS columns (optional).** Slurm enables the io and pids controllers in job cgroups
  only with `EnableExtraControllers=io,pids` in `cgroup.conf`. Without it, cgroup rows show no
  IO values (processes still do) and PIDS shows the summed thread count of the sampled
  processes, marked `~`. Nothing else depends on it.
- **Overlap steps.** `unjam --slurm` runs `srun --jobid=<J> --overlap` with one task per node,
  which Slurm allows for the job owner, operators and root. The step shares the job's resources
  and takes none away. Sites whose `job_submit` or `cli_filter` plugins, SPANK plugins or
  `MaxStepCount` restrict steps must allow one extra overlap step per viewer; `--srun-arg`
  passes further srun options.
- **No shared filesystem.** `--bcast` copies the binary to `/tmp/unjam.<J>.<uid>.<random>` on
  each node, and the agent removes the copy when it starts. Where `/tmp` is mounted `noexec`,
  name another directory: `--bcast=/var/tmp`. `--bcast` refuses another user's job, whose
  owner may control the directory; use `--agent` there. Static binaries for x86_64 and aarch64
  are attached to each [GitHub release](https://github.com/GSI-HPC/unjam/releases).

## Recording

`unjam record` writes one Arrow IPC stream (`.arrows`, LZ4-compressed) per node. Every batch
covers at most 10 s or 8,192 rows and decodes on its own, so a killed job leaves a readable
prefix. `-o` takes `%j` (job), `%s` (step), `%n` (host) and `%%`; existing files are never
overwritten. `-i` sets the interval (default 5s), `--no-procs` records cgroups only. The
recorder keeps sampling until the app and everything it left in the step are gone, and exits
with the app's exit code. After SIGTERM it finishes the journal within 5 s and exits with
128 + signal.

In the TUI, `r` (or `--record DIR` with `--slurm`) records what the viewer receives into
`DIR/<host>.arrows` (default DIR `unjam-rec-<secs>`); `r` again stops.

`unjam --replay FILE...` shows recordings on one shared timeline. Keys: `space` play/pause,
`,` `.` one tick, `<` `>` 60 s, `g` `G` start/end. `--at TIME` starts at `start`, `end`,
`+DUR`, `-DUR` or `HH:MM[:SS]` (UTC); with `--batch N` it prints N snapshots from there.

Columns: `ts`, `kind` (1 = cgroup, 2 = process), `cg`, `pid`, `start`, `ppid`, `path`, `comm`,
`cmdline`, and one `UInt64` column per metric (such as `cg_cpu_usage_usec` or `p_rss_bytes`)
with its unit and kind in the field metadata. Each tick is a complete snapshot of the recorded
scope.

Convert a recording to Parquet with pyarrow:

```sh
uv run --with pyarrow==21.0.0 python -c "import sys,pyarrow as pa,pyarrow.parquet as pq; \
  pq.write_table(pa.ipc.open_stream(sys.argv[1]).read_all(), sys.argv[2], compression='lz4')" X.arrows X.parquet
```

`start`, `ppid`, `path`, `comm` and `cmdline` are filled only on the first row of each entity
in a batch. To have them on every row, forward-fill them per entity:

```python
import pandas as pd, pyarrow as pa

df = pa.ipc.open_stream("X.arrows").read_all().to_pandas(types_mapper=pd.ArrowDtype)
# An entity is a cgroup (kind 1, by cg) or a process (kind 2, by pid); a reused pid starts
# over with a new first row.
entity = df["pid"].astype("uint64[pyarrow]").where(df["kind"] == 2, df["cg"])
keyframe = ["start", "ppid", "path", "comm", "cmdline"]
df[keyframe] = df.groupby([df["kind"], entity])[keyframe].ffill()
```

## License

© GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany

SPDX-License-Identifier: LGPL-3.0-or-later
