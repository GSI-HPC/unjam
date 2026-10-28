// SPDX-License-Identifier: LGPL-3.0-or-later
// SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany

//! The cgroup v2 source, the discovery walk, `cgroup.procs` parsing, the v2 startup check and
//! the structural Slurm job filter (§4.3, §5.1, §5.5).

use std::os::unix::fs::{DirEntryExt, MetadataExt};
use std::{fs, path::Path, path::PathBuf};

use crate::model::{Ent, Kind, Metric, NA, metric as m};
use crate::source::{Env, Files, Sampled, Slot, Source, Target, kv, kv_eq, num};

/// Files read per cgroup at level `c`, in slot order; the first two are hot (§5.2, §5.3).
const FILES: [&str; 10] = [
    "cpu.stat",
    "memory.current",
    "memory.peak",
    "memory.events",
    "memory.stat",
    "io.stat",
    "pids.current",
    "cpu.pressure",
    "memory.pressure",
    "io.pressure",
];

/// The `cpu.stat` keys behind the first four metrics.
const CPU_KEYS: [&str; 4] = ["usage_usec", "user_usec", "system_usec", "throttled_usec"];

/// Round-one cgroup metrics (§4.3). Names are append-only: they are wire and column identity.
pub const METRICS: &[Metric] = &[
    m("cg_cpu_usage_usec", "us", Kind::Counter),
    m("cg_cpu_user_usec", "us", Kind::Counter),
    m("cg_cpu_system_usec", "us", Kind::Counter),
    m("cg_cpu_throttled_usec", "us", Kind::Counter),
    m("cg_mem_current_bytes", "B", Kind::Gauge),
    m("cg_mem_peak_bytes", "B", Kind::Gauge),
    m("cg_oom_kill", "1", Kind::Counter),
    m("cg_mem_anon_bytes", "B", Kind::Gauge),
    m("cg_mem_file_bytes", "B", Kind::Gauge),
    m("cg_io_read_bytes", "B", Kind::Counter),
    m("cg_io_write_bytes", "B", Kind::Counter),
    m("cg_pids_current", "1", Kind::Gauge),
    m("cg_psi_cpu_some_usec", "us", Kind::Counter),
    m("cg_psi_mem_some_usec", "us", Kind::Counter),
    m("cg_psi_io_some_usec", "us", Kind::Counter),
];

/// The cgroup v2 source. It holds the stacked devices `io_sum` skips; the rest is in the slots.
pub struct Cgroup(Vec<String>);

/// The source registry entry: reads the stacked block devices once from the sysfs next to
/// `proc_root` (`/sys` in production, absent in fixture trees).
#[allow(clippy::unnecessary_wraps, reason = "the Probe signature; other sources can fail")]
pub fn probe(env: &Env) -> Result<Box<dyn Source>, String> {
    Ok(Box::new(Cgroup(stacked(&env.proc_root.with_file_name("sys/class/block")))))
}

/// `MAJ:MIN ` of each disk that is, or has a partition that is, held by a dm or md device:
/// `io.stat` charges a stacked bio again on the disk below, so only the top device is summed.
fn stacked(class_block: &Path) -> Vec<String> {
    let devs = fs::read_dir(class_block).into_iter().flatten().flatten().map(|e| e.path());
    let held = |d: &PathBuf| fs::read_dir(d.join("holders")).is_ok_and(|mut h| h.next().is_some());
    let disk = |d: PathBuf| d.join(if d.join("partition").exists() { "../dev" } else { "dev" });
    let dev = |d| fs::read_to_string(disk(d)).ok().map(|t| format!("{} ", t.trim()));
    devs.filter(held).filter_map(dev).collect()
}

impl Source for Cgroup {
    /// Cgroup metrics fill the cgroup value vector.
    fn entity(&self) -> Ent {
        Ent::Cgroup
    }

    /// The round-one table.
    fn metrics(&self) -> &'static [Metric] {
        METRICS
    }

    /// Reads each file through its slot; a missing file leaves its metrics NA.
    fn sample(
        &mut self,
        t: Target,
        slots: &mut [Slot],
        io: &mut Files,
        out: &mut [u64],
    ) -> Sampled {
        let Target::Cgroup { dir } = t else { return Ok(()) };
        out.fill(NA);
        for (i, (slot, name)) in slots.iter_mut().zip(FILES).enumerate() {
            let Ok(b) = io.read(slot, i < 2, dir, name) else { continue };
            match i {
                0 => CPU_KEYS.iter().zip(&mut *out).for_each(|(k, o)| *o = kv(b, k).unwrap_or(NA)),
                1 | 2 => out[i + 3] = num(b).unwrap_or(NA),
                3 => out[6] = kv(b, "oom_kill").unwrap_or(NA),
                4 => (out[7], out[8]) = (kv(b, "anon").unwrap_or(NA), kv(b, "file").unwrap_or(NA)),
                5 => (out[9], out[10]) = io_sum(b, &self.0),
                6 => out[11] = num(b).unwrap_or(NA),
                _ => out[i + 5] = psi(b),
            }
        }
        Ok(())
    }
}

/// The `some … total=` microseconds of a PSI file (its first line).
fn psi(b: &[u8]) -> u64 {
    b.split(|&c| c == b'\n').next().and_then(|l| kv_eq(l, "total")).unwrap_or(NA)
}

/// Sums `rbytes` and `wbytes` over the devices of `io.stat` except the `skip` prefixes (no
/// devices gives 0, 0).
pub fn io_sum(text: &[u8], skip: &[String]) -> (u64, u64) {
    let top = |l: &&[u8]| !skip.iter().any(|d| l.starts_with(d.as_bytes()));
    text.split(|&b| b == b'\n').filter(top).fold((0, 0), |(r, w), line| {
        let get = |k| kv_eq(line, k).unwrap_or(0);
        (r.saturating_add(get("rbytes")), w.saturating_add(get("wbytes")))
    })
}

/// Parses `cgroup.procs` into a sorted pid list for the merge-diff against the previous one.
pub fn procs(text: &[u8], out: &mut Vec<u32>) {
    out.clear();
    out.extend(text.split(|&b| b == b'\n').filter_map(|l| u32::try_from(num(l)?).ok()));
    out.sort_unstable();
}

/// Discovery: `(dir, inode, parent inode)` of `root` (parent 0) and of every directory below
/// it, parents first, without the `exclude` subtree (the agent's own step). Empty if `root` is
/// gone.
pub fn walk(root: &Path, exclude: Option<&Path>) -> Vec<(PathBuf, u64, u64)> {
    let mut found = Vec::new();
    if let Ok(meta) = fs::metadata(root) {
        found.push((root.to_path_buf(), meta.ino(), 0));
        descend(root, meta.ino(), exclude, &mut found);
    }
    found
}

/// The recursive part of `walk`; a directory that vanished meanwhile is skipped.
fn descend(dir: &Path, ino: u64, exclude: Option<&Path>, found: &mut Vec<(PathBuf, u64, u64)>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let path = e.path();
        if e.file_type().is_ok_and(|t| t.is_dir()) && exclude != Some(path.as_path()) {
            found.push((path.clone(), e.ino(), ino));
            descend(&path, e.ino(), exclude, found);
        }
    }
}

/// The path of the unified (`0::`) line of `/proc/self/cgroup`. Hybrid hosts list v1 lines
/// too; whether `/sys/fs/cgroup` really is cgroup2 is checked by `v2_check`.
fn v2_path(self_cgroup: &str) -> Result<&str, String> {
    self_cgroup.lines().find_map(|l| l.strip_prefix("0::")).ok_or_else(|| V1.into())
}

/// The refusal text for v1 and hybrid hierarchies.
const V1: &str = "cgroup v2 only: no cgroup2 hierarchy at /sys/fs/cgroup (use --root to point at a cgroup2 mount)";

/// Startup check (§5.1): `root` must be a cgroup2 directory and, unless it was given explicitly,
/// this process must live in a pure v2 hierarchy. Returns the own cgroup directory (below
/// `mount`) for the self-check, or `root` when it cannot be located.
pub fn v2_check(
    mount: &Path,
    root: &Path,
    self_cgroup: &str,
    explicit: bool,
) -> Result<PathBuf, String> {
    let own = match v2_path(self_cgroup) {
        Ok(p) => mount.join(p.trim_start_matches('/')),
        Err(e) if !explicit => return Err(e),
        Err(_) => root.to_path_buf(),
    };
    if !root.join("cgroup.controllers").is_file() {
        let e = format!("{}: not a cgroup v2 directory (no cgroup.controllers)", root.display());
        return Err(if explicit { e } else { V1.into() });
    }
    Ok(if own.join("cpu.stat").is_file() { own } else { root.to_path_buf() })
}

/// Splits `0::<prefix>/<job>/step_<S>/user/task_<T>` into the job and step paths relative to the
/// mount. Structural only: it never looks for `job_<id>`, so 26.05 SLUID names work too.
fn step_paths(self_cgroup: &str) -> Result<(&str, &str), String> {
    let path = v2_path(self_cgroup)?.trim_matches('/');
    let mut it = path.rsplitn(4, '/');
    match (it.next(), it.next(), it.next(), it.next()) {
        (Some(task), Some("user"), Some(step), job)
            if task.starts_with("task_") && step.starts_with("step_") =>
        {
            let job = job.unwrap_or("");
            Ok((job, &path[..job.len() + usize::from(!job.is_empty()) + step.len()]))
        }
        _ => Err("not inside a Slurm step (or the step runs in a cgroup namespace rooted below \
                  the job)"
            .to_owned()),
    }
}

/// Attach mode: the job root and the agent's own step, which the walk excludes (§5.5). The step
/// must exist below `mount`: a path from a cgroup namespace joined onto a host-view mount would
/// otherwise make the whole mount the job.
pub fn job_root(mount: &Path, self_cgroup: &str) -> Result<(PathBuf, PathBuf), String> {
    let (job, step) = step_paths(self_cgroup)?;
    let own = mount.join(step);
    let err = format!("{}: own step not found (cgroup namespace)", own.display());
    own.is_dir().then(|| (mount.join(job), own)).ok_or(err)
}

/// Record mode: `…/step_<S>/user`, every task of the own step on this node (§3.5).
pub fn record_root(mount: &Path, self_cgroup: &str) -> Result<PathBuf, String> {
    Ok(mount.join(step_paths(self_cgroup)?.1).join("user"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(rel: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cgroup").join(rel)
    }

    fn sample(dir: &Path) -> Vec<u64> {
        let mut slots: Vec<Slot> = (0..METRICS.len()).map(|_| Slot::Empty).collect();
        let mut out = vec![0; METRICS.len()];
        let mut src = Cgroup(Vec::new());
        src.sample(Target::Cgroup { dir }, &mut slots, &mut Files::new(64), &mut out).unwrap();
        out
    }

    #[test]
    fn samples_every_metric() {
        let task1 = "system.slice/slurmstepd.scope/job_42/step_0/user/task_1";
        let v = sample(&fixture("25.11").join(task1));
        let want = [
            1_000_003,
            800_001,
            200_002,
            7,
            1_048_576,
            2_097_152,
            1,
            524_288,
            262_144,
            4096 + 100,
            8192,
            3,
            1111,
            1111,
            1111,
        ];
        assert_eq!(v, want);
        let v = sample(&fixture("no-controllers").join(task1));
        let na = [5, 9, 10, 11, 12, 13, 14];
        for (i, x) in v.iter().enumerate() {
            assert_eq!(*x == NA, na.contains(&i), "metric {}", METRICS[i].name);
        }
    }

    #[test]
    fn io_and_procs_parsers() {
        let two = b"8:0 rbytes=1 wbytes=2 rios=3\n259:0 rbytes=10 wbytes=20\n";
        assert_eq!(io_sum(two, &[]), (11, 22));
        assert_eq!(io_sum(two, &["8:0 ".into()]), (10, 20), "8:0 is stacked");
        assert_eq!(io_sum(b"", &[]), (0, 0));
        let mut out = vec![9];
        procs(b"30\n4\n100\n", &mut out);
        assert_eq!(out, [4, 30, 100]);
    }

    #[test]
    fn stacked_devices_are_the_held_disks() {
        let tmp = std::env::temp_dir().join(format!("unjam-stacked-{}", std::process::id()));
        let devices = tmp.join("devices");
        for (dir, dev, holder) in [
            ("sda", "8:0", None),
            ("sda/sda2", "8:2", Some("dm-0")),
            ("dm-0", "253:0", None),
            ("nvme0n1", "259:0", Some("md0")),
            ("nvme1n1", "259:1", None),
        ] {
            fs::create_dir_all(devices.join(dir).join("holders")).unwrap();
            fs::write(devices.join(dir).join("dev"), format!("{dev}\n")).unwrap();
            if let Some(h) = holder {
                fs::write(devices.join(dir).join("holders").join(h), "").unwrap();
            }
        }
        fs::write(devices.join("sda/sda2/partition"), "2\n").unwrap();
        fs::create_dir_all(tmp.join("class")).unwrap();
        for dir in ["sda", "sda/sda2", "dm-0", "nvme0n1", "nvme1n1"] {
            let name = dir.rsplit('/').next().unwrap();
            std::os::unix::fs::symlink(devices.join(dir), tmp.join("class").join(name)).unwrap();
        }
        let mut got = stacked(&tmp.join("class"));
        got.sort();
        assert_eq!(got, ["259:0 ", "8:0 "], "a held partition marks its disk");
        assert!(stacked(&tmp.join("nope")).is_empty());
        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn walk_reports_parents_first_and_excludes() {
        let root = fixture("26.05-sluid/system.slice/slurmstepd.scope/s01JC4X8Q2RZ7M");
        let seen: Vec<_> = walk(&root, Some(&root.join("step_batch")))
            .into_iter()
            .map(|(p, ino, parent)| (p.strip_prefix(&root).unwrap().to_owned(), ino, parent))
            .collect();
        assert!(walk(&root.join("nope"), None).is_empty());
        assert_eq!(seen.len(), 7);
        assert_eq!(seen[0].2, 0);
        for (p, _, parent) in &seen[1..] {
            let up = seen.iter().position(|(_, ino, _)| ino == parent).unwrap();
            assert_eq!(Some(seen[up].0.as_path()), p.parent(), "{p:?}");
        }
        assert!(!seen.iter().any(|(p, ..)| p.starts_with("step_batch")));
    }

    #[test]
    fn slurm_layout() {
        let tmp = std::env::temp_dir().join(format!("unjam-layout-{}", std::process::id()));
        let mount = tmp.as_path();
        for (cg, job) in [
            (
                "0::/system.slice/slurmstepd.scope/job_42/step_3/user/task_0\n",
                "system.slice/slurmstepd.scope/job_42",
            ),
            (
                "0::/system.slice/slurmstepd.scope/s01JC4X8Q2RZ7M/step_3/user/task_7\n",
                "system.slice/slurmstepd.scope/s01JC4X8Q2RZ7M",
            ),
            (
                "0::/system.slice/node1_slurmstepd.scope/job_9/step_3/user/task_special\n",
                "system.slice/node1_slurmstepd.scope/job_9",
            ),
            ("0::/step_3/user/task_0\n", ""),
            (
                "8:pids:/\n1:cpu:/\n0::/system.slice/slurmstepd.scope/job_7/step_3/user/task_0\n",
                "system.slice/slurmstepd.scope/job_7",
            ),
        ] {
            fs::create_dir_all(mount.join(job).join("step_3")).unwrap();
            let (root, own) = job_root(mount, cg).unwrap();
            assert_eq!(root, mount.join(job), "{cg}");
            assert_eq!(own, mount.join(job).join("step_3"));
            assert_eq!(record_root(mount, cg).unwrap(), mount.join(job).join("step_3/user"));
        }
        for bad in [
            "0::/\n",
            "0::/user.slice/user-1000.slice/session-3.scope\n",
            "0::/system.slice/slurmstepd.scope/job_42/step_3/slurm\n",
            "0::/system.slice/slurmstepd.scope/job_42/step_3/user\n",
            "12:pids:/\n1:cpu:/\n",
            "",
        ] {
            assert!(job_root(mount, bad).is_err(), "{bad:?}");
        }
        let host_view = fixture("25.11");
        let err = job_root(&host_view, "0::/step_3/user/task_0\n").unwrap_err();
        assert!(err.contains("cgroup namespace"), "{err}");
        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn v2_startup_check() {
        let mount = fixture("25.11");
        let root = mount.join("system.slice/slurmstepd.scope/job_42");
        let me = "0::/system.slice/slurmstepd.scope/job_42/step_0/user/task_0\n";
        assert_eq!(v2_check(&mount, &root, me, false).unwrap(), root.join("step_0/user/task_0"));
        let hybrid = "8:pids:/\n1:cpu:/\n0::/\n";
        assert_eq!(v2_check(&mount, &root, hybrid, false).unwrap(), root, "unified line of hybrid");
        let err = v2_check(&mount, &mount, hybrid, false).unwrap_err();
        assert!(err.contains("cgroup v2 only"), "{err}");
        assert!(v2_check(&mount, &root, "1:cpu:/\n", false).unwrap_err().contains("v2 only"));
        assert_eq!(v2_check(&mount, &root, "1:cpu:/\n", true).unwrap(), root, "explicit --root");
        let not_v2 = v2_check(&mount, &mount.join("nope"), me, true).unwrap_err();
        assert!(not_v2.contains("not a cgroup v2 directory"), "{not_v2}");
    }
}
