// SPDX-License-Identifier: LGPL-3.0-or-later
// SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany

//! The metric-source plug-in surface (R10) and the single hot-path read routine with its bounded,
//! tiered fd cache (§4.4, §5.2).

use std::time::{Duration, Instant};
use std::{fs::File, io, os::unix::fs::FileExt, path::Path, path::PathBuf};

use crate::model::{Ent, Metric};
use io::ErrorKind::{NotFound, PermissionDenied};

/// `ESRCH`: a cached `/proc/<pid>/stat` fd reports it once its process has exited.
pub const ESRCH: i32 = 3;
/// `ENODEV`: reads on a removed cgroup's open files fail with it.
const ENODEV: i32 = 19;

/// One entity's cached file. `Missing` remembers an absent or unreadable file (controller not
/// enabled, old kernel, no permission) so it is not retried every tick (the walk retries it).
pub enum Slot {
    Empty,
    Open(File),
    Missing,
}

/// What a source samples; an enum instead of generics. The entity's fd slots travel separately.
#[derive(Clone, Copy, Debug)]
pub enum Target<'a> {
    Cgroup { dir: &'a Path },
    Proc { pid: u32, start: u64, comm: &'a str },
}

/// The metric-source extension point (R10). `&mut self`, because future likwid or PAPI handles
/// carry state.
pub trait Source {
    /// The entity whose value vector this source fills a slice of.
    fn entity(&self) -> Ent;
    /// Ordered descriptors; their count is also the number of fd slots the source may use.
    fn metrics(&self) -> &'static [Metric];
    /// Fills `out` (one value per metric, `NA` when unknown); `Err` means the entity is gone.
    fn sample(&mut self, t: Target, slots: &mut [Slot], io: &mut Files, out: &mut [u64])
    -> Sampled;
}

/// What `Source::sample` returns: `Err` means the entity is gone.
pub type Sampled = io::Result<()>;

/// Injected roots and constants, so every reader runs against fixture trees in tests.
pub struct Env {
    pub proc_root: PathBuf,
    /// Own cgroup directory, for the kernfs half of the self-check.
    pub self_cg: PathBuf,
    pub page_size: u64,
    pub clk_tck: u64,
}

/// A source constructor; `Err(reason)` becomes one `W` line and the source is skipped.
pub type Probe = fn(&Env) -> Result<Box<dyn Source>, String>;

/// The compile-time source registry: a new source is one line here.
pub const SOURCES: &[(&str, Probe)] =
    &[("cgroup", crate::cgroup::probe), ("proc", crate::procfs::probe)];

/// Runs every probe; failures come back as warnings, never as errors.
pub fn sources(env: &Env) -> (Vec<Box<dyn Source>>, Vec<String>) {
    let mut warn = Vec::new();
    let mut skip = |name, e| warn.push(format!("source {name} skipped: {e}"));
    let ok = SOURCES.iter().filter_map(|(name, probe)| probe(env).map_err(|e| skip(name, e)).ok());
    (ok.collect(), warn)
}

/// Bounded, tiered fd cache and the reused read buffer: the single hot-path read policy (§5.2).
pub struct Files {
    buf: Vec<u8>,
    path: PathBuf,
    pub open: usize,
    pub cap: usize,
}

impl Files {
    /// An empty cache that may keep up to `cap` fds open.
    pub fn new(cap: usize) -> Self {
        Self { buf: vec![0; 4096], path: PathBuf::with_capacity(256), open: 0, cap }
    }

    /// Reads `dir/name` through `slot`: one `pread` on a cached fd, otherwise open, read and keep
    /// the fd if its tier has room (hot files up to `cap`, others up to `cap − cap/8`).
    pub fn read(&mut self, s: &mut Slot, hot: bool, dir: &Path, name: &str) -> io::Result<&[u8]> {
        let res = match s {
            Slot::Missing => return Err(NotFound.into()),
            Slot::Open(f) => fill(&mut self.buf, f),
            Slot::Empty => {
                self.path.clear();
                self.path.push(dir);
                self.path.push(name);
                File::open(&self.path).and_then(|f| {
                    let n = fill(&mut self.buf, &f)?;
                    if self.open < if hot { self.cap } else { self.cap - self.cap / 8 } {
                        self.open += 1;
                        *s = Slot::Open(f);
                    }
                    Ok(n)
                })
            }
        };
        match res {
            Ok(n) => Ok(&self.buf[..n]),
            Err(e) => {
                self.release(std::slice::from_mut(s));
                let missing = matches!(e.kind(), NotFound | PermissionDenied)
                    || e.raw_os_error() == Some(ENODEV);
                *s = if missing { Slot::Missing } else { Slot::Empty };
                Err(e)
            }
        }
    }

    /// Closes an entity's fds the moment it goes, so a removed cgroup does not linger as dying.
    pub fn release(&mut self, slots: &mut [Slot]) {
        for s in slots {
            self.open -= usize::from(matches!(s, Slot::Open(_)));
            *s = Slot::Empty;
        }
    }

    /// Proves that `pread` at offset 0 regenerates procfs and kernfs content; otherwise caching
    /// is turned off (`cap = 0`) and the caller sends one `W` line (§5.2).
    pub fn self_check(&mut self, env: &Env) -> bool {
        let stat = env.proc_root.join("self/stat");
        let ok = self.regenerates(&env.proc_root.join("self/io"), "rchar", || {
            let mut b = [0; 4096];
            let _ = File::open(&stat).and_then(|f| f.read_at(&mut b, 0));
        }) && self.regenerates(&env.self_cg.join("cpu.stat"), "usage_usec", || {
            let t = Instant::now();
            while t.elapsed() < Duration::from_millis(2) {}
        });
        self.cap = if ok { self.cap } else { 0 };
        ok
    }

    /// Re-reads `key` through one fd after `between` until it grows (true) or 50 ms pass, as
    /// counters such as the root cgroup's `usage_usec` advance only per scheduler tick.
    fn regenerates(&mut self, path: &Path, key: &str, between: impl Fn()) -> bool {
        let Ok(f) = File::open(path) else { return false };
        let read = |buf: &mut Vec<u8>| fill(buf, &f).ok().and_then(|n| kv(&buf[..n], key));
        let (a, t) = (read(&mut self.buf), Instant::now());
        while a.is_some() && t.elapsed() < Duration::from_millis(50) {
            between();
            if read(&mut self.buf) > a {
                return true;
            }
        }
        false
    }
}

/// Reads a whole file at offset 0, doubling the buffer while a read fills it.
fn fill(buf: &mut Vec<u8>, f: &File) -> io::Result<usize> {
    loop {
        let n = f.read_at(buf, 0)?;
        if n < buf.len() {
            return Ok(n);
        }
        buf.resize(buf.len() * 2, 0);
    }
}

/// Leading decimal digits of `b` (`None` if there are none or they overflow).
pub fn num(b: &[u8]) -> Option<u64> {
    let end = b.iter().position(|c| !c.is_ascii_digit()).unwrap_or(b.len());
    let digits = b.get(..end).filter(|d| !d.is_empty())?;
    digits.iter().try_fold(0u64, |a, &c| a.checked_mul(10)?.checked_add(u64::from(c - b'0')))
}

/// The value of a `key value` or `key: value` line (`cpu.stat`, `memory.events`, `memory.stat`,
/// `/proc/<pid>/io`, `/proc/stat`).
pub fn kv(text: &[u8], key: &str) -> Option<u64> {
    text.split(|&b| b == b'\n').find_map(|line| {
        let rest = line.strip_prefix(key.as_bytes())?;
        let rest = rest.strip_prefix(b":").unwrap_or(rest);
        rest.first().filter(|&&b| b == b' ')?;
        num(rest.trim_ascii_start())
    })
}

/// The value of a `key=value` token on one line (`io.stat`, PSI files).
pub fn kv_eq(line: &[u8], key: &str) -> Option<u64> {
    line.split(|&b| b == b' ')
        .find_map(|tok| num(tok.strip_prefix(key.as_bytes())?.strip_prefix(b"=")?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(rel: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(rel)
    }

    #[test]
    fn parsers() {
        assert_eq!(num(b"123\n"), Some(123));
        assert_eq!(num(b"x"), None);
        assert_eq!(num(b"99999999999999999999"), None);
        let cpu = b"usage_usec 10\nuser_usec 7\nthrottled_usec 3\n";
        assert_eq!(kv(cpu, "usage_usec"), Some(10));
        assert_eq!(kv(cpu, "user"), None, "whole keys only");
        assert_eq!(kv(b"anon_thp 5\nanon 9\n", "anon"), Some(9));
        assert_eq!(kv(b"rchar: 77\n", "rchar"), Some(77));
        let psi = b"some avg10=0.00 avg60=0.12 avg300=0.04 total=1111";
        assert_eq!(kv_eq(psi, "total"), Some(1111));
        assert_eq!(kv_eq(psi, "tot"), None);
    }

    #[test]
    fn tiers() {
        let dir = fixture("cgroup/25.11/system.slice/slurmstepd.scope/job_42");
        let mut files = Files::new(8);
        let mut slots: Vec<Slot> = (0..10).map(|_| Slot::Empty).collect();
        for s in &mut slots[..7] {
            files.read(s, false, &dir, "memory.stat").unwrap();
        }
        assert_eq!(files.open, 7, "cold files fill cap − cap/8");
        assert!(files.read(&mut slots[7], false, &dir, "memory.stat").is_ok());
        assert!(matches!(slots[7], Slot::Empty), "cold file past cap − cap/8 stays uncached");
        files.read(&mut slots[8], true, &dir, "cpu.stat").unwrap();
        assert!(matches!(slots[8], Slot::Open(_)), "hot file still cached");
        let b = files.read(&mut slots[9], true, &dir, "cpu.stat").unwrap();
        assert_eq!(kv(b, "usage_usec"), Some(8_000_024), "uncached read still returns data");
        assert!(matches!(slots[9], Slot::Empty), "nothing cached at cap");
        assert_eq!(
            kv(files.read(&mut slots[8], true, &dir, "cpu.stat").unwrap(), "usage_usec"),
            Some(8_000_024)
        );
        files.release(&mut slots);
        assert_eq!(files.open, 0);
    }

    #[test]
    fn missing_files_are_remembered() {
        let dir = fixture("cgroup/no-controllers/system.slice/slurmstepd.scope/job_42");
        let mut files = Files::new(64);
        let mut slot = Slot::Empty;
        assert!(files.read(&mut slot, false, &dir, "io.stat").is_err());
        assert!(matches!(slot, Slot::Missing));
        assert_eq!(files.open, 0);
        let mut big = Files::new(64);
        big.buf.truncate(4);
        let mut s = Slot::Empty;
        let len = std::fs::metadata(dir.join("memory.stat")).unwrap().len();
        assert_eq!(
            big.read(&mut s, true, &dir, "memory.stat").unwrap().len() as u64,
            len,
            "buffer grows"
        );
    }

    #[test]
    fn self_check_failure_disables_caching() {
        let env = Env {
            proc_root: fixture("proc"),
            self_cg: fixture("cgroup/25.11"),
            page_size: 4096,
            clk_tck: 100,
        };
        let mut files = Files::new(1024);
        assert!(!files.self_check(&env), "static fixture files never regenerate");
        assert_eq!(files.cap, 0);
    }
}
