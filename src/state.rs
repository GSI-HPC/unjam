// SPDX-License-Identifier: LGPL-3.0-or-later
// SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany

//! The TUI data store that every feed fills through `State::apply`, and the rows that the three
//! layouts (step, node, flat) make of it: sibling-only sort, filter, folds and folded node
//! ranges (§4.5, §9.2, §9.4).

use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};

use crate::model::{Ent, Event, Kind, NA, Rule, What, effective};

/// Metrics the table shows, interned first so their column ids are constants: per entity in
/// value order (CPU, MEM, IO-R, IO-W, PIDS; processes have two CPU counters).
const KNOWN: [&[&str]; 2] = [
    &["cg_cpu_usage_usec", "cg_mem_current_bytes", "cg_io_read_bytes", "cg_io_write_bytes"],
    &["p_cpu_user_usec", "p_cpu_system_usec", "p_rss_bytes", "p_io_read_bytes"],
];
/// The fifth and sixth known columns: PIDS for cgroups; IO-W and threads for processes.
const MORE: [&[&str]; 2] = [&["cg_pids_current"], &["p_io_write_bytes", "p_threads"]];
/// Display values per row: CPU %, memory bytes, IO read B/s, IO write B/s, PIDS.
pub const NCOL: usize = 5;
/// History ring length: the last 16 CPU samples per entity (D41).
pub const HIST: usize = 16;

/// One interned column: name, unit and kind from its `D` line.
pub type Col = (Box<str>, Box<str>, Kind);

/// Interns `(Ent, name)` → column id per entity, so a new metric reaches the Detail view
/// without code changes (R10).
#[derive(Debug, PartialEq)]
pub struct Columns(pub [Vec<Col>; 2]);

impl Columns {
    /// Returns the id of `name`, adding it on first sight; a later `D` updates unit and kind.
    fn intern(&mut self, ent: Ent, name: &str, unit: &str, kind: Kind) -> u16 {
        let cols = &mut self.0[ent as usize];
        let i = cols.iter().position(|c| &*c.0 == name).unwrap_or(cols.len());
        if i == cols.len() {
            cols.push((name.into(), "".into(), kind));
        }
        (cols[i].1, cols[i].2) = (unit.into(), kind);
        u16::try_from(i).unwrap_or(u16::MAX)
    }
}

/// One entity's values at the last two ticks (for rates) and its CPU history ring.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Vals {
    pub v: Vec<u64>,
    pub pv: Vec<u64>,
    /// Tick times of `v` and `pv`.
    pub t: [u64; 2],
    hist: [u16; HIST],
    head: usize,
}

impl Vals {
    /// Stores one value line taken at tick `t` through the node's `D` map; the first line of a
    /// new tick shifts to `pv`, and the line with column 0 adds `cpu` to the history. `Err` if
    /// the `D` table does not cover the line.
    fn line(
        &mut self,
        map: &[u16],
        t: u64,
        first: u16,
        v: &[u64],
        cpu: fn(&Vals) -> Option<f64>,
    ) -> Result<(), ()> {
        let cols = map.get(usize::from(first)..usize::from(first) + v.len()).ok_or(())?;
        if cols.contains(&u16::MAX) {
            return Err(());
        }
        if t != self.t[0] {
            self.pv.clone_from(&self.v);
            self.t = [t, self.t[0]];
        }
        for (&c, &x) in cols.iter().zip(v) {
            let c = usize::from(c);
            self.v.resize(self.v.len().max(c + 1), NA);
            self.v[c] = x;
        }
        if cols.contains(&0) {
            self.push(cpu(self));
        }
        Ok(())
    }

    /// Counter → rate per second, Gauge → value; `None` for NA.
    pub fn get(&self, c: usize, kind: Kind) -> Option<f64> {
        let at = |v: &[u64]| v.get(c).copied().unwrap_or(NA);
        rate(kind, at(&self.v), at(&self.pv), self.t[0].saturating_sub(self.t[1]))
    }

    /// The history in time order, oldest first.
    pub fn hist(&self) -> [u16; HIST] {
        std::array::from_fn(|i| self.hist[(self.head + i) % HIST])
    }

    /// Appends CPU per mille of one core (µs/s ÷ 1000) to the ring.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "clamped first")]
    fn push(&mut self, us_per_s: Option<f64>) {
        self.hist[self.head] = us_per_s.map_or(0, |r| (r / 1000.0).clamp(0.0, 65535.0) as u16);
        self.head = (self.head + 1) % HIST;
    }
}

/// Counter → `(v − pv) / dt` per second (NA when it went backwards), Gauge → value.
#[allow(clippy::cast_precision_loss, reason = "display values; 2^53 is far beyond any counter")]
pub fn rate(kind: Kind, v: u64, pv: u64, dt_ms: u64) -> Option<f64> {
    match kind {
        _ if v == NA => None,
        Kind::Gauge => Some(v as f64),
        Kind::Counter if pv == NA || dt_ms == 0 => None,
        Kind::Counter => Some(v.checked_sub(pv)? as f64 * 1000.0 / dt_ms as f64),
    }
}

/// A cgroup: tree links, its processes (sorted pids), values and the derived thread sum.
#[derive(Debug, Default, PartialEq)]
pub struct Cg {
    pub parent: u64,
    pub path: Box<str>,
    pub kids: Vec<u64>,
    pub procs: Vec<u32>,
    pub vals: Vals,
    pub threads: u64,
}

/// A process with its `P` fields and values.
#[derive(Debug, PartialEq)]
pub struct Proc {
    pub start: u64,
    pub cg: u64,
    pub ppid: u32,
    pub comm: Box<str>,
    pub cmd: Box<str>,
    pub vals: Vals,
}

/// One agent's view: identity, column map from its `D` table, entities and liveness stats.
#[derive(Debug, Default, PartialEq)]
pub struct Node {
    pub host: String,
    pub step: String,
    pub proto: u32,
    /// Boot time from `H`, for the record metadata.
    pub btime: u64,
    cmap: [Vec<u16>; 2],
    pub cgs: HashMap<u64, Cg>,
    pub procs: HashMap<u32, Proc>,
    pub root: u64,
    /// The last `T` fields (time, own CPU µs, output bytes) and when it arrived (local ms).
    pub prev: [u64; 3],
    pub seen: u64,
    /// The agent's CPU %, RSS bytes and output B/s from consecutive `T` lines (D40); the TUI
    /// adds the lines per second.
    pub agent: [f64; 4],
    pub dropped: u32,
    /// Kept by the TUI: decode errors, lines since the last tick, time of the last `S`.
    pub errors: u64,
    pub lines: u64,
    pub asked: u64,
    dirty: bool,
    /// Cgroups and processes held at the last `H` and not re-announced since: a resync sends
    /// only what the agent has, so the next `T` removes these (a lost `X`/`x`, §7.4).
    stale: (HashSet<u64>, HashSet<u32>),
}

/// The TUI data store filled by every feed.
#[derive(Debug, PartialEq)]
pub struct State {
    pub cols: Columns,
    pub nodes: Vec<Node>,
}

impl State {
    /// An empty store with the table's columns interned at their constant ids.
    pub fn new() -> Self {
        let mut cols = Columns([vec![], vec![]]);
        for (ent, e) in [(Ent::Cgroup, 0), (Ent::Proc, 1)] {
            for n in KNOWN[e].iter().chain(MORE[e]) {
                cols.intern(ent, n, "", Kind::Gauge);
            }
        }
        Self { cols, nodes: Vec::new() }
    }

    /// The single mutation path. `Err` means the line does not fit what the node announced
    /// (unknown entity or value index), which makes the TUI ask for a resync (§7.4).
    pub fn apply(&mut self, node: u32, ev: &Event, now: u64) -> Result<(), ()> {
        let i = node as usize;
        self.nodes.resize_with(self.nodes.len().max(i + 1), Node::default);
        let n = &mut self.nodes[i];
        n.dirty = true;
        match *ev {
            Event::Hello { proto, host, step, btime, .. } => {
                (n.proto, n.host, n.step, n.btime, n.cmap) =
                    (proto, host.into(), step.into(), btime, Default::default());
                n.stale = (n.cgs.keys().copied().collect(), n.procs.keys().copied().collect());
            }
            _ if n.proto != crate::wire::PROTO => {}
            Event::Desc { ent, idx, name, unit, kind } => {
                let (id, m) = (self.cols.intern(ent, name, unit, kind), &mut n.cmap[ent as usize]);
                m.resize(m.len().max(usize::from(idx) + 1), u16::MAX);
                m[usize::from(idx)] = id;
            }
            Event::Tick { ts_ms, self_cpu_us, out_bytes, dropped, self_rss } => {
                n.tick([ts_ms, self_cpu_us, out_bytes], self_rss, now);
                n.dropped = dropped;
                let stale = std::mem::take(&mut n.stale);
                stale.0.into_iter().for_each(|id| n.cg_gone(id));
                stale.1.into_iter().for_each(|pid| n.unlink(pid));
            }
            Event::Cg { id, parent, path } => n.cg(id, parent, path),
            Event::CgVals { id, first, v } => {
                let c = n.cgs.get_mut(&id).ok_or(())?;
                c.vals.line(&n.cmap[0], n.prev[0], first, v, |x| x.get(0, Kind::Counter))?;
            }
            Event::CgGone(id) => n.cg_gone(id),
            Event::Proc { .. } => n.proc(ev),
            Event::ProcVals { pid, first, v } => {
                n.procs.get_mut(&pid).ok_or(())?.vals.line(&n.cmap[1], n.prev[0], first, v, cpu)?;
            }
            Event::ProcGone(pid) => n.unlink(pid),
            Event::Warn(_) => {}
        }
        Ok(())
    }

    /// PIDS fallback (§4.3): sums `p_threads` into each process's cgroup and its ancestors, for
    /// nodes that changed since the last call. Runs in the TUI's ≤ 4 Hz redraw slot.
    pub fn derive(&mut self) {
        for n in self.nodes.iter_mut().filter(|n| n.dirty) {
            n.dirty = false;
            n.cgs.values_mut().for_each(|c| c.threads = 0);
            for p in n.procs.values() {
                let Some(&t) = p.vals.v.get(P_THREADS).filter(|&&t| t != NA) else { continue };
                let mut id = p.cg;
                for _ in 0..64 {
                    let Some(c) = n.cgs.get_mut(&id) else { break };
                    (c.threads, id) = (c.threads + t, c.parent);
                }
            }
        }
    }
}

/// Column id of `p_threads`.
const P_THREADS: usize = 5;

/// A process's CPU in µs/s: user plus system.
fn cpu(v: &Vals) -> Option<f64> {
    let (u, s) = (v.get(0, Kind::Counter), v.get(1, Kind::Counter));
    u.or(s).map(|_| u.unwrap_or(0.0) + s.unwrap_or(0.0))
}

impl Node {
    /// The values of the node's root cgroup: host or job totals for headers and the heat strip.
    pub fn root_vals(&self) -> Option<&Vals> {
        self.cgs.get(&self.root).map(|c| &c.vals)
    }

    /// Updates the agent's self-monitoring rates and the liveness fields from a `T` line.
    #[allow(clippy::cast_precision_loss, reason = "display values")]
    fn tick(&mut self, t: [u64; 3], rss: u64, now: u64) {
        let dt = t[0].saturating_sub(self.prev[0]) as f64;
        let d = |i: usize, k: f64| {
            t[i].checked_sub(self.prev[i])
                .filter(|_| t[i] != NA)
                .map_or(f64::NAN, |x| x as f64 * k / dt)
        };
        if self.prev[0] > 0 && dt > 0.0 {
            (self.agent[0], self.agent[2]) = (d(1, 0.1), d(2, 1000.0));
        }
        self.agent[1] = if rss == NA { f64::NAN } else { rss as f64 };
        (self.prev, self.seen) = (t, now);
    }

    /// Adds or re-announces a cgroup (resyncs repeat `G` lines; values are kept). A new parent
    /// takes it from the old one's kids, and a parent at or below the cgroup itself (only a
    /// crafted journal) is ignored, so `rows` always walks a tree.
    fn cg(&mut self, id: u64, parent: u64, path: &str) {
        let mut up = std::iter::successors(Some(parent), |p| Some(self.cgs.get(p)?.parent));
        if up.any(|p| p == id) {
            return;
        }
        let old = self.cgs.get(&id).map_or(parent, |c| c.parent);
        self.cgs.entry(old).and_modify(|p| p.kids.retain(|&k| k != id || old == parent));
        if parent == 0 {
            self.root = id;
        } else if let Some(p) = self.cgs.get_mut(&parent).filter(|p| !p.kids.contains(&id)) {
            p.kids.push(id);
        }
        let c = self.cgs.entry(id).or_default();
        (c.parent, c.path) = (parent, path.into());
        self.stale.0.remove(&id);
    }

    /// Removes a cgroup and its link from its parent (`X`, or not re-announced by a resync).
    fn cg_gone(&mut self, id: u64) {
        let parent = self.cgs.remove(&id).map_or(0, |c| c.parent);
        self.cgs.entry(parent).and_modify(|p| p.kids.retain(|&k| k != id));
    }

    /// Adds a process or moves it to its new cgroup; a re-announced process keeps its values.
    #[allow(clippy::similar_names, reason = "pid and ppid are the kernel's names")]
    fn proc(&mut self, ev: &Event) {
        let Event::Proc { pid, start, cg, ppid, comm, cmd } = *ev else { return };
        let same = self.procs.get(&pid).filter(|p| p.start == start && p.cg == cg);
        let vals = same.map(|p| p.vals.clone()).unwrap_or_default();
        self.unlink(pid);
        self.stale.1.remove(&pid);
        self.procs.insert(pid, Proc { start, cg, ppid, comm: comm.into(), cmd: cmd.into(), vals });
        if let Some(c) = self.cgs.get_mut(&cg) {
            c.procs.insert(c.procs.binary_search(&pid).unwrap_or_else(|e| e), pid);
        }
    }

    /// Removes a process and its link from its cgroup.
    fn unlink(&mut self, pid: u32) {
        let Some(p) = self.procs.remove(&pid) else { return };
        self.cgs.entry(p.cg).and_modify(|c| c.procs.retain(|&x| x != pid));
    }
}

/// The three level orders of D39; local mode has no node level and uses `Node` as its tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    Step,
    Node,
    Flat,
}

/// Row identity: fold state, marks and the cursor follow it across redraws.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Key {
    #[default]
    Job,
    Step(u64),
    Folded(u64),
    Cg(u32, u64),
    Proc(u32, u32),
}

/// How the rows are made: layout, sort column (0 = NAME, value column + 1, NCOL + 1 = NODE),
/// folds, filter, the authoritative rule list, the folded-range window, whether this is local,
/// and which nodes are stale (set by the TUI once per second).
#[derive(Debug)]
pub struct View {
    pub layout: Layout,
    pub sort: usize,
    pub desc: bool,
    pub toggled: HashSet<Key>,
    pub filter: String,
    pub rules: Vec<Rule>,
    pub window: usize,
    pub local: bool,
    pub stale: Vec<bool>,
}

impl View {
    /// Folded node ranges start closed, everything else open; a toggle flips the default.
    pub fn is_open(&self, k: Key) -> bool {
        !matches!(k, Key::Folded(_)) ^ self.toggled.contains(&k)
    }
}

/// One table row. Names borrow from the state (only cross-node rows own theirs), so rows are
/// rebuilt per frame. `id` is the cgroup id or pid, for aggregates their first member's.
#[derive(Clone, Debug, Default)]
pub struct Row<'a> {
    pub key: Key,
    pub node: u32,
    pub id: u64,
    pub depth: u16,
    pub open: Option<bool>,
    pub last: bool,
    pub name: Cow<'a, str>,
    pub v: [f64; NCOL],
    pub approx: bool,
    pub hist: [u16; HIST],
}

/// The visible-window walk: rows in layout order until `limit` rows exist (§9.4).
struct Walk<'a, 'v> {
    st: &'a State,
    v: &'v View,
    out: Vec<Row<'a>>,
    limit: usize,
    /// Inside the subtree of a filter match: shown in full, with its folds.
    free: bool,
}

/// Builds the first `limit` rows of the view.
pub fn rows<'a>(st: &'a State, v: &View, limit: usize) -> Vec<Row<'a>> {
    let mut w = Walk { st, v, out: Vec::new(), limit, free: false };
    let mut top = Vec::new();
    for (i, n) in (0..).zip(&st.nodes) {
        match n.cgs.get(&n.root) {
            Some(_) if v.layout == Layout::Flat => w.flat(i, n, &mut top),
            Some(c) => top.push(w.cg(i, n.root, c, Some(if v.local { "/" } else { &n.host }))),
            None => {}
        }
    }
    if v.layout == Layout::Step {
        let job = st.nodes.first().map_or("", |n| n.step.split('.').next().unwrap_or(""));
        top = vec![agg(Key::Job, format!("job {job}").into(), &top)];
    }
    w.siblings(top, 0, None);
    w.out
}

impl<'a> Walk<'a, '_> {
    /// Sorts one sibling set, folds a step's surplus nodes into one range row, and emits each
    /// row with its open subtree. Returns whether anything survived the filter: a match keeps
    /// its ancestors (opened) and its whole subtree (as folded).
    fn siblings(&mut self, mut sibs: Vec<Row<'a>>, depth: u16, parent: Option<Key>) -> bool {
        let room = self.limit.saturating_sub(self.out.len());
        let filter = !self.v.filter.is_empty() && !self.free;
        let step = matches!(parent, Some(Key::Step(_)));
        if sibs.len() > room && room > 0 && !filter && !step {
            sibs.select_nth_unstable_by(room - 1, |a, b| self.cmp(a, b));
            sibs.truncate(room);
        }
        sibs.sort_by(|a, b| self.cmp(a, b));
        let (mut rest, window) = (Vec::new(), self.v.window.max(1));
        if let Some(Key::Step(h)) = parent.filter(|_| sibs.len() > window) {
            rest = sibs.split_off(window);
            let mut names: Vec<&str> = rest.iter().map(|r| &*r.name).collect();
            names.sort_by(|a, b| natural(a, b));
            let name = format!("{} … {} ({} folded)", names[0], names[names.len() - 1], rest.len());
            sibs.push(agg(Key::Folded(h), name.into(), &rest));
        }
        let (n, mut any) = (sibs.len(), false);
        for (i, mut r) in sibs.into_iter().enumerate() {
            if self.out.len() >= self.limit {
                break;
            }
            let kids =
                if let Key::Folded(_) = r.key { std::mem::take(&mut rest) } else { self.kids(&r) };
            let hit = !filter || self.hit(&r);
            let open = (filter && !hit) || self.v.is_open(r.key);
            (r.depth, r.last, r.open) = (depth, i + 1 == n, (!kids.is_empty()).then_some(open));
            let (mark, key) = (self.out.len(), r.key);
            self.out.push(r);
            let free = self.free;
            self.free |= hit;
            let deep = open && !kids.is_empty() && self.siblings(kids, depth + 1, Some(key));
            self.free = free;
            if hit || deep {
                any = true;
            } else {
                self.out.truncate(mark);
            }
        }
        any
    }

    /// Filter match: an ASCII case-insensitive substring of the name, a process's comm or a
    /// cgroup's path; a leading `/` is ignored, so a path matches as typed.
    fn hit(&self, r: &Row) -> bool {
        let q = self.v.filter.trim_start_matches('/').as_bytes();
        let has = |h: &str| {
            q.is_empty() || h.as_bytes().windows(q.len()).any(|w| w.eq_ignore_ascii_case(q))
        };
        let n = self.st.nodes.get(r.node as usize);
        has(&r.name)
            || match r.key {
                Key::Cg(_, id) => n.and_then(|n| n.cgs.get(&id)).is_some_and(|c| has(&c.path)),
                Key::Proc(_, p) => n.and_then(|n| n.procs.get(&p)).is_some_and(|p| has(&p.comm)),
                _ => false,
            }
    }

    /// Sibling order: the sort column with NA last in both directions, ties and NAME in
    /// natural order; NODE orders by host name in natural order, then NAME.
    fn cmp(&self, a: &Row, b: &Row) -> Ordering {
        let name = natural(&a.name, &b.name).then(a.node.cmp(&b.node)).then(a.id.cmp(&b.id));
        let flip = |ord: Ordering| if self.v.desc { ord.reverse() } else { ord };
        if self.v.sort > NCOL {
            let host = |r: &Row| self.st.nodes.get(r.node as usize).map_or("", |n| &n.host);
            return flip(natural(host(a), host(b)).then(name));
        }
        let Some(col) = self.v.sort.checked_sub(1) else { return flip(name) };
        match (a.v[col].is_nan(), b.v[col].is_nan()) {
            (false, false) => flip(a.v[col].total_cmp(&b.v[col])).then(name),
            (na_a, na_b) => na_a.cmp(&na_b).then(name),
        }
    }

    /// The children of a row in the current layout. In the step layout a `user` cgroup is
    /// replaced by its tasks (job → step → node → task → process).
    fn kids(&self, r: &Row<'a>) -> Vec<Row<'a>> {
        let mut out: Vec<Row<'a>> = Vec::new();
        let (i, step) = (r.node, self.v.layout == Layout::Step);
        let Some(n) = self.st.nodes.get(i as usize) else { return out };
        match r.key {
            _ if self.v.layout == Layout::Flat => {}
            Key::Job => {
                let mut steps: Vec<(&str, Vec<Row<'a>>)> = Vec::new();
                for (name, m) in self.members(None) {
                    match steps.iter_mut().find(|s| s.0 == name) {
                        Some(s) => s.1.push(m),
                        None => steps.push((name, vec![m])),
                    }
                }
                out.extend(steps.iter().map(|(s, m)| agg(Key::Step(hash(s)), Cow::Borrowed(s), m)));
            }
            Key::Step(h) => out.extend(self.members(Some(h)).map(|m| m.1)),
            Key::Cg(_, id) => {
                let Some(c) = n.cgs.get(&id) else { return out };
                for (k, kc) in c.kids.iter().filter_map(|k| Some((*k, n.cgs.get(k)?))) {
                    if step && seg(&kc.path) == "user" {
                        let tasks = kc.kids.iter().filter_map(|t| Some((*t, n.cgs.get(t)?)));
                        out.extend(tasks.map(|(t, tc)| self.cg(i, t, tc, None)));
                    } else {
                        out.push(self.cg(i, k, kc, None));
                    }
                }
                out.extend(c.procs.iter().filter_map(|&p| self.proc_row(i, p)));
            }
            Key::Proc(..) | Key::Folded(_) => {}
        }
        out
    }

    /// Step layout: every node's job-level cgroups (steps), or those of one step, as rows
    /// named by their host, each with its step name.
    fn members(&self, step: Option<u64>) -> impl Iterator<Item = (&'a str, Row<'a>)> + '_ {
        let st = self.st;
        let per_node = (0..).zip(&st.nodes).filter_map(|(i, n)| Some((i, n, n.cgs.get(&n.root)?)));
        let all = per_node.flat_map(|(i, n, root)| {
            root.kids.iter().filter_map(move |k| Some((i, n, *k, n.cgs.get(k)?)))
        });
        all.filter(move |(.., c)| step.is_none_or(|h| hash(seg(&c.path)) == h))
            .map(move |(i, n, k, c)| (seg(&c.path), self.cg(i, k, c, Some(&n.host))))
    }

    /// Flat layout: every process, plus leaf cgroups whose processes are not sampled.
    fn flat(&self, i: u32, n: &'a Node, out: &mut Vec<Row<'a>>) {
        out.extend(n.procs.keys().filter_map(|&p| self.proc_row(i, p)));
        let leaf =
            |c: &&Cg| c.kids.is_empty() && effective(&self.v.rules, i, &c.path).1 < What::Procs;
        out.extend(n.cgs.iter().filter(|(_, c)| leaf(c)).map(|(&id, c)| self.cg(i, id, c, None)));
    }

    /// A cgroup row, blank when its node is stale or its effective level is off: neither sends
    /// current values, so the row and the sums over it show none (§6.1, §7.4).
    fn cg(&self, node: u32, id: u64, c: &'a Cg, name: Option<&'a str>) -> Row<'a> {
        let (mut r, rules) = (cg_row(node, id, c, name), &self.v.rules);
        let off = !rules.is_empty() && effective(rules, node, &c.path).1 == What::Off;
        if off || self.v.stale.get(node as usize) == Some(&true) {
            (r.v, r.approx, r.hist) = ([f64::NAN; NCOL], false, [0; HIST]);
        }
        r
    }

    /// A process row, named by its command line (comm for kernel threads).
    fn proc_row(&self, node: u32, pid: u32) -> Option<Row<'a>> {
        let p = self.st.nodes[node as usize].procs.get(&pid)?;
        let dead = self.v.stale.get(node as usize) == Some(&true);
        let v = if dead {
            [f64::NAN; NCOL]
        } else {
            values(&p.vals, cpu(&p.vals), [2, 3, 4, P_THREADS])
        };
        let name = Cow::Borrowed(if p.cmd.is_empty() { &*p.comm } else { &*p.cmd });
        let (key, id) = (Key::Proc(node, pid), u64::from(pid));
        let hist = if dead { [0; HIST] } else { p.vals.hist() };
        Some(Row { key, node, id, name, v, hist, ..Row::default() })
    }
}

/// A cgroup row named by its last path segment or `name`; PIDS falls back to `~` summed
/// threads where `cg_pids_current` is NA (§4.3).
#[allow(clippy::cast_precision_loss, reason = "display values")]
fn cg_row<'a>(node: u32, id: u64, c: &'a Cg, name: Option<&'a str>) -> Row<'a> {
    let mut v = values(&c.vals, c.vals.get(0, Kind::Counter), [1, 2, 3, 4]);
    let approx = v[4].is_nan() && c.threads > 0;
    v[4] = if approx { c.threads as f64 } else { v[4] };
    let (key, name, hist) =
        (Key::Cg(node, id), name.unwrap_or_else(|| seg(&c.path)).into(), c.vals.hist());
    Row { key, node, id, name, v, approx, hist, ..Row::default() }
}

/// Display values: CPU (µs/s → %), then the MEM gauge, IO counters and PIDS gauge at `cols`.
fn values(v: &Vals, cpu: Option<f64>, cols: [usize; 4]) -> [f64; NCOL] {
    let kinds = [Kind::Gauge, Kind::Counter, Kind::Counter, Kind::Gauge];
    let mut out = [cpu.map_or(f64::NAN, |c| c / 1e4); NCOL];
    for ((o, c), k) in out[1..].iter_mut().zip(cols).zip(kinds) {
        *o = v.get(c, k).unwrap_or(f64::NAN);
    }
    out
}

/// A cross-node row (job, step, folded range): values and history summed over its members.
fn agg<'a>(key: Key, name: Cow<'a, str>, m: &[Row<'a>]) -> Row<'a> {
    let (node, id) = m.first().map_or((0, 0), |r| (r.node, r.id));
    let mut r = Row { key, node, id, name, v: [f64::NAN; NCOL], ..Row::default() };
    for x in m {
        for (s, &v) in r.v.iter_mut().zip(&x.v).filter(|(_, v)| !v.is_nan()) {
            *s = if s.is_nan() { v } else { *s + v };
        }
        r.hist.iter_mut().zip(&x.hist).for_each(|(s, &h)| *s = s.saturating_add(h));
        r.approx |= x.approx;
    }
    r
}

/// The last path segment: what the NAME column shows for a cgroup.
fn seg(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Stable identity for a step name across nodes.
fn hash(s: &str) -> u64 {
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// Natural order: digit runs compare by value, so `n2` < `n10` and `task_9` < `task_10`.
pub fn natural(a: &str, b: &str) -> Ordering {
    let (mut left, mut right) = (a.as_bytes(), b.as_bytes());
    while let (Some(l0), Some(r0)) = (left.first(), right.first()) {
        let run = |s: &[u8]| s.iter().take_while(|d| d.is_ascii_digit()).count();
        let digits = l0.is_ascii_digit() && r0.is_ascii_digit();
        let (llen, rlen) = if digits { (run(left), run(right)) } else { (1, 1) };
        let (lnum, rnum) = (strip0(&left[..llen]), strip0(&right[..rlen]));
        match lnum.len().cmp(&rnum.len()).then(lnum.cmp(rnum)) {
            Ordering::Equal => (left, right) = (&left[llen..], &right[rlen..]),
            ord => return ord,
        }
    }
    left.len().cmp(&right.len())
}

/// Digit runs without leading zeros, so they compare by length first (single other bytes
/// pass unchanged).
fn strip0(s: &[u8]) -> &[u8] {
    if s.len() > 1 { &s[s.iter().position(|&b| b != b'0').unwrap_or(s.len())..] } else { s }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::{agent, procfs, wire};

    /// Wire text without node prefixes, as the agent writes it (fields joined by tabs).
    const DESC: &[&str] = &[
        "H 1 0.1 n10 42.0 1 0",
        "D c 0 cg_cpu_usage_usec us counter",
        "D c 1 cg_mem_current_bytes B gauge",
        "D p 0 p_cpu_user_usec us counter",
        "D p 1 p_rss_bytes B gauge",
    ];

    /// Applies space-separated lines (`~` stands for a space inside a field) to `node`.
    fn feed(st: &mut State, node: u32, lines: &[&str]) {
        let mut vals = Vec::new();
        for l in lines {
            let text: Vec<String> = l.split(' ').map(|f| f.replace('~', " ")).collect();
            let line = format!("{}\n", text.join("\t"));
            let ev = wire::parse(line.as_bytes(), &mut vals).unwrap_or_else(|| panic!("{line:?}"));
            st.apply(node, &ev, 0).unwrap();
        }
    }

    /// Two ticks of one node: job 42 with step 0 and the batch step, tasks 0, 9 and 10 under
    /// `user`, two processes in task 0. `cpu` scales the node's CPU rates.
    fn node(st: &mut State, i: u32, host: &str, cpu: u64) {
        let hello = format!("H 1 0.1 {host} 42.0 1 0");
        feed(st, i, &[&hello]);
        feed(st, i, &DESC[1..]);
        feed(
            st,
            i,
            &[
                "G 1 0 ",
                "G 2 1 step_0",
                "G 3 2 step_0/user",
                "G 4 3 step_0/user/task_0",
                "G 5 3 step_0/user/task_10",
                "G 6 3 step_0/user/task_9",
                "G 7 1 step_batch",
                "P 100 1 4 1 a.out ./a.out~-n~4",
                "P 101 1 4 100 kworker ",
            ],
        );
        for (t, k) in [(1000, 0), (2000, 1)] {
            let c = |x: u64| (k * x * cpu).to_string();
            feed(st, i, &[&format!("T {t} 0 0 0 -")]);
            let lines = [
                format!("g 1 0 {} 4096", c(900_000)),
                format!("g 2 0 {} 2048", c(800_000)),
                format!("g 4 0 {} 1024", c(600_000)),
                format!("g 5 0 {} 512", c(200_000)),
                format!("g 7 0 {} 8", c(1000)),
                format!("p 100 0 {} 1000", c(500_000)),
                format!("p 101 0 {} -", c(100_000)),
            ];
            feed(st, i, &lines.iter().map(String::as_str).collect::<Vec<_>>());
        }
    }

    #[test]
    #[ignore = "benchmark: mise run bench"]
    #[allow(clippy::print_stdout, reason = "the number is the report docs/perf.md quotes")]
    fn bench_apply_values() {
        let mut st = State::new();
        node(&mut st, 0, "n1", 1);
        let lines: [&[u8]; 7] = [
            b"g\t1\t0\t1800000\t4096\n",
            b"g\t2\t0\t1600000\t2048\n",
            b"g\t4\t0\t1200000\t1024\n",
            b"g\t5\t0\t400000\t512\n",
            b"g\t7\t0\t2000\t8\n",
            b"p\t100\t0\t1000000\t1000\n",
            b"p\t101\t0\t200000\t-\n",
        ];
        let (mut vals, mut values) = (Vec::new(), 0u128);
        let start = std::time::Instant::now();
        for _ in 0..200_000 {
            for line in lines {
                let ev = wire::parse(line, &mut vals).unwrap();
                st.apply(0, &ev, 0).unwrap();
                values += vals.len() as u128;
            }
        }
        let ps = start.elapsed().as_nanos() * 1000 / values;
        println!("bench_apply_values: decode + apply {}.{:03} ns/value", ps / 1000, ps % 1000);
    }

    fn view(layout: Layout) -> View {
        let (toggled, filter) = (HashSet::new(), String::new());
        View {
            layout,
            sort: 1,
            desc: true,
            toggled,
            filter,
            rules: vec![],
            window: 16,
            local: false,
            stale: vec![],
        }
    }

    /// One line per row: indent, fold state, name and CPU %.
    fn dump(rows: &[Row]) -> String {
        let fold = |o: Option<bool>| match o {
            Some(true) => "- ",
            Some(false) => "+ ",
            None => "  ",
        };
        let line = |r: &Row| {
            let ind = usize::from(r.depth) * 2;
            format!("{:ind$}{}{} {:.0}\n", "", fold(r.open), r.name, r.v[0])
        };
        rows.iter().map(line).collect()
    }

    fn two_nodes() -> State {
        let mut st = State::new();
        node(&mut st, 0, "n10", 1);
        node(&mut st, 1, "n2", 2);
        st
    }

    #[test]
    fn layouts_of_an_empty_store() {
        for layout in [Layout::Step, Layout::Node, Layout::Flat] {
            assert!(rows(&State::new(), &view(layout), 100).len() <= 1, "{layout:?}");
        }
    }

    #[test]
    fn parent_cycles_are_refused() {
        for (g, kids) in [
            (&["G 1 0 ", "G 1 1 a"][..], [(1, vec![])]),
            (&["G 1 0 ", "G 2 1 a", "G 3 2 a/b", "G 1 3 a/b/c"], [(1, vec![2])]),
            (&["G 1 0 ", "G 2 1 a", "G 2 0 ", "G 1 2 a"], [(1, vec![])]),
        ] {
            let mut st = State::new();
            feed(&mut st, 0, &DESC[..1]);
            feed(&mut st, 0, g);
            for (id, want) in kids {
                assert_eq!(st.nodes[0].cgs[&id].kids, want, "{g:?}");
            }
            for layout in [Layout::Step, Layout::Node, Layout::Flat] {
                assert!(rows(&st, &view(layout), usize::MAX).len() <= 4, "{g:?}");
            }
        }
    }

    #[test]
    fn step_layout_inlines_user_and_sorts_siblings() {
        let st = two_nodes();
        assert_eq!(
            dump(&rows(&st, &view(Layout::Step), usize::MAX)),
            "\
- job 42 270
  - step_0 240
    - n2 160
      - task_0 120
          ./a.out -n 4 100
          kworker 20
        task_10 40
        task_9 NaN
    - n10 80
      - task_0 60
          ./a.out -n 4 50
          kworker 10
        task_10 20
        task_9 NaN
  - step_batch 0
      n2 0
      n10 0
"
        );
    }

    #[test]
    fn off_and_stale_rows_show_no_values_and_leave_the_step_sums() {
        let st = two_nodes();
        let mut v = view(Layout::Step);
        v.rules = ["*\t.\t1000\tp", "1\tstep_0\t-\to"].map(|r| Rule::parse(r).unwrap()).into();
        let text = dump(&rows(&st, &v, 4));
        assert_eq!(text, "- job 42 270\n  - step_0 80\n    - n10 80\n      - task_0 60\n");
        let n2 = rows(&st, &v, usize::MAX).into_iter().find(|r| r.name == "n2").unwrap();
        assert!(n2.v[0].is_nan() && n2.hist == [0; HIST], "n2's step_0 is off");
        v.rules.truncate(1);
        v.stale = vec![false, true];
        let text = dump(&rows(&st, &v, 3));
        assert_eq!(text, "- job 42 90\n  - step_0 80\n    - n10 80\n", "n2 is stale");
    }

    #[test]
    fn node_and_flat_layouts() {
        let st = two_nodes();
        let mut v = view(Layout::Node);
        v.sort = 0;
        v.desc = false;
        let node = rows(&st, &v, 6);
        assert_eq!(
            dump(&node),
            "\
- n2 180
  - step_0 160
    - user NaN
      - task_0 120
          ./a.out -n 4 100
          kworker 20
"
        );
        let mut v = view(Layout::Flat);
        assert_eq!(dump(&rows(&st, &v, 3)), "  task_0 120\n  ./a.out -n 4 100\n  task_0 60\n");
        v.rules = vec![Rule::parse("*\t.\t1000\tp").unwrap()];
        let flat = dump(&rows(&st, &v, 4));
        assert_eq!(flat, "  ./a.out -n 4 100\n  ./a.out -n 4 50\n  kworker 20\n  kworker 10\n");
        (v.sort, v.desc) = (NCOL + 1, false);
        let by_node = dump(&rows(&st, &v, 4));
        assert_eq!(by_node, "  ./a.out -n 4 100\n  kworker 20\n  ./a.out -n 4 50\n  kworker 10\n");
    }

    #[test]
    fn folds_filter_and_na_last() {
        let st = two_nodes();
        let mut v = view(Layout::Step);
        v.toggled.insert(Key::Step(hash("step_0")));
        assert_eq!(
            dump(&rows(&st, &v, usize::MAX)),
            "\
- job 42 270
  + step_0 240
  - step_batch 0
      n2 0
      n10 0
"
        );
        v.toggled.clear();
        (v.desc, v.filter) = (false, "task_".into());
        let names: Vec<_> = rows(&st, &v, usize::MAX).iter().map(|r| r.name.to_string()).collect();
        assert_eq!(names[..6], ["job 42", "step_0", "n10", "task_10", "task_0", "kworker"]);
        v.filter = "TASK_0".into();
        let hit = dump(&rows(&st, &v, usize::MAX));
        assert!(hit.contains("task_0 120\n          kworker 20\n          ./a.out"), "{hit}");
        v.filter = "/user/".into();
        assert!(dump(&rows(&st, &v, usize::MAX)).contains("task_0 120"));
        v.filter = "nomatch".into();
        assert!(rows(&st, &v, usize::MAX).is_empty());
    }

    #[test]
    fn surplus_nodes_fold_into_a_range() {
        let mut st = State::new();
        for i in 0..5 {
            node(&mut st, i, &format!("n{}", i + 1), u64::from(5 - i));
        }
        let mut v = view(Layout::Step);
        v.window = 2;
        v.toggled.insert(Key::Step(hash("step_batch")));
        v.toggled.extend((0..5).map(|i| Key::Cg(i, 2)));
        let rows = rows(&st, &v, usize::MAX);
        assert_eq!(
            dump(&rows),
            "\
- job 42 1350
  - step_0 1200
    + n1 400
    + n2 320
    + n3 … n5 (3 folded) 480
  + step_batch 2
"
        );
        assert!(rows[4].hist.iter().any(|&h| h > 0), "the range sums its members' history");
    }

    #[test]
    fn history_ring_and_rates() {
        let mut v = Vals::default();
        for i in 0..20u16 {
            v.push(Some(f64::from(i) * 1000.0));
        }
        assert_eq!(v.hist()[0], 4);
        assert_eq!(v.hist()[HIST - 1], 19);
        assert_eq!(rate(Kind::Counter, 3000, 1000, 2000), Some(1000.0));
        assert_eq!(rate(Kind::Counter, 1000, 3000, 2000), None);
        assert_eq!(rate(Kind::Counter, 1000, NA, 2000), None);
        assert_eq!(rate(Kind::Gauge, 7, NA, 0), Some(7.0));
        assert_eq!(rate(Kind::Gauge, NA, 1, 1), None);
    }

    #[test]
    fn resync_drops_what_the_agent_no_longer_announces() {
        // A lost `X 6` and `x 101`: the resync's H, D, G and P lines no longer list them.
        let mut st = State::new();
        node(&mut st, 0, "n1", 1);
        let live = ["G 1 0 ", "G 2 1 step_0", "G 3 2 step_0/user", "G 4 3 step_0/user/task_0"];
        feed(&mut st, 0, &[&["H 1 0.1 n1 42.0 1 0"], &DESC[1..], &live[..]].concat());
        feed(&mut st, 0, &["G 5 3 step_0/user/task_10", "G 7 1 step_batch"]);
        feed(&mut st, 0, &["P 100 1 4 1 a.out ./a.out~-n~4"]);
        assert_eq!((st.nodes[0].cgs.len(), st.nodes[0].procs.len()), (7, 2), "kept until T");
        feed(&mut st, 0, &["T 3000 0 0 0 -"]);
        let n = &st.nodes[0];
        assert!(!n.cgs.contains_key(&6) && n.cgs[&3].kids == [4, 5], "{:?}", n.cgs[&3].kids);
        assert_eq!(
            (n.procs.keys().collect::<Vec<_>>(), &n.cgs[&4].procs),
            (vec![&100], &vec![100])
        );
        assert_eq!(n.procs[&100].vals.v[0], 500_000, "re-announced: values kept");
        feed(&mut st, 0, &["T 4000 0 0 0 -"]);
        assert_eq!(st.nodes[0].cgs.len(), 6, "only the T after H sweeps");
    }

    #[test]
    fn natural_order() {
        let mut names = ["n10", "n2", "n1", "task_09", "task_10", "a", "n02x"];
        names.sort_by(|a, b| natural(a, b));
        assert_eq!(names, ["a", "n1", "n2", "n02x", "n10", "task_09", "task_10"]);
    }

    #[test]
    fn bad_lines_and_old_protocols() {
        let mut st = State::new();
        feed(&mut st, 0, DESC);
        let mut vals = vec![];
        let bad = wire::parse(b"g\t9\t0\t1\n", &mut vals).unwrap();
        assert_eq!(st.apply(0, &bad, 0), Err(()), "unknown cgroup");
        feed(&mut st, 0, &["G 1 0 /"]);
        let bad = wire::parse(b"g\t1\t5\t1\n", &mut vals).unwrap();
        assert_eq!(st.apply(0, &bad, 0), Err(()), "value index without D");
        feed(&mut st, 1, &["H 2 0.9 future 1.0 1 0", "G 1 0 /"]);
        assert!(st.nodes[1].cgs.is_empty(), "a newer protocol is ignored after its H line");
    }

    /// The agent over the no-controllers fixture: no `pids.current`, so PIDS is the `~` sum of
    /// the processes' threads, per cgroup and up to the job.
    #[test]
    fn derive_sums_threads_without_the_pids_controller() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let job = fixtures.join("cgroup/no-controllers/system.slice/slurmstepd.scope/job_42");
        let env = procfs::env(fixtures.join("proc"), job.clone());
        let opts = agent::Opts {
            node: 0,
            ntasks: 1,
            step: "42.0".into(),
            root: job,
            exclude: None,
            cap: 64,
        };
        let mut s = agent::Sampler::new(env, opts);
        let (mut buf, mut st, mut vals) = (Vec::new(), State::new(), Vec::new());
        let mut emit = |e: &Event| wire::write(&mut buf, 0, e);
        s.snapshot(0, &mut emit);
        s.control(wire::parse_control("R\t*\t.\t1000\tp").unwrap(), 0, &mut emit);
        s.tick(1000, &mut emit);
        for line in buf.split_inclusive(|&b| b == b'\n') {
            let (node, rest) = wire::split_node(line).unwrap();
            st.apply(node, &wire::parse(rest, &mut vals).unwrap(), 1000).unwrap();
        }
        st.derive();
        let mut v = view(Layout::Node);
        v.sort = 0;
        let rows = rows(&st, &v, usize::MAX);
        let pids: Vec<_> = rows.iter().map(|r| (r.name.to_string(), r.v[4], r.approx)).collect();
        let find = |n: &str| pids.iter().find(|p| p.0 == n).cloned().unwrap();
        let (job, task0) = (find("worker-0"), find("task_0"));
        assert!(job.2 && task0.2, "{pids:?}");
        assert!(job.1 >= task0.1 && task0.1 > 0.0, "{pids:?}");
        assert!(!find("step_batch").2, "no processes, no fallback");
    }
}
