// SPDX-License-Identifier: LGPL-3.0-or-later
// SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany

//! The sampling state machine shared by local, agent and record modes, the run loop that local
//! and remote agents share, and the output path that never blocks the sampler (§3.4, §6, §7.5).

use std::collections::HashMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use std::{fs, mem, thread};

use crate::model::{Control, Ent, Event, NA, Rule, TaskSet, What, effective, next_due, upsert};
use crate::source::{self, Env, Files, Slot, Source, Target};
use crate::tui::{self, App, Msg};
use crate::{cgroup, procfs, wire};

/// Discovery walk period while anything is sampled: at most 1 Hz (§5.3).
const WALK_MS: u64 = 1000;
/// Structure rescan period while every subtree is `Off`.
pub const OFF_WALK_MS: u64 = 10_000;
/// Buffered structure beyond this applies the fork-storm self-rule (§7.5).
const PENDING_CAP: usize = 1 << 20;
/// Writer queue depth in buffers (§7.5).
const QUEUE: usize = 4;
/// Output budget until the TUI sends `B`: the 4 MiB/s total of a one-node job.
const DEFAULT_RATE: u64 = 4 << 20;

/// Receives every event the sampler produces: the wire encoder, or the record writer.
pub type Emit<'a> = &'a mut dyn FnMut(&Event);

/// Who a sampler is and what it watches.
pub struct Opts {
    pub node: u32,
    pub ntasks: u32,
    pub step: String,
    pub root: PathBuf,
    pub exclude: Option<PathBuf>,
    pub cap: usize,
}

/// A known cgroup: path for rules and display, effective rule, fd slots and last pid list.
struct CgEnt {
    dir: PathBuf,
    rel: Box<str>,
    parent: u64,
    seen: u32,
    interval: u32,
    what: What,
    slots: Box<[Slot]>,
    procs: Slot,
    pids: Vec<u32>,
}

/// A known process: identity for reuse detection, `P` fields for resyncs, and fd slots.
struct ProcEnt {
    start: u64,
    cg: u64,
    ppid: u32,
    comm: Box<str>,
    cmd: Box<str>,
    slots: Box<[Slot]>,
}

/// Sources, fd cache, processes and scratch buffers: what per-cgroup work borrows while the
/// cgroup map is iterated.
struct Core {
    env: Env,
    sources: Vec<Box<dyn Source>>,
    width: [usize; 2],
    files: Files,
    procs: HashMap<u32, ProcEnt>,
    vals: Vec<u64>,
    pids: Vec<u32>,
}

impl Core {
    /// Runs the sources of `ent` over one entity into `vals`; false if the entity is gone.
    fn run(&mut self, ent: Ent, t: Target, slots: &mut [Slot]) -> bool {
        self.vals.clear();
        self.vals.resize(self.width[ent as usize], NA);
        let mut off = 0;
        for s in self.sources.iter_mut().filter(|s| s.entity() == ent) {
            let r = off..off + s.metrics().len();
            let (io, out) = (&mut self.files, &mut self.vals[r.clone()]);
            if s.sample(t, &mut slots[r.clone()], io, out).is_err() {
                return false;
            }
            off = r.end;
        }
        true
    }

    /// Samples one due cgroup and, at level `p`, diffs its `cgroup.procs` and samples its
    /// processes.
    fn cgroup(&mut self, id: u64, cg: &mut CgEnt, emit: Emit) {
        if self.run(Ent::Cgroup, Target::Cgroup { dir: &cg.dir }, &mut cg.slots) {
            emit(&Event::CgVals { id, first: 0, v: &self.vals });
        }
        if cg.what < What::Procs {
            return;
        }
        let Ok(b) = self.files.read(&mut cg.procs, true, &cg.dir, "cgroup.procs") else { return };
        cgroup::procs(b, &mut self.pids);
        for &pid in &cg.pids {
            if self.pids.binary_search(&pid).is_err()
                && self.procs.get(&pid).is_some_and(|p| p.cg == id)
            {
                self.drop_proc(pid, emit);
            }
        }
        for i in 0..self.pids.len() {
            let pid = self.pids[i];
            if cg.pids.binary_search(&pid).is_err() {
                self.new_proc(pid, id, emit);
            }
            self.sample_proc(pid, id, emit);
        }
        cg.pids.clear();
        cg.pids.extend_from_slice(&self.pids);
    }

    /// Samples one process. A vanished process gives `x`; a reused pid gives `x` then `P`; an
    /// exec (new `comm`) gives `P` again with the new names.
    fn sample_proc(&mut self, pid: u32, cg: u64, emit: Emit) {
        let Some(p) = self.procs.get_mut(&pid).filter(|p| p.cg == cg) else { return };
        let (start, mut slots, comm) = (p.start, mem::take(&mut p.slots), mem::take(&mut p.comm));
        let ok = self.run(Ent::Proc, Target::Proc { pid, start, comm: &comm }, &mut slots);
        if let Some(p) = self.procs.get_mut(&pid) {
            (p.slots, p.comm) = (slots, comm);
        }
        if ok {
            emit(&Event::ProcVals { pid, first: 0, v: &self.vals });
        } else {
            self.new_proc(pid, cg, emit);
        }
    }

    /// Announces a process new to cgroup `cg`, or again after an exec. `x` comes first when the
    /// pid was known with another start (reuse) or cgroup (move), or has vanished.
    fn new_proc(&mut self, pid: u32, cg: u64, emit: Emit) {
        let old = self.procs.get(&pid).map(|p| (p.start, p.cg));
        self.drop_proc(pid, &mut |_| ()); // forgotten silently: whether `x` is due is known below
        let (root, files) = (&self.env.proc_root, &mut self.files);
        let mut slots: Box<[Slot]> = (0..self.width[1]).map(|_| Slot::Empty).collect();
        let id = slots.first_mut().and_then(|s| procfs::ident(root, files, s, pid));
        if old.is_some() && old != id.as_ref().map(|i| (i.start, cg)) {
            emit(&Event::ProcGone(pid));
        }
        let Some(procfs::Ident { start, ppid: up, comm, cmd }) = id else {
            return self.files.release(&mut slots);
        };
        emit(&Event::Proc { pid, start, cg, ppid: up, comm: &comm, cmd: &cmd });
        self.procs.insert(pid, ProcEnt { start, cg, ppid: up, comm, cmd, slots });
    }

    /// Forgets a process: closes its fds at once and emits `x`.
    fn drop_proc(&mut self, pid: u32, emit: Emit) {
        if let Some(mut p) = self.procs.remove(&pid) {
            self.files.release(&mut p.slots);
            emit(&Event::ProcGone(pid));
        }
    }

    /// Forgets the processes of a cgroup that went away or below level `p`.
    fn drop_procs(&mut self, id: u64, cg: &mut CgEnt, emit: Emit) {
        for &pid in &cg.pids {
            if self.procs.get(&pid).is_some_and(|p| p.cg == id) {
                self.drop_proc(pid, emit);
            }
        }
        cg.pids.clear();
        self.files.release(std::slice::from_mut(&mut cg.procs));
    }
}

/// The sampling state machine (rules, timers, discovery), used by local, agent and record modes.
pub struct Sampler {
    core: Core,
    opts: Opts,
    /// Host name and boot time for `H`.
    hello: (String, u64),
    rules: Vec<Rule>,
    cgs: HashMap<u64, CgEnt>,
    timers: Vec<(u32, u64)>,
    due: Vec<u32>,
    walked: Option<u64>,
    seen: u32,
    me: Slot,
    warns: Vec<String>,
}

impl Sampler {
    /// Probes the sources, sizes the fd cache from the soft fd limit and runs the self-check.
    pub fn new(env: Env, opts: Opts) -> Self {
        let (sources, mut warns) = source::sources(&env);
        let mut width = [0; 2];
        for s in &sources {
            width[s.entity() as usize] += s.metrics().len();
        }
        let limits = fs::read_to_string(env.proc_root.join("self/limits")).unwrap_or_default();
        let soft = procfs::nofile(&limits).unwrap_or(1024).saturating_sub(64);
        let mut files = Files::new(usize::try_from(soft).unwrap_or(usize::MAX).min(opts.cap));
        if !files.self_check(&env) {
            warns.push("pread at offset 0 returns stale data: fd caching off".into());
        }
        let hello = procfs::host_btime(&env.proc_root);
        let (procs, vals, pids) = (HashMap::new(), Vec::new(), Vec::new());
        let core = Core { env, sources, width, files, procs, vals, pids };
        let (rules, cgs, timers, due, walked) = (vec![], HashMap::new(), vec![], vec![], None);
        Self { core, opts, hello, rules, cgs, timers, due, walked, seen: 0, me: Slot::Empty, warns }
    }

    /// Announces everything (`H`, `D`, pending warnings, every `G` and `P`): at start and on `S`.
    pub fn snapshot(&mut self, now: u64, emit: Emit) {
        let ((host, btime), step, ntasks) = (&self.hello, &self.opts.step, self.opts.ntasks);
        let (proto, version, btime) = (wire::PROTO, env!("CARGO_PKG_VERSION"), *btime);
        emit(&Event::Hello { proto, version, host, step, ntasks, btime });
        for ent in [Ent::Cgroup, Ent::Proc] {
            let srcs = self.core.sources.iter().filter(|s| s.entity() == ent);
            for (idx, m) in (0..).zip(srcs.flat_map(|s| s.metrics())) {
                emit(&Event::Desc { ent, idx, name: m.name, unit: m.unit, kind: m.kind });
            }
        }
        for w in self.warns.drain(..) {
            emit(&Event::Warn(&w));
        }
        if self.walked.is_none() {
            return self.walk(now, emit);
        }
        let mut cgs: Vec<_> = self.cgs.iter().map(|(&id, c)| (&*c.rel, id, c.parent)).collect();
        cgs.sort_unstable();
        for (path, id, parent) in cgs {
            emit(&Event::Cg { id, parent, path });
        }
        for (&pid, p) in &self.core.procs {
            let (start, cg, ppid, comm, cmd) = (p.start, p.cg, p.ppid, &*p.comm, &*p.cmd);
            emit(&Event::Proc { pid, start, cg, ppid, comm, cmd });
        }
    }

    /// Discovery: `X` for vanished cgroups first, then `G` for new ones, parents first (§4.2).
    /// Absent files are retried on the first walk of each `OFF_WALK_MS` period, not every walk.
    fn walk(&mut self, now: u64, emit: Emit) {
        let retry = self.walked.is_none_or(|w| w / OFF_WALK_MS != now / OFF_WALK_MS);
        self.walked = Some(now);
        self.seen = self.seen.wrapping_add(1);
        let found = cgroup::walk(&self.opts.root, self.opts.exclude.as_deref());
        for (_, ino, _) in &found {
            let Some(c) = self.cgs.get_mut(ino) else { continue };
            c.seen = self.seen;
            for s in c.slots.iter_mut().filter(|s| retry && matches!(s, Slot::Missing)) {
                *s = Slot::Empty;
            }
        }
        let seen = self.seen;
        for (id, mut c) in self.cgs.extract_if(|_, c| c.seen != seen) {
            self.core.drop_procs(id, &mut c, emit);
            self.core.files.release(&mut c.slots);
            emit(&Event::CgGone(id));
        }
        for (dir, ino, parent) in found {
            if self.cgs.contains_key(&ino) {
                continue;
            }
            let rel: Box<str> =
                dir.strip_prefix(&self.opts.root).unwrap_or(&dir).to_string_lossy().into();
            let (interval, what) = effective(&self.rules, self.opts.node, &rel);
            emit(&Event::Cg { id: ino, parent, path: &rel });
            let slots = (0..self.core.width[0]).map(|_| Slot::Empty).collect();
            let (seen, procs, pids) = (self.seen, Slot::Empty, Vec::new());
            let c = CgEnt { dir, rel, parent, seen, interval, what, slots, procs, pids };
            self.cgs.insert(ino, c);
        }
        self.retime(now);
    }

    /// One timer per distinct interval of the sampled cgroups; surviving timers keep their phase.
    fn retime(&mut self, now: u64) {
        let mut ivs: Vec<u32> =
            self.cgs.values().filter(|c| c.what > What::Off).map(|c| c.interval).collect();
        ivs.sort_unstable();
        ivs.dedup();
        self.timers.retain(|t| ivs.contains(&t.0));
        for iv in ivs {
            if !self.timers.iter().any(|t| t.0 == iv) {
                self.timers.push((iv, next_due(now, iv)));
            }
        }
    }

    /// Re-resolves every cgroup's rule and drops processes where level `p` ended.
    fn apply_rules(&mut self, now: u64, emit: Emit) {
        for (&id, c) in &mut self.cgs {
            (c.interval, c.what) = effective(&self.rules, self.opts.node, &c.rel);
            if c.what < What::Procs {
                self.core.drop_procs(id, c, emit);
            }
        }
        self.retime(now);
    }

    /// Applies `R`, `C` and `S` addressed to this agent; `B` and `K` belong to the run loop.
    pub fn control(&mut self, c: Control, now: u64, emit: Emit) {
        if !c.applies(self.opts.node) {
            return;
        }
        match c {
            Control::Rule(r) => upsert(&mut self.rules, r),
            Control::Clear(_) => self.rules.clear(),
            Control::Resync(_) => return self.snapshot(now, emit),
            Control::Budget(..) | Control::Keepalive => return,
        }
        self.apply_rules(now, emit);
    }

    /// Fork-storm fallback: processes off everywhere at the longest active interval (§7.5).
    pub fn storm(&mut self, now: u64, emit: Emit) {
        let interval_ms = self.timers.iter().map(|t| t.0).max().unwrap_or(1000);
        let rule = Rule { tasks: TaskSet::All, pattern: ".".into(), interval_ms, what: What::Cg };
        emit(&Event::Warn(&format!("output backlog over 1 MiB: rules now R * . {interval_ms} c")));
        self.rules = vec![rule];
        self.apply_rules(now, emit);
    }

    /// One tick: fires due timers, walks if due, samples; returns own CPU µs and RSS for `T`.
    pub fn tick(&mut self, now: u64, emit: Emit) -> (u64, u64) {
        self.due.clear();
        for (iv, due) in self.timers.iter_mut().filter(|t| t.1 <= now) {
            self.due.push(*iv);
            *due = next_due(now, *iv);
        }
        let since = now.saturating_sub(self.walked.unwrap_or(0));
        if since >= OFF_WALK_MS || (!self.due.is_empty() && since >= WALK_MS) {
            self.walk(now, emit);
        }
        self.sample(emit);
        if let Some(w) = self.core.files.denied.as_mut().filter(|w| !w.is_empty()) {
            emit(&Event::Warn(w));
            w.clear();
        }
        let Core { files, env, .. } = &mut self.core;
        let s =
            files.read(&mut self.me, true, &env.proc_root, "self/stat").ok().and_then(procfs::stat);
        let us =
            |s: &procfs::Stat| (s.utime + s.stime).saturating_mul(1_000_000) / env.clk_tck.max(1);
        s.map_or((NA, NA), |s| (us(&s), s.rss.saturating_mul(env.page_size)))
    }

    /// Samples the due cgroups and their processes: the allocation-free steady state.
    fn sample(&mut self, emit: Emit) {
        for (&id, c) in &mut self.cgs {
            if c.what > What::Off && self.due.contains(&c.interval) {
                self.core.cgroup(id, c, emit);
            }
        }
    }

    /// The earliest deadline: a timer, or the structure rescan. A deadline more than one period
    /// ahead of `now` means the wall clock stepped back; it is re-phased to `now`.
    pub fn next_due(&mut self, now: u64) -> u64 {
        for t in self.timers.iter_mut().filter(|t| t.1 > now + u64::from(t.0)) {
            t.1 = next_due(now, t.0);
        }
        self.walked = self.walked.map(|w| w.min(now));
        let walk = self.walked.map_or(0, |t| t + OFF_WALK_MS);
        self.timers.iter().map(|t| t.1).fold(walk, u64::min)
    }

    /// The shortest active interval, which bounds the emission stagger.
    fn interval(&self) -> u32 {
        self.timers.iter().map(|t| t.0).min().unwrap_or(1000)
    }
}

/// Emission delay that spreads phase-aligned ticks of many agents over time:
/// `procid × min(interval/2, 250 ms) / ntasks` (§7.5).
pub fn stagger(node: u32, ntasks: u32, interval: u32) -> u64 {
    u64::from(node) * u64::from((interval / 2).min(250)) / u64::from(ntasks.max(1))
}

/// Writer-thread handle with the token bucket, pending structure and stagger: the backpressure
/// point. The sampler only ever calls `try_send`.
pub struct Out {
    node: u32,
    ntasks: u32,
    tx: SyncSender<(Vec<u8>, u64)>,
    back: Receiver<Vec<u8>>,
    /// The buffers not in the writer's hands; all `QUEUE + 1` are made at spawn.
    spare: Vec<Vec<u8>>,
    pending: Vec<u8>,
    vals: Vec<u8>,
    rate: u64,
    tokens: u64,
    refilled: u64,
    dropped: u32,
    /// Bytes handed to the writer so far, reported in `T`.
    sent: u64,
}

impl Out {
    /// Starts the writer thread: it sleeps the stagger, writes a buffer and hands it back for
    /// reuse. A write error (`EPIPE`) ends the thread, which the next send reports.
    pub fn spawn<W: Write + Send + 'static>(mut w: W, node: u32, ntasks: u32) -> Self {
        let (tx, rx) = mpsc::sync_channel::<(Vec<u8>, u64)>(QUEUE);
        let (back_tx, back) = mpsc::sync_channel(QUEUE + 1);
        thread::spawn(move || {
            for (buf, delay) in rx {
                thread::sleep(Duration::from_millis(delay));
                if w.write_all(&buf).and_then(|()| w.flush()).is_err() {
                    return;
                }
                let _ = back_tx.try_send(buf);
            }
        });
        let (spare, pending, vals) = ((0..=QUEUE).map(|_| Vec::new()).collect(), vec![], vec![]);
        let (rate, tokens, refilled, dropped, sent) = (DEFAULT_RATE, 2 * DEFAULT_RATE, 0, 0, 0);
        Self { node, ntasks, tx, back, spare, pending, vals, rate, tokens, refilled, dropped, sent }
    }

    /// Encodes an event into the tick's values or into the never-dropped structure.
    pub fn push(&mut self, ev: &Event) {
        wire::write(if ev.is_value() { &mut self.vals } else { &mut self.pending }, self.node, ev);
    }

    /// Ends a tick (`me` = own CPU and RSS adds `T`) or a control reply: queues the structure,
    /// plus the values if the budget allows, without blocking. `Err` once the writer is gone.
    pub fn send(&mut self, now: u64, interval: u32, me: Option<(u64, u64)>) -> io::Result<()> {
        if let Some((self_cpu_us, self_rss)) = me {
            let (out_bytes, dropped) = (self.sent, self.dropped);
            let t = Event::Tick { ts_ms: now, self_cpu_us, out_bytes, dropped, self_rss };
            wire::write(&mut self.pending, self.node, &t);
        }
        let refill = self.rate.saturating_mul(now.saturating_sub(self.refilled)) / 1000;
        self.tokens = self.tokens.saturating_add(refill).min(self.rate.saturating_mul(2));
        self.refilled = now;
        self.spare.extend(self.back.try_iter());
        let vals = !self.vals.is_empty() && self.tokens >= self.vals.len() as u64;
        let buf = if vals || !self.pending.is_empty() { self.spare.pop() } else { None };
        let Some(mut buf) = buf else {
            self.dropped += u32::from(!self.vals.is_empty());
            self.vals.clear();
            return Ok(());
        };
        self.dropped += u32::from(!vals && !self.vals.is_empty());
        buf.clear();
        buf.extend_from_slice(&self.pending);
        if vals {
            buf.extend_from_slice(&self.vals);
        }
        let (len, delay) = (buf.len() as u64, stagger(self.node, self.ntasks, interval));
        match self.tx.try_send((buf, delay)) {
            Ok(()) => {
                self.pending.clear();
                self.sent += len;
                self.tokens = self.tokens.saturating_sub(len);
            }
            Err(TrySendError::Full((b, _))) => {
                self.spare.push(b);
                self.dropped += u32::from(vals);
            }
            Err(TrySendError::Disconnected(_)) => return Err(io::ErrorKind::BrokenPipe.into()),
        }
        self.vals.clear();
        Ok(())
    }
}

/// Wall-clock milliseconds: ticks align to the wall clock, so all nodes sample together.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis().try_into().unwrap_or(0))
}

/// The loop every sampler runs, remote (stdin/stdout) or local (channels): control lines first,
/// then due ticks. Returns on control EOF, when the writer is gone (`EPIPE`) and after
/// `keep` milliseconds without input, timed on the monotonic clock so wall-clock steps do not.
pub fn run(ctl: &Receiver<String>, out: &mut Out, s: &mut Sampler, keep: u64) -> io::Result<()> {
    let (mut heard, keep, now) = (Instant::now(), Duration::from_millis(keep), now_ms());
    s.snapshot(now, &mut |e| out.push(e));
    out.send(now, s.interval(), None)?;
    loop {
        let now = now_ms();
        let wait = Duration::from_millis(s.next_due(now).saturating_sub(now));
        match ctl.recv_timeout(wait.min(keep.saturating_sub(heard.elapsed()))) {
            Ok(line) => {
                let now = now_ms();
                heard = Instant::now();
                match wire::parse_control(&line) {
                    Ok(Control::Budget(t, b)) if t.contains(s.opts.node) => out.rate = b,
                    Ok(c) => s.control(c, now, &mut |e| out.push(e)),
                    Err(e) => out.push(&Event::Warn(&e)),
                }
                out.send(now, s.interval(), None)?;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
        if heard.elapsed() >= keep {
            return Ok(());
        }
        let now = now_ms();
        if now >= s.next_due(now) {
            let me = s.tick(now, &mut |e| out.push(e));
            if out.pending.len() > PENDING_CAP {
                s.storm(now, &mut |e| out.push(e));
            }
            out.send(now, s.interval(), Some(me))?;
        }
    }
}

/// The sampled root, environment and identity. Inside a Slurm step (`job`) the root is the job
/// and the own step is excluded (§3.3, §5.5); otherwise it is `--root` or the whole cgroup2
/// mount, sampled as node 0 of 1.
pub fn setup(root: Option<PathBuf>, job: bool, cap: usize) -> Result<(Env, Opts), String> {
    let (mount, me) = cgroup::self_cgroup()?;
    let var = |k: &str| std::env::var(k).ok().filter(|_| job);
    let explicit = root.is_some();
    let (root, exclude) = match (root, var("SLURM_STEP_ID")) {
        (Some(r), _) => (r, None),
        (None, Some(_)) => cgroup::job_root(mount, &me).map(|(r, own)| (r, Some(own)))?,
        (None, None) => (mount.to_path_buf(), None),
    };
    let env = procfs::env("/proc".into(), cgroup::v2_check(mount, &root, &me, explicit)?);
    let num = |k, default| var(k).and_then(|v| v.parse().ok()).unwrap_or(default);
    let step = step_id(&var);
    let (node, ntasks) = (num("SLURM_PROCID", 0), num("SLURM_NTASKS", 1));
    Ok((env, Opts { node, ntasks, step, root, exclude, cap }))
}

/// Removes `exe` if it is an `srun --bcast` copy (`unjam.…`, D36). A split from `main` for its
/// test: a copy already gone, for example unlinked by another agent in a shared DIR, counts as
/// removed instead of stopping the agent.
fn unlink_copy(exe: &Path) -> Result<(), String> {
    let copy = exe.file_name().is_some_and(|n| n.as_encoded_bytes().starts_with(b"unjam."));
    let gone = |e: io::Error| (e.kind() == io::ErrorKind::NotFound).then_some(()).ok_or(e);
    let res = if copy { fs::remove_file(exe).or_else(gone) } else { Ok(()) };
    res.map_err(|e| format!("--unlink-self {}: {e}", exe.display()))
}

/// `JOB.STEP` from the Slurm variables `var` yields, or `-`: the `H` line's step field, which
/// the recorder writes too.
pub fn step_id(var: &dyn Fn(&str) -> Option<String>) -> String {
    var("SLURM_JOB_ID").zip(var("SLURM_STEP_ID")).map_or("-".into(), |(j, s)| format!("{j}.{s}"))
}

/// `unjam agent`: one sampler on this node, stdin as control, stdout as output. `unlink`
/// removes an `srun --bcast` copy of the own binary at start.
pub fn main(root: Option<PathBuf>, keepalive: u64, unlink: bool) -> Result<(), String> {
    std::env::current_exe().ok().filter(|_| unlink).map_or(Ok(()), |e| unlink_copy(&e))?;
    let (env, opts) = setup(root, true, 1024)?;
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || io::stdin().lines().map_while(Result::ok).try_for_each(|l| tx.send(l)));
    let mut out = Out::spawn(io::stdout(), opts.node, opts.ntasks);
    let mut s = Sampler::new(env, opts);
    match run(&rx, &mut out, &mut s, keepalive.saturating_mul(1000)) {
        Err(e) if e.kind() != io::ErrorKind::BrokenPipe => Err(format!("agent: {e}")),
        _ => Ok(()),
    }
}

/// The local sampler's stdout: each written buffer (whole lines) becomes one `Msg::Lines`.
struct Pipe(SyncSender<Msg>);

impl Write for Pipe {
    /// Forwards the buffer; fails like a closed pipe once the viewer is gone.
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.0.send(Msg::Lines(b.to_vec())).map_err(|_| io::ErrorKind::BrokenPipe)?;
        Ok(b.len())
    }

    /// Nothing is buffered.
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Local mode (§3.4): this module's sampler and loop on a thread, feeding the viewer the same
/// text lines a remote agent sends.
pub fn local(opts: tui::Opts, root: Option<PathBuf>, batch: Option<u32>) -> Result<(), String> {
    let (env, sopts) = setup(root, false, 2048)?;
    let ((tx, rx), (ctl, ctl_rx)) = (mpsc::sync_channel(64), mpsc::channel());
    let (out, err) = (Pipe(tx.clone()), tx.clone());
    thread::spawn(move || {
        let (mut out, mut s) = (Out::spawn(out, 0, 1), Sampler::new(env, sopts));
        if let Err(e) = run(&ctl_rx, &mut out, &mut s, u64::MAX) {
            let _ = err.send(Msg::Stderr(format!("sampler: {e}")));
        }
    });
    let mut app = App::new(opts, ctl);
    app.start(1);
    tui::view(&mut app, &tx, &rx, batch, &mut io::stdout()).map_err(|e| format!("tui: {e}"))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::state::State;

    /// A private copy of the fixture trees that a test may modify.
    struct Tree(PathBuf);

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn copy(from: &Path, to: &Path) {
        fs::create_dir_all(to).unwrap();
        for e in fs::read_dir(from).unwrap().map(Result::unwrap) {
            if e.file_type().unwrap().is_dir() {
                copy(&e.path(), &to.join(e.file_name()));
            } else {
                fs::copy(e.path(), to.join(e.file_name())).unwrap();
            }
        }
    }

    /// A sampler over a copy of the `variant` fixture rooted at its job directory; the fd cache
    /// is enabled although the self-check fails on static fixture files.
    fn sampler(variant: &str, test: &str) -> (Sampler, Tree) {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let tree = Tree(std::env::temp_dir().join(format!("unjam-{test}-{}", std::process::id())));
        let _ = fs::remove_dir_all(&tree.0);
        copy(&fixtures.join("proc"), &tree.0.join("proc"));
        copy(&fixtures.join("cgroup").join(variant), &tree.0.join("cg"));
        let job = fs::read_dir(tree.0.join("cg/system.slice/slurmstepd.scope"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| !p.ends_with("system"))
            .unwrap();
        let env = procfs::env(tree.0.join("proc"), job.clone());
        let opts =
            Opts { node: 0, ntasks: 1, step: "42.1".into(), root: job, exclude: None, cap: 1024 };
        let mut s = Sampler::new(env, opts);
        s.core.files.cap = 1024;
        (s, tree)
    }

    /// Runs `f` with an emitter that collects wire lines, and returns them.
    fn lines(f: impl FnOnce(Emit)) -> Vec<String> {
        let mut buf = Vec::new();
        f(&mut |e| wire::write(&mut buf, 0, e));
        String::from_utf8(buf).unwrap().lines().map(|l| l[2..].to_owned()).collect()
    }

    fn count(lines: &[String], tag: &str) -> usize {
        lines.iter().filter(|l| l.split('\t').next() == Some(tag)).count()
    }

    fn rule(s: &mut Sampler, text: &str, now: u64) -> Vec<String> {
        let c = wire::parse_control(&format!("R\t{text}")).unwrap();
        lines(|e| s.control(c, now, e))
    }

    #[test]
    fn snapshot_then_rules() {
        let (mut s, _t) = sampler("25.11", "snapshot");
        let snap = lines(|e| s.snapshot(0, e));
        assert!(snap[0].starts_with("H\t1\t"), "{}", snap[0]);
        assert_eq!(count(&snap, "D"), cgroup::METRICS.len() + procfs::METRICS.len());
        assert_eq!(count(&snap, "W"), 1, "the self-check fails on fixtures");
        assert_eq!(count(&snap, "G"), 8);
        assert!(snap.iter().any(|l| l.ends_with("\tstep_0/user/task_1")));
        assert_eq!(
            lines(|e| {
                s.tick(10_000, e);
            })
            .len(),
            0,
            "Off: structure only"
        );

        rule(&mut s, "*\t.\t1000\tp", 10_000);
        let tick = lines(|e| {
            s.tick(11_000, e);
        });
        assert_eq!(count(&tick, "g"), 8);
        assert_eq!(count(&tick, "P"), 3);
        assert_eq!(count(&tick, "p"), 3);
        assert!(tick.iter().any(|l| l.starts_with("P\t101\t5001\t")
            && l.ends_with("\tx) (y\t/bin/x) (y --flag with space")));
        let again = lines(|e| {
            s.tick(12_000, e);
        });
        assert_eq!((count(&again, "P"), count(&again, "p"), count(&again, "g")), (0, 3, 8));

        let resync = lines(|e| s.control(Control::Resync(TaskSet::All), 12_100, e));
        assert_eq!((count(&resync, "H"), count(&resync, "G"), count(&resync, "P")), (1, 8, 3));
        let cleared = lines(|e| s.control(Control::Clear(TaskSet::All), 12_200, e));
        assert_eq!(count(&cleared, "x"), 3, "processes go with level p");
        assert!(s.timers.is_empty());
        assert_eq!(
            lines(|e| {
                s.tick(13_000, e);
            })
            .len(),
            0
        );
    }

    #[test]
    fn timers_group_by_interval() {
        let (mut s, _t) = sampler("26.05-sluid", "timers");
        lines(|e| s.snapshot(0, e));
        rule(&mut s, "*\t.\t1000\tc", 0);
        rule(&mut s, "*\tstep_0/user\t500\tp", 0);
        assert_eq!(s.timers.len(), 2);
        assert_eq!(s.next_due(0), 500);
        let half = lines(|e| {
            s.tick(500, e);
        });
        assert_eq!(
            (count(&half, "g"), count(&half, "p")),
            (3, 3),
            "only step_0/user and its tasks"
        );
        let full = lines(|e| {
            s.tick(1000, e);
        });
        assert_eq!(count(&full, "g"), 8);
        assert_eq!(s.next_due(1000), 1500);
        assert_eq!(s.next_due(400), 500, "a backward clock step re-phases the timers");
        assert_eq!(s.walked, Some(400), "and the walk time, so the walk is not held off");
        assert_eq!(count(&lines(|e| _ = s.tick(500, e)), "g"), 3);
        rule(&mut s, "1\t.\t200\tp", 1000);
        assert_eq!(s.timers.len(), 2, "rules for other agents change nothing");
    }

    #[test]
    fn pid_reuse_and_cgroup_recreation() {
        let (mut s, t) = sampler("25.11", "reuse");
        lines(|e| s.snapshot(0, e));
        rule(&mut s, "*\t.\t1000\tp", 0);
        lines(|e| {
            s.tick(1000, e);
        });
        let stat = t.0.join("proc/101/stat");
        let text = fs::read_to_string(&stat).unwrap().replace(" 5001 ", " 6001 ");
        fs::write(&stat, text).unwrap();
        let reused = lines(|e| {
            s.tick(2000, e);
        });
        let x = reused.iter().position(|l| l == "x\t101").unwrap();
        let p = reused.iter().position(|l| l.starts_with("P\t101\t6001\t")).unwrap();
        assert!(x < p, "{reused:?}");

        let task1 = s.opts.root.join("step_0/user/task_1");
        let saved = t.0.join("task_1");
        fs::rename(&task1, &saved).unwrap();
        copy(&saved, &task1);
        let old = s.cgs.iter().find(|c| c.1.dir == task1).map(|c| *c.0).unwrap();
        let recreated = lines(|e| {
            s.tick(3000, e);
        });
        let gone = recreated.iter().position(|l| *l == format!("X\t{old}")).unwrap();
        let new =
            recreated.iter().position(|l| l.starts_with("G\t") && l.ends_with("/task_1")).unwrap();
        assert!(gone < new, "{recreated:?}");
        assert!(recreated.contains(&"x\t102".to_owned()), "its process goes with it");
        assert_eq!(count(&recreated, "P"), 1, "and comes back in the new cgroup");
    }

    #[test]
    fn exec_reannounces_names() {
        let (mut s, t) = sampler("25.11", "exec");
        lines(|e| s.snapshot(0, e));
        rule(&mut s, "*\t.\t1000\tp", 0);
        lines(|e| _ = s.tick(1000, e));
        let stat = t.0.join("proc/101/stat");
        fs::write(&stat, fs::read_to_string(&stat).unwrap().replace("(x) (y)", "(app)")).unwrap();
        fs::write(t.0.join("proc/101/cmdline"), "./app\0-n\0").unwrap();
        let exec = lines(|e| _ = s.tick(2000, e));
        let p: Vec<_> = exec.iter().filter(|l| l.starts_with("P\t101\t5001\t")).collect();
        assert!(p.len() == 1 && p[0].ends_with("\tapp\t./app -n"), "{exec:?}");
        assert!(!exec.contains(&"x\t101".to_owned()), "same process: values are kept");
        let after = lines(|e| _ = s.tick(3000, e));
        assert_eq!((count(&after, "P"), count(&after, "p")), (0, 3));
    }

    #[test]
    fn unreadable_proc_io_warns_once() {
        let (mut s, t) = sampler("25.11", "denied");
        lines(|e| s.snapshot(0, e));
        let io = t.0.join("proc/101/io");
        fs::remove_file(&io).unwrap();
        // A write-only sysctl: reading it is denied even to root, like another user's io.
        std::os::unix::fs::symlink("/proc/sys/vm/drop_caches", &io).unwrap();
        rule(&mut s, "*\t.\t1000\tp", 0);
        let first = lines(|e| _ = s.tick(1000, e));
        let w: Vec<_> = first.iter().filter(|l| l.starts_with("W\t")).collect();
        assert!(w.len() == 1 && w[0].contains("101/io: permission denied"), "{first:?}");
        let p101 = first.iter().find(|l| l.starts_with("p\t101\t")).unwrap();
        assert!(p101.ends_with("\t-\t-"), "{p101}");
        assert_eq!(count(&lines(|e| _ = s.tick(2000, e)), "W"), 0, "one W per agent");
    }

    #[test]
    fn missing_controllers_are_na() {
        let (mut s, _t) = sampler("no-controllers", "nocontrollers");
        lines(|e| s.snapshot(0, e));
        rule(&mut s, "*\tstep_0/user/task_1\t1000\tc", 0);
        let tick = lines(|e| {
            s.tick(1000, e);
        });
        let g: Vec<&str> = tick[0].split('\t').collect();
        assert_eq!(g[0], "g");
        let na: Vec<usize> =
            g[3..].iter().enumerate().filter(|v| *v.1 == "-").map(|v| v.0).collect();
        assert_eq!(na, [5, 9, 10, 11, 12, 13, 14]);
        let missing = |s: &Sampler| {
            s.cgs
                .values()
                .flat_map(|c| c.slots.iter())
                .filter(|s| matches!(s, Slot::Missing))
                .count()
        };
        assert_eq!(missing(&s), 6);
        let task = s.opts.root.join("step_0/user/task_1");
        fs::write(task.join("pids.current"), "7\n").unwrap();
        let pids = |tick: &[String]| tick[0].split('\t').nth(3 + 11).map(str::to_owned);
        let again = lines(|e| {
            s.tick(2000, e);
        });
        assert_eq!(pids(&again).as_deref(), Some("-"), "not retried by the next walk");
        assert_eq!(missing(&s), 6);
        let later = lines(|e| {
            s.tick(10_000, e);
        });
        assert_eq!(pids(&later).as_deref(), Some("7"), "retried once per OFF_WALK_MS");
    }

    #[test]
    fn fork_storm_turns_processes_off() {
        let (mut s, _t) = sampler("25.11", "storm");
        lines(|e| s.snapshot(0, e));
        rule(&mut s, "*\t.\t500\tc", 0);
        rule(&mut s, "*\tstep_0\t2000\tp", 0);
        lines(|e| {
            s.tick(2000, e);
        });
        let storm = lines(|e| s.storm(2000, e));
        assert_eq!(count(&storm, "W"), 1);
        assert_eq!(count(&storm, "x"), 3);
        let mut text = String::new();
        wire::write_control(&mut text, &Control::Rule(s.rules[0].clone()));
        assert_eq!(text, "R\t*\t.\t2000\tc\n");
        assert_eq!(wire::parse_control(&text), Ok(Control::Rule(s.rules[0].clone())));
        assert!(s.cgs.values().all(|c| c.what == What::Cg && c.interval == 2000));
    }

    #[test]
    fn zero_allocations_per_steady_state_tick() {
        let (mut s, _t) = sampler("25.11", "alloc");
        lines(|e| s.snapshot(0, e));
        rule(&mut s, "*\t.\t1000\tp", 0);
        let mut buf = Vec::with_capacity(1 << 17);
        let mut emit = |e: &Event| {
            if buf.len() > 1 << 16 {
                buf.clear();
            }
            wire::write(&mut buf, 7, e);
        };
        s.tick(1000, &mut emit);
        let before = crate::alloc_count::allocs();
        for _ in 0..100 {
            s.due.clear();
            s.due.push(1000);
            s.sample(&mut emit);
        }
        assert_eq!(crate::alloc_count::allocs() - before, 0);
        assert_eq!(s.core.files.open, 8 * 11 + 3 * 2 + 1, "the warm path reads cached fds only");
        // The discovery walk, which runs on nearly every due tick, is the allocating part.
        let entries: u64 =
            s.cgs.values().map(|c| fs::read_dir(&c.dir).unwrap().count() as u64).sum();
        let before = crate::alloc_count::allocs();
        s.tick(2000, &mut emit);
        let walk = crate::alloc_count::allocs() - before;
        assert!(walk <= 2 * entries, "{walk} allocations for {entries} directory entries");
    }

    /// Blocks every write until the test drops the sender, then records what was written.
    struct Gate(Receiver<()>, Arc<Mutex<Vec<u8>>>, mpsc::Sender<()>);

    impl Write for Gate {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            let _ = self.2.send(());
            let _ = self.0.recv();
            self.1.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn blocked_writer_drops_values_and_keeps_structure() {
        let (open, gate) = mpsc::channel();
        let (entered, blocked) = mpsc::channel();
        let written = Arc::new(Mutex::new(Vec::new()));
        let mut out = Out::spawn(Gate(gate, written.clone(), entered), 0, 1);
        for t in 0..20u64 {
            if t == 1 {
                blocked.recv().unwrap(); // the writer holds tick 0, so the queue state is fixed
            }
            out.push(&Event::Cg { id: t + 1, parent: 0, path: "a" });
            out.push(&Event::CgVals { id: t + 1, first: 0, v: &[t] });
            out.send(t * 1000, 1000, Some((t, 0))).unwrap();
        }
        assert!(out.dropped >= 14, "dropped {}", out.dropped);
        assert!(!out.pending.is_empty(), "structure waits for the writer");
        drop(open);
        let t0 = std::time::Instant::now();
        while written.lock().unwrap().split(|&b| b == b'\n').count() <= 40 {
            assert!(t0.elapsed() < Duration::from_secs(10), "writer never drained");
            thread::sleep(Duration::from_millis(5));
            out.send(30_000, 1000, None).unwrap();
        }
        let text = String::from_utf8(written.lock().unwrap().clone()).unwrap();
        for id in 1..=20 {
            assert_eq!(text.matches(&format!("\tG\t{id}\t0\ta\n")).count(), 1, "G {id} once");
        }
        assert_eq!(text.matches("\tT\t").count(), 20, "every T arrives");
        assert!(text.matches("\tg\t").count() <= 6);
    }

    #[test]
    fn budget_limits_bytes_per_second() {
        let mut out = Out::spawn(io::sink(), 0, 1);
        out.rate = 2000;
        out.tokens = 0;
        let v = [u64::MAX - 1; 40]; // one ≈ 850-byte value line per tick
        for t in 0..100u64 {
            out.push(&Event::CgVals { id: 1, first: 0, v: &v });
            out.send(t * 100, 100, None).unwrap();
            thread::sleep(Duration::from_millis(1));
        }
        assert!(out.sent <= 2000 * 10 + 1000, "{} bytes in 10 s at 2000 B/s", out.sent);
        assert!(out.sent >= 2000 * 4, "{} bytes", out.sent);
        assert!(out.dropped > 50);
        assert_eq!(stagger(3, 4, 1000), 3 * 250 / 4);
        assert_eq!(stagger(999, 1000, 200), 999 * 100 / 1000);
        assert_eq!(stagger(0, 0, 1000), 0);
    }

    fn xorshift(x: &mut u64) -> u64 {
        *x ^= *x << 13;
        *x ^= *x >> 7;
        *x ^= *x << 17;
        *x
    }

    /// Re-encodes one agent's stream for `n` nodes with node-specific values; returns the
    /// streams and each node's reference state, decoded directly.
    fn node_streams(base: &[u8], n: u32) -> (Vec<Vec<u8>>, State) {
        let mut want = State::new();
        let mut streams = vec![Vec::new(); n as usize];
        let mut vals = Vec::new();
        for line in base.split_inclusive(|&b| b == b'\n') {
            let ev = wire::parse(wire::split_node(line).unwrap().1, &mut vals).unwrap();
            for node in 0..n {
                let shift = |v: &[u64]| -> Vec<u64> {
                    v.iter().map(|&x| if x == NA { NA } else { x + u64::from(node) }).collect()
                };
                let shifted = match ev {
                    Event::CgVals { v, .. } | Event::ProcVals { v, .. } => shift(v),
                    _ => Vec::new(),
                };
                let ev = match ev {
                    Event::CgVals { id, first, .. } => Event::CgVals { id, first, v: &shifted },
                    Event::ProcVals { pid, first, .. } => {
                        Event::ProcVals { pid, first, v: &shifted }
                    }
                    other => other,
                };
                want.apply(node, &ev, 0).unwrap();
                wire::write(&mut streams[node as usize], node, &ev);
            }
        }
        (streams, want)
    }

    /// slurmstepd forwards whole lines in messages of at most 1024 bytes; returned last first.
    fn messages(stream: &[u8]) -> Vec<&[u8]> {
        let (mut out, mut start, mut end) = (Vec::new(), 0, 0);
        for line in stream.split_inclusive(|&b| b == b'\n') {
            assert!(line.len() <= wire::MAX_LINE);
            if end + line.len() - start > 1024 {
                out.push(&stream[start..end]);
                start = end;
            }
            end += line.len();
        }
        out.push(&stream[start..end]);
        out.reverse();
        out
    }

    /// srun interleaves the messages of all tasks into one stdout in random order.
    fn interleave(mut msgs: Vec<Vec<&[u8]>>) -> Vec<u8> {
        let (mut stdout, mut seed) = (Vec::new(), 0x9e37_79b9_7f4a_7c15);
        let mut live: Vec<usize> = (0..msgs.len()).collect();
        while !live.is_empty() {
            let k = usize::try_from(xorshift(&mut seed) % live.len() as u64).unwrap();
            stdout.extend_from_slice(msgs[live[k]].pop().unwrap());
            if msgs[live[k]].is_empty() {
                live.swap_remove(k);
            }
        }
        stdout
    }

    /// The TUI side: srun stdout through the real reader in short reads, `split_node`, `parse`,
    /// apply.
    fn demux(stdout: &[u8]) -> State {
        let (tx, rx) = mpsc::sync_channel(4);
        let (mut got, mut vals, mut seen) = (State::new(), Vec::new(), 0);
        thread::scope(|sc| {
            sc.spawn(move || crate::slurm::lines(crate::slurm::tests::Trickle(stdout, 3), &tx));
            for msg in rx {
                let Msg::Lines(b) = msg else { continue };
                seen += b.len();
                for line in b.split_inclusive(|&b| b == b'\n') {
                    let (node, rest) = wire::split_node(line).unwrap();
                    got.apply(node, &wire::parse(rest, &mut vals).unwrap(), 0).unwrap();
                }
            }
        });
        assert_eq!(seen, stdout.len());
        got
    }

    #[test]
    fn unlink_self_removes_only_a_copy_and_tolerates_it_gone() {
        let dir = std::env::temp_dir().join(format!("unjam-unlink-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let (copy, other) = (dir.join("unjam.42.1000.00000000000000bc"), dir.join("unjam"));
        for f in [&copy, &other] {
            fs::write(f, "").unwrap();
        }
        unlink_copy(&copy).unwrap();
        assert!(!copy.exists());
        // A second agent exec'd the same copy: it is gone, and the agent keeps running.
        unlink_copy(&copy).unwrap();
        let deleted = dir.join("unjam.42.1000.00000000000000bc (deleted)");
        unlink_copy(&deleted).unwrap();
        unlink_copy(&other).unwrap();
        assert!(other.exists(), "not a --bcast copy");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn mux_1000_agents() {
        let (mut s, _t) = sampler("25.11", "mux");
        let mut base = Vec::new();
        let mut emit = |e: &Event| wire::write(&mut base, 0, e);
        s.snapshot(0, &mut emit);
        s.control(wire::parse_control("R\t*\t.\t1000\tp").unwrap(), 0, &mut emit);
        for t in 1..=3 {
            let (self_cpu_us, self_rss) = s.tick(t * 1000, &mut emit);
            emit(&Event::Tick { ts_ms: t * 1000, self_cpu_us, out_bytes: 0, dropped: 0, self_rss });
        }
        let (streams, want) = node_streams(&base, 1000);
        let got = demux(&interleave(streams.iter().map(|s| messages(s)).collect()));
        let last = &got.nodes[999];
        assert_eq!((last.host.as_str(), last.prev[0]), ("worker-0", 3000));
        assert_eq!((last.cgs.len(), last.procs.len()), (8, 3));
        assert!(got == want, "every node's state equals its reference");
    }
}
