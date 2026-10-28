// SPDX-License-Identifier: LGPL-3.0-or-later
// SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany

//! The /proc source and the constants unjam reads from /proc instead of calling libc
//! (`sysconf`, `uname`, `getrlimit`; §4.3, §5.1).

use std::{fmt::Write as _, fs, io, path::Path, path::PathBuf};

use crate::model::{Ent, Kind, Metric, NA};
use crate::source::{ESRCH, Env, Files, Sampled, Slot, Source, Target, kv, num};

/// Shorthand for the descriptor table below.
const fn m(name: &'static str, unit: &'static str, kind: Kind) -> Metric {
    Metric { name, unit, kind }
}

/// Round-one process metrics (§4.3). Names are append-only.
pub const METRICS: &[Metric] = &[
    m("p_cpu_user_usec", "us", Kind::Counter),
    m("p_cpu_system_usec", "us", Kind::Counter),
    m("p_rss_bytes", "B", Kind::Gauge),
    m("p_threads", "1", Kind::Gauge),
    m("p_state", "char", Kind::Gauge),
    m("p_last_cpu", "1", Kind::Gauge),
    m("p_minflt", "1", Kind::Counter),
    m("p_majflt", "1", Kind::Counter),
    m("p_io_read_bytes", "B", Kind::Counter),
    m("p_io_write_bytes", "B", Kind::Counter),
];

/// The fields of `/proc/<pid>/stat` unjam uses, parsed once for both discovery and values.
#[derive(Debug, PartialEq, Eq)]
pub struct Stat<'a> {
    pub comm: &'a [u8],
    pub state: u8,
    pub ppid: u32,
    pub minflt: u64,
    pub majflt: u64,
    pub utime: u64,
    pub stime: u64,
    pub threads: u64,
    pub start: u64,
    pub rss: u64,
    pub cpu: u64,
}

/// Splits `stat` at the first `(` and the last `)`, which is safe for any `comm`.
pub fn stat(buf: &[u8]) -> Option<Stat<'_>> {
    let open = buf.iter().position(|&b| b == b'(')?;
    let close = buf.iter().rposition(|&b| b == b')')?;
    let mut fields = buf.get(close + 2..)?.split(|&b| b == b' ');
    let state = *fields.next()?.first()?;
    let mut f = [0u64; 36]; // fields 4 ..= 39
    for (slot, tok) in f.iter_mut().zip(fields) {
        *slot = num(tok).unwrap_or(0);
    }
    Some(Stat {
        comm: buf.get(open + 1..close)?,
        state,
        ppid: u32::try_from(f[0]).ok()?,
        minflt: f[6],
        majflt: f[8],
        utime: f[10],
        stime: f[11],
        threads: f[16],
        start: f[18],
        rss: f[20],
        cpu: f[35],
    })
}

/// The /proc source: stat and io of each sampled process.
pub struct Proc {
    root: PathBuf,
    page: u64,
    clk: u64,
    /// Reused `<pid>/<file>` name, so a cached read formats into existing capacity.
    name: String,
}

/// The source registry entry; /proc is always there.
#[allow(clippy::unnecessary_wraps, reason = "the Probe signature; other sources can fail")]
pub fn probe(env: &Env) -> Result<Box<dyn Source>, String> {
    let (root, page, clk) = (env.proc_root.clone(), env.page_size, env.clk_tck.max(1));
    Ok(Box::new(Proc { root, page, clk, name: String::with_capacity(32) }))
}

impl Source for Proc {
    /// Process metrics fill the process value vector.
    fn entity(&self) -> Ent {
        Ent::Proc
    }

    /// The round-one table.
    fn metrics(&self) -> &'static [Metric] {
        METRICS
    }

    /// Reads `stat` (hot) and `io` (cold, NA without ptrace access). A vanished process, or a
    /// reused pid whose `start` differs, is `Err` and leads to `x`.
    fn sample(
        &mut self,
        t: Target,
        slots: &mut [Slot],
        io: &mut Files,
        out: &mut [u64],
    ) -> Sampled {
        let Target::Proc { pid, start } = t else { return Ok(()) };
        self.name.clear();
        let _ = write!(self.name, "{pid}/stat");
        let s = stat(io.read(&mut slots[0], true, &self.root, &self.name)?)
            .filter(|s| s.start == start)
            .ok_or_else(|| io::Error::from_raw_os_error(ESRCH))?;
        let us = |ticks: u64| ticks.saturating_mul(1_000_000) / self.clk;
        let v = [us(s.utime), us(s.stime), s.rss.saturating_mul(self.page), s.threads];
        out[..4].copy_from_slice(&v);
        out[4..8].copy_from_slice(&[s.state.into(), s.cpu, s.minflt, s.majflt]);
        self.name.truncate(self.name.len() - 4);
        self.name.push_str("io");
        let io = io.read(&mut slots[1], false, &self.root, &self.name).ok();
        out[8] = io.and_then(|b| kv(b, "read_bytes")).unwrap_or(NA);
        out[9] = io.and_then(|b| kv(b, "write_bytes")).unwrap_or(NA);
        Ok(())
    }
}

/// What announces a new process in a `P` line: identity, parent and names.
#[derive(Debug, PartialEq, Eq)]
pub struct Ident {
    pub start: u64,
    pub ppid: u32,
    pub comm: Box<str>,
    pub cmd: Box<str>,
}

/// Reads a new process's identity through its `stat` slot (which stays cached for sampling;
/// `Proc` is the first process source, so it owns slot 0) and its `cmdline` once, uncached.
pub fn ident(root: &Path, io: &mut Files, stat_slot: &mut Slot, pid: u32) -> Option<Ident> {
    let s = stat(io.read(stat_slot, true, root, &format!("{pid}/stat")).ok()?)?;
    let (start, parent, comm) = (s.start, s.ppid, String::from_utf8_lossy(s.comm).into());
    let mut once = Slot::Empty;
    let cmd = io.read(&mut once, false, root, &format!("{pid}/cmdline")).map(cmdline);
    io.release(std::slice::from_mut(&mut once));
    Some(Ident { start, ppid: parent, comm, cmd: cmd.unwrap_or_default().into() })
}

/// `cmdline` for display: NUL separators become spaces, invalid UTF-8 is replaced.
pub fn cmdline(buf: &[u8]) -> String {
    let s: Vec<u8> = buf.iter().map(|&b| if b == 0 { b' ' } else { b }).collect();
    String::from_utf8_lossy(s.trim_ascii_end()).into_owned()
}

/// Page size and `CLK_TCK` from `/proc/self/auxv` (`AT_PAGESZ` = 6, `AT_CLKTCK` = 17), which
/// replaces `sysconf`; Linux defaults when an entry is absent.
pub fn auxv(buf: &[u8]) -> (u64, u64) {
    let (mut page, mut clk) = (4096, 100);
    for pair in buf.chunks_exact(16) {
        let word = |i: usize| pair[i..i + 8].try_into().map_or(0, u64::from_ne_bytes);
        match word(0) {
            6 => page = word(8),
            17 => clk = word(8),
            _ => {}
        }
    }
    (page, clk)
}

/// The soft `Max open files` limit from `/proc/self/limits`, which sizes the fd cache without
/// `getrlimit`; `unlimited` is `u64::MAX`.
pub fn nofile(text: &str) -> Option<u64> {
    let rest = text.lines().find_map(|l| l.strip_prefix("Max open files"))?;
    let soft = rest.split_whitespace().next()?;
    if soft == "unlimited" { Some(u64::MAX) } else { soft.parse().ok() }
}

/// Builds the sampling environment, reading the constants from `proc_root`.
pub fn env(proc_root: PathBuf, self_cg: PathBuf) -> Env {
    let (page_size, clk_tck) = auxv(&fs::read(proc_root.join("self/auxv")).unwrap_or_default());
    Env { proc_root, self_cg, page_size, clk_tck }
}

/// Host name for `H` (from `/proc/sys/kernel/hostname`, replacing `uname`) and boot time for
/// turning `start` into wall time (`/proc/stat` `btime`).
pub fn host_btime(proc_root: &Path) -> (String, u64) {
    let host = fs::read_to_string(proc_root.join("sys/kernel/hostname")).unwrap_or_default();
    let btime = fs::read(proc_root.join("stat")).ok().and_then(|b| kv(&b, "btime"));
    (host.trim().to_owned(), btime.unwrap_or(NA))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/proc")
    }

    fn read(rel: &str) -> Vec<u8> {
        fs::read(root().join(rel)).unwrap()
    }

    #[test]
    fn stat_survives_any_comm() {
        let buf = read("101/stat");
        let s = stat(&buf).unwrap();
        assert_eq!(s.comm, b"x) (y");
        assert_eq!((s.state, s.ppid, s.minflt, s.majflt), (b'R', 100, 1200, 3));
        assert_eq!(
            (s.utime, s.stime, s.threads, s.start, s.rss, s.cpu),
            (1000, 10, 4, 5001, 1280, 1)
        );
        assert_eq!(stat(&read("100/stat")).unwrap().comm, b"a b");
        assert_eq!(stat(&read("102/stat")).unwrap().state, b'Z');
        assert_eq!(stat(b"1 (x"), None);
    }

    #[test]
    fn samples_every_metric() {
        let env = env(root(), PathBuf::new());
        let mut src = probe(&env).unwrap();
        let mut slots: Vec<Slot> = (0..METRICS.len()).map(|_| Slot::Empty).collect();
        let mut files = Files::new(64);
        let mut out = vec![0; METRICS.len()];
        src.sample(Target::Proc { pid: 101, start: 5001 }, &mut slots, &mut files, &mut out)
            .unwrap();
        assert_eq!(
            out,
            [
                10_000_000,
                100_000,
                1280 * 4096,
                4,
                u64::from(b'R'),
                1,
                1200,
                3,
                101 * 4096,
                101 * 512
            ]
        );
        let reused =
            src.sample(Target::Proc { pid: 101, start: 1 }, &mut slots, &mut files, &mut out);
        assert_eq!(reused.unwrap_err().raw_os_error(), Some(ESRCH));
        let gone = src.sample(
            Target::Proc { pid: 7, start: 1 },
            &mut [Slot::Empty, Slot::Empty],
            &mut files,
            &mut out,
        );
        assert!(gone.is_err());
    }

    #[test]
    fn ident_and_cmdline() {
        let mut files = Files::new(64);
        let mut slot = Slot::Empty;
        let id = ident(&root(), &mut files, &mut slot, 101).unwrap();
        assert_eq!(&*id.cmd, "/bin/x) (y --flag with space");
        assert_eq!((id.start, id.ppid, &*id.comm), (5001, 100, "x) (y"));
        assert!(matches!(slot, Slot::Open(_)), "stat stays cached");
        assert_eq!(files.open, 1, "cmdline was read once and closed");
        assert_eq!(&*ident(&root(), &mut files, &mut Slot::Empty, 102).unwrap().cmd, "");
        assert_eq!(cmdline(b"a\xffb\0c\0"), "a\u{fffd}b c");
    }

    #[test]
    fn constants() {
        assert_eq!(auxv(&read("self/auxv")), (4096, 100));
        assert_eq!(auxv(&[]), (4096, 100));
        assert_eq!(nofile(&String::from_utf8(read("self/limits")).unwrap()), Some(1024));
        assert_eq!(
            nofile("Max open files            unlimited            unlimited  files"),
            Some(u64::MAX)
        );
        assert_eq!(nofile(""), None);
        assert_eq!(host_btime(&root()), ("worker-0".to_owned(), 1_761_552_000));
    }
}
