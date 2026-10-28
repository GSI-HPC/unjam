// SPDX-License-Identifier: LGPL-3.0-or-later
// SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany

//! The viewer: one channel that the input thread, the sampler (or srun readers) and a 1 Hz
//! ticker feed; key bindings, drawing with the self-monitoring header, heat strip and history
//! bars, and the `--batch` text snapshots (§3.4, §7.3, §9).

use std::collections::{HashSet, VecDeque};
use std::fmt::Write as _;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};
use std::{fs, thread};

use ratatui::crossterm::event::{self, Event as Term, KeyCode, KeyEvent};
use ratatui::crossterm::event::{KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout as Split, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Clear, Paragraph, Row as TRow, Table};
use ratatui::{DefaultTerminal, Frame};

use crate::agent::{OFF_WALK_MS, now_ms};
use crate::model::{self, Control, Event, Kind, NA, Rule, TaskSet, What, effective, upsert};
use crate::record::{Reader, Rec};
use crate::source::{Files, Slot, kv};
use crate::state::{self, HIST, Key, Layout, NCOL, Row, State, View};
use crate::theme::{THEMES, Theme};
use crate::{procfs, wire};

/// What the main thread blocks on (D38): terminal events, data, the end of srun's output and
/// the 1 Hz tick.
pub enum Msg {
    Key(KeyEvent),
    Resize,
    Lines(Vec<u8>),
    Stderr(String),
    End,
    Tick,
}

/// Value columns after NAME: header, unit for `fmt`, width; in `state` value order.
const COLS: [(&str, &str, u16); NCOL] = [
    ("CPU%", "%", 6),
    ("MEM", "B", 6),
    ("IO-R/s", "B/s", 7),
    ("IO-W/s", "B/s", 7),
    ("PIDS", "1", 6),
];
/// Sort columns cycled by `s`: NAME, the value columns, then NODE (not in local mode).
const SORTS: [&str; NCOL + 2] = ["NAME", "CPU%", "MEM", "IO-R/s", "IO-W/s", "PIDS", "NODE"];
/// Key hints for the footer and the help pane.
const HINTS: &str = "↑↓ move ←→ fold ␣ mark p what +/- rate v layout s sort / filter r record t theme ? help q quit";
/// Replay key hints.
const REPLAY: &str = "␣ play , . tick < > 60s g G start/end";
/// Set by Ctrl-C in the input thread, by `q`, SIGTERM and SIGHUP: quits even while a draw
/// blocks (D38).
static QUIT: LazyLock<Arc<AtomicBool>> = LazyLock::new(Arc::default);
/// Whether the filter is being edited, so the input thread leaves `q` to the filter.
static EDIT: AtomicBool = AtomicBool::new(false);

/// A rule target: the agents and the cgroup path pattern.
type Target = (TaskSet, String);

/// What the CLI decides for a viewer session.
pub struct Opts {
    pub source: String,
    pub interval_ms: u32,
    pub rules: Vec<Rule>,
    pub theme: usize,
    pub local: bool,
}

/// The TUI's own cost and data rates for the self-monitoring header (D40).
struct Mon {
    /// Cached `/proc/self/stat`, page size and clock ticks.
    stat: (Files, Slot, [u64; 2]),
    /// Time and own CPU µs at the last tick; own CPU % and RSS bytes.
    at: [u64; 2],
    tui: [f64; 2],
    /// Bytes in, lines in and control bytes out since the last tick, and their rates.
    count: [u64; 3],
    rate: [f64; 3],
    /// Host memory used and total, and the CPU count, for the local header bars.
    host: [f64; 3],
}

/// UI state around the data store.
pub struct App {
    pub st: State,
    pub view: View,
    opts: Opts,
    ctl: Sender<String>,
    /// Cursor index, the entity it follows, and the first visible row.
    cursor: usize,
    sel: Option<Key>,
    scroll: usize,
    marks: Vec<(Key, TaskSet, String)>,
    /// 0: table only, 1: detail popup, 2: help and log, 3: editing the filter.
    overlay: u8,
    pub log: VecDeque<String>,
    /// Nodes the job has (from `start`) and whether srun ended (job over, every agent gone).
    expect: u32,
    ended: bool,
    /// `--budget`: the total agent output in bytes per second, split over `expect` agents.
    pub budget: Option<u64>,
    mon: Mon,
    pub now: u64,
    ticks: u64,
    vals: Vec<u64>,
    /// Replay: the reader and whether it plays; live: the recording and its `--record` DIR.
    pub replay: Option<Reader>,
    playing: bool,
    rec: Option<Rec>,
    pub rec_dir: Option<PathBuf>,
}

impl App {
    /// A viewer over an empty store; `ctl` carries control lines to the agents.
    #[allow(clippy::cast_precision_loss, reason = "a CPU count")]
    pub fn new(opts: Opts, ctl: Sender<String>) -> Self {
        let env = procfs::env("/proc".into(), PathBuf::new());
        let ncpu = thread::available_parallelism().map_or(1, usize::from) as f64;
        let stat = (Files::new(1), Slot::Empty, [env.page_size, env.clk_tck.max(1)]);
        let (at, tui, count, rate) = ([now_ms(), 0], [f64::NAN; 2], [0; 3], [0.0; 3]);
        let mon = Mon { stat, at, tui, count, rate, host: [f64::NAN, f64::NAN, ncpu] };
        let layout = if opts.local { Layout::Node } else { Layout::Step };
        let (toggled, filter, local) = (HashSet::new(), String::new(), opts.local);
        let view =
            View { layout, sort: 1, desc: true, toggled, filter, rules: vec![], window: 16, local };
        let (st, log, now) = (State::new(), VecDeque::new(), now_ms());
        let (cursor, sel, scroll, marks, overlay, ticks, vals) = (0, None, 0, vec![], 0, 0, vec![]);
        Self {
            st,
            view,
            opts,
            ctl,
            cursor,
            sel,
            scroll,
            marks,
            overlay,
            log,
            expect: 1,
            ended: false,
            budget: None,
            mon,
            now,
            ticks,
            vals,
            replay: None,
            playing: false,
            rec: None,
            rec_dir: None,
        }
    }

    /// Sends the `--rule` rules or the §6.3 defaults and the byte budget for `nodes` agents.
    pub fn start(&mut self, nodes: u32) {
        let (iv, rules) = (self.opts.interval_ms, self.opts.rules.clone());
        let rules = if rules.is_empty() { default_rules(nodes, iv) } else { rules };
        self.expect = nodes.max(1);
        let budget = (self.budget.unwrap_or(4_194_304) / u64::from(self.expect)).max(4096);
        self.send(&Control::Budget(TaskSet::All, budget));
        for r in &rules {
            self.send(&Control::Rule(r.clone()));
        }
        self.view.rules = rules;
    }

    /// Encodes and sends one control line, counting it for the header's `out` rate.
    fn send(&mut self, c: &Control) {
        let mut s = String::new();
        wire::write_control(&mut s, c);
        self.mon.count[2] += s.len() as u64;
        let _ = self.ctl.send(s);
    }

    /// Keeps the last 200 log lines (stderr, warnings, junk) for the help pane and footer.
    fn log(&mut self, s: String) {
        self.log.push_front(s);
        self.log.truncate(200);
    }

    /// Decodes whole lines into the store. A bad line counts as a node error and asks that
    /// node for a resync, at most once per 10 s (§7.4). A recording takes a node's tick before
    /// the next tick's first line, structure or `T`, changes it.
    pub fn ingest(&mut self, buf: &[u8]) {
        self.mon.count[0] += buf.len() as u64;
        for line in buf.split_inclusive(|&b| b == b'\n') {
            self.mon.count[1] += 1;
            let Some((node, rest)) = wire::split_node(line).filter(|l| l.0 < 1 << 16) else {
                self.log(format!("junk: {}", String::from_utf8_lossy(line).trim_end()));
                continue;
            };
            let nodes = &mut self.st.nodes;
            nodes.resize_with(nodes.len().max(node as usize + 1), Default::default);
            let (ok, warn) = match wire::parse(rest, &mut self.vals) {
                Some(Event::Warn(w)) => (true, Some(format!("node {node}: {w}"))),
                Some(ev @ Event::Hello { proto, .. }) if proto != wire::PROTO => {
                    let w = format!("node {node}: proto {proto}, need {}: ignored", wire::PROTO);
                    (self.st.apply(node, &ev, self.now).is_ok(), Some(w))
                }
                Some(ev) => {
                    let tick = matches!(ev, Event::Tick { .. });
                    let rec = self.rec.as_mut().filter(|_| !ev.is_value());
                    self.ticks += u64::from(tick);
                    let err = rec.and_then(|r| r.node(&self.st, node).err());
                    (self.st.apply(node, &ev, self.now).is_ok(), err)
                }
                None => (false, None),
            };
            let n = &mut self.st.nodes[node as usize];
            (n.lines, n.errors) = (n.lines + 1, n.errors + u64::from(!ok));
            if !ok && self.now.saturating_sub(n.asked) >= 10_000 {
                n.asked = self.now;
                self.send(&Control::Resync(TaskSet::Ranges(vec![(node, node)])));
            }
            if let Some(w) = warn {
                self.log(w);
            }
        }
    }

    /// The 1 Hz tick: self-monitoring sample, per-node line rates and the PIDS fallback.
    #[allow(clippy::cast_precision_loss, reason = "display values")]
    fn tick(&mut self) {
        let (now, m) = (now_ms(), &mut self.mon);
        let dt = (now.saturating_sub(m.at[0]) as f64 / 1000.0).max(0.001);
        let (files, slot, [page, clk]) = &mut m.stat;
        let stat = files.read(slot, true, Path::new("/proc"), "self/stat").ok();
        if let Some(s) = stat.and_then(procfs::stat) {
            let us = (s.utime + s.stime) * 1_000_000 / *clk;
            m.tui = [us.saturating_sub(m.at[1]) as f64 / dt / 1e4, (s.rss * *page) as f64];
            m.at[1] = us;
        }
        m.at[0] = now;
        for (r, c) in m.rate.iter_mut().zip(&mut m.count) {
            (*r, *c) = (*c as f64 / dt, 0);
        }
        for n in &mut self.st.nodes {
            (n.agent[3], n.lines) = (n.lines as f64 / dt, 0);
        }
        if self.opts.local {
            let info = fs::read("/proc/meminfo").unwrap_or_default();
            let kb = |k| kv(&info, k).map_or(f64::NAN, |v| v as f64 * 1024.0);
            (m.host[0], m.host[1]) = (kb("MemTotal") - kb("MemAvailable"), kb("MemTotal"));
        }
        self.now = now;
        if let Some(at) = self.replay.as_ref().filter(|_| self.playing).map(|r| r.at + 1) {
            self.seek(|_| at);
        }
        self.now = self.replay.as_ref().map_or(now, Reader::now);
        self.st.derive();
    }

    /// Moves the replay cursor to the tick `to` picks.
    fn seek(&mut self, to: impl FnOnce(&Reader) -> usize) {
        let Some(r) = self.replay.as_mut() else { return };
        let k = to(r);
        self.playing &= k < r.times.len();
        if let Err(e) = r.goto(&mut self.st, k) {
            self.log(e);
        }
        self.now = self.replay.as_ref().map_or(self.now, Reader::now);
        self.st.derive();
    }

    /// Replay keys (§9.3); false for any other key.
    fn replay_key(&mut self, code: KeyCode) -> bool {
        let jump = |r: &Reader, d: i64| r.find(r.now().saturating_add_signed(d));
        match code {
            _ if self.replay.is_none() => return false,
            KeyCode::Char(' ') => self.playing = !self.playing,
            KeyCode::Char(',') => self.seek(|r| r.at.saturating_sub(1)),
            KeyCode::Char('.') => self.seek(|r| r.at + 1),
            KeyCode::Char('<') => self.seek(|r| jump(r, -60_000)),
            KeyCode::Char('>') => self.seek(|r| jump(r, 60_000)),
            KeyCode::Char('g') => self.seek(|_| 0),
            KeyCode::Char('G') => self.seek(|r| r.times.len()),
            _ => return false,
        }
        true
    }

    /// `r`: starts recording into the `--record` DIR (first time) or `unjam-rec-<secs>`, or
    /// stops and closes the journals (§8.5).
    pub fn record(&mut self) {
        if let Some(r) = self.rec.take() {
            let dir = r.dir.display().to_string();
            r.finish(&self.st).into_iter().for_each(|e| self.log(e));
            return self.log(format!("recording stopped: {dir}"));
        }
        let dir =
            self.rec_dir.take().unwrap_or_else(|| format!("unjam-rec-{}", now_ms() / 1000).into());
        match Rec::new(dir, &self.opts.source, self.opts.interval_ms) {
            Ok(r) => (self.log(format!("recording to {}", r.dir.display())), self.rec = Some(r)).1,
            Err(e) => self.log(e),
        }
    }

    /// Handles one message; true if the screen must be redrawn at once (keys, resize).
    fn handle(&mut self, m: Msg) -> bool {
        let now = matches!(m, Msg::Key(_) | Msg::Resize | Msg::End);
        match m {
            Msg::Key(k) => self.on_key(k),
            Msg::Lines(b) => self.ingest(&b),
            Msg::Stderr(s) => self.log(s),
            Msg::End => {
                (self.ended, self.now) = (true, now_ms());
                self.log("srun ended: the job is over or every agent has gone".into());
                QUIT.fetch_or(self.st.nodes.is_empty(), Relaxed);
            }
            Msg::Tick => self.tick(),
            Msg::Resize => {}
        }
        now
    }

    /// Moves the cursor by `d` rows; it then stays on its index until the next key.
    fn go(&mut self, d: isize) {
        (self.cursor, self.sel) = (self.cursor.saturating_add_signed(d), None);
    }

    /// Key bindings (§9.3): filter editing, overlays and navigation here, commands in `act`.
    fn on_key(&mut self, k: KeyEvent) {
        match (self.overlay, k.code) {
            _ if k.kind == KeyEventKind::Release => return,
            (3, KeyCode::Char(c)) => return self.view.filter.push(c),
            (3, KeyCode::Backspace) => {
                self.view.filter.pop();
                return;
            }
            (3, KeyCode::Esc) => return (self.overlay, self.view.filter) = (0, String::new()),
            (1..=3, c) if c != KeyCode::Char('q') => return self.overlay = 0,
            (_, c) if self.replay_key(c) => return,
            _ => {}
        }
        let (at, rows) = follow(&self.st, &self.view, self.sel, self.cursor.saturating_add(1));
        self.cursor = at.unwrap_or(self.cursor);
        let row = rows.get(self.cursor).map(|r| (r.key, r.open, target(&self.st, r)));
        drop(rows);
        match (k.code, row) {
            (KeyCode::Up | KeyCode::Char('k'), _) => self.go(-1),
            (KeyCode::Down | KeyCode::Char('j'), _) => self.go(1),
            (KeyCode::PageUp, _) => self.go(-20),
            (KeyCode::PageDown, _) => self.go(20),
            (KeyCode::Home | KeyCode::Char('g'), _) => self.go(isize::MIN),
            (KeyCode::End | KeyCode::Char('G'), _) => self.go(isize::MAX),
            (KeyCode::Left | KeyCode::Right | KeyCode::Char('h' | 'l'), Some((key, open, _))) => {
                let unfold = matches!(k.code, KeyCode::Right | KeyCode::Char('l'));
                if open.is_some_and(|o| o != unfold) && !self.view.toggled.remove(&key) {
                    self.view.toggled.insert(key);
                }
            }
            (KeyCode::Char(' '), Some((key, _, Some((tasks, pat))))) => {
                match self.marks.iter().position(|m| m.0 == key) {
                    Some(i) => drop(self.marks.remove(i)),
                    None => self.marks.push((key, tasks, pat)),
                }
            }
            (code, row) => self.act(code, row.and_then(|r| r.2)),
        }
    }

    /// Commands: quit, overlays, rules (`p`, `+`, `-`, `0`), layout, sort, filter, theme.
    fn act(&mut self, code: KeyCode, row: Option<Target>) {
        let (sort, local) = (self.view.sort, self.opts.local);
        match code {
            KeyCode::Char('q') => QUIT.store(true, Relaxed),
            KeyCode::Enter => self.overlay = 1,
            KeyCode::Char('?') => self.overlay = 2,
            KeyCode::Char('/') => (self.overlay, self.view.filter) = (3, String::new()),
            KeyCode::Esc => self.view.filter.clear(),
            KeyCode::Char('a') => self.marks.clear(),
            KeyCode::Char(c @ ('p' | '+' | '-')) => self.retarget(row, c),
            KeyCode::Char('0') => {
                self.send(&Control::Clear(TaskSet::All));
                (self.opts.rules, self.marks) = (vec![], vec![]);
                self.start(self.expect);
            }
            KeyCode::Char('v') => {
                self.view.layout = match self.view.layout {
                    Layout::Node => Layout::Flat,
                    Layout::Flat if !local => Layout::Step,
                    _ => Layout::Node,
                };
            }
            KeyCode::Char('s') => {
                let s = (sort + 1) % (SORTS.len() - usize::from(local));
                (self.view.sort, self.view.desc) = (s, s != 0 && s <= NCOL);
            }
            KeyCode::Char('S') => self.view.desc = !self.view.desc,
            KeyCode::Char('t') => self.opts.theme = (self.opts.theme + 1) % THEMES.len(),
            KeyCode::Char('r') if self.replay.is_none() => self.record(),
            _ => {}
        }
    }

    /// Steps the effective `(interval, level)` of the marked rows (or the cursor row) and sends
    /// the resulting `R` lines: `p` cycles p → c → o → p, `+` halves and `-` doubles the
    /// interval; off restarts at `p` (§6.3).
    fn retarget(&mut self, row: Option<Target>, key: char) {
        let mut targets: Vec<_> = self.marks.iter().map(|m| (m.1.clone(), m.2.clone())).collect();
        targets.extend(row.filter(|_| self.marks.is_empty()));
        for (tasks, pattern) in targets {
            let (iv, what) = level(&self.view.rules, &tasks, &pattern);
            let (iv, what) = match (key, what) {
                ('p', What::Procs) => (iv, What::Cg),
                ('p', What::Cg) => (0, What::Off),
                (_, What::Off) => (self.opts.interval_ms, What::Procs),
                ('+', w) => (iv / 2, w),
                (_, w) => (iv * 2, w),
            };
            let interval_ms = if what == What::Off { 0 } else { model::interval_ms(iv.into()) };
            let r = Rule { tasks, pattern: pattern.into(), interval_ms, what };
            upsert(&mut self.view.rules, r.clone());
            self.send(&Control::Rule(r));
        }
    }

    /// Whether node `i` sent no `T` for 3 × its longest active interval (§7.4), or srun ended.
    /// Its newest `.` off rule shadows all older ones; with no active rule its agent only
    /// rescans, every `OFF_WALK_MS`.
    fn stale(&self, i: u32) -> bool {
        let mine = self.view.rules.iter().rev().filter(|r| r.tasks.contains(i));
        let live = mine.take_while(|r| r.what > What::Off || &*r.pattern != ".");
        let longest = live.filter(|r| r.what > What::Off).map(|r| r.interval_ms.max(1000)).max();
        let limit = 3 * longest.map_or(OFF_WALK_MS, u64::from);
        let n = self.st.nodes.get(i as usize);
        self.ended || n.is_some_and(|n| self.now.saturating_sub(n.seen) > limit)
    }
}

/// The rule target of a row: all agents for cross-node rows, the row's node otherwise.
fn target(st: &State, r: &Row) -> Option<Target> {
    let c = st.nodes.get(r.node as usize)?.cgs.get(&r.id)?;
    let path = if c.path.is_empty() { ".".into() } else { c.path.to_string() };
    match r.key {
        Key::Job => Some((TaskSet::All, ".".into())),
        Key::Step(_) => Some((TaskSet::All, path)),
        Key::Cg(i, _) => Some((TaskSet::Ranges(vec![(i, i)]), path)),
        Key::Proc(..) | Key::Folded(_) => None,
    }
}

/// Rows up to `n`, or all when the cursor's entity `k` lies beyond them after a re-sort
/// (D39), and `k`'s index: draw and keys both act on that entity, not on the index.
fn follow<'a>(st: &'a State, v: &View, k: Option<Key>, n: usize) -> (Option<usize>, Vec<Row<'a>>) {
    let mut rows = state::rows(st, v, n);
    if k.is_some_and(|k| rows.len() == n && rows.iter().all(|r| r.key != k)) {
        rows = state::rows(st, v, usize::MAX);
    }
    (k.and_then(|k| rows.iter().position(|r| r.key == k)), rows)
}

/// The effective rule of a target as the agents resolve it (node 0 for `*`).
fn level(rules: &[Rule], tasks: &TaskSet, pattern: &str) -> (u32, What) {
    let task = if let TaskSet::Ranges(r) = tasks { r[0].0 } else { 0 };
    effective(rules, task, if pattern == "." { "" } else { pattern })
}

/// The §6.3 defaults: processes everywhere for small jobs; cgroups at half rate and task
/// subtrees off for jobs over 16 nodes.
pub fn default_rules(nodes: u32, interval_ms: u32) -> Vec<Rule> {
    let big = [format!("*\t.\t{}\tc", interval_ms * 2), "*\t*/user\t-\to".into()];
    let text = if nodes > 16 { &big[..] } else { &[format!("*\t.\t{interval_ms}\tp")][..] };
    text.iter().filter_map(|t| Rule::parse(t).ok()).collect()
}

/// Runs a viewer: the ticker, then `--batch` snapshots or the terminal with its input thread.
pub fn view(
    app: &mut App,
    tx: &SyncSender<Msg>,
    rx: &Receiver<Msg>,
    batch: Option<u32>,
) -> io::Result<()> {
    let (tick, keys) = (tx.clone(), tx.clone());
    thread::spawn(move || {
        while tick.send(Msg::Tick).is_ok() {
            thread::sleep(Duration::from_secs(1));
        }
    });
    if let Some(mut left) = batch {
        let (mut seen, out) = (0, &mut io::stdout().lock());
        while let Some(m) = (left > 0).then(|| rx.recv().ok()).flatten() {
            let tick = matches!(m, Msg::Tick);
            app.handle(m);
            if tick && app.ticks > seen {
                (seen, left) = (app.ticks, left - 1);
                snapshot(app, out)?;
            }
            left *= u32::from(!app.ended);
        }
    } else {
        let (ktx, krx) = mpsc::channel();
        thread::spawn(move || input(&ktx, &keys));
        for sig in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGHUP] {
            signal_hook::flag::register(sig, Arc::clone(&QUIT))?;
        }
        let mut term = ratatui::try_init()?;
        let res = event_loop(app, [rx, &krx], &mut term);
        ratatui::restore();
        res?;
    }
    if app.rec.is_some() {
        app.record();
    }
    Ok(())
}

/// The input thread (D38): blocks on terminal events; `q`, Ctrl-C (or a dead terminal) set the
/// quit flag at once. Keys go to the unbounded `keys` queue and only wake the main thread, so
/// they never wait behind a full data channel.
fn input(keys: &Sender<Msg>, wake: &SyncSender<Msg>) {
    while let Ok(ev) = event::read() {
        let msg = match ev {
            Term::Key(k) if k == KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL) => break,
            Term::Key(k) if k.code == KeyCode::Char('q') && !EDIT.load(Relaxed) => break,
            Term::Key(k) => Msg::Key(k),
            Term::Resize(..) => Msg::Resize,
            _ => continue,
        };
        if keys.send(msg).is_err() {
            return;
        }
        let _ = wake.try_send(Msg::Resize);
    }
    QUIT.store(true, Relaxed);
}

/// The main loop: block on the data channel `rx[0]`, handle the keys queued in `rx[1]` first,
/// decode for at most 10 ms per batch, redraw at once after keys and at most at 4 Hz after data
/// (§7.3, §9.4).
fn event_loop(app: &mut App, rx: [&Receiver<Msg>; 2], t: &mut DefaultTerminal) -> io::Result<()> {
    let (frame, mut drawn, mut dirty) = (Duration::from_millis(250), None::<Instant>, true);
    while !QUIT.load(Relaxed) {
        if dirty && drawn.is_none_or(|d| d.elapsed() >= frame) {
            app.now = app.replay.as_ref().map_or_else(now_ms, Reader::now);
            t.draw(|f| draw(f, app))?;
            (drawn, dirty) = (Some(Instant::now()), false);
        }
        let wait = drawn.map_or(Duration::ZERO, |d| frame.saturating_sub(d.elapsed()));
        let mut now = match rx[0].recv_timeout(if dirty { wait } else { frame * 4 }) {
            Ok(m) => rx[1].try_iter().fold(false, |n, k| app.handle(k) | n) | app.handle(m),
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        let t0 = Instant::now();
        while let Some(m) =
            (t0.elapsed() < Duration::from_millis(10)).then(|| rx[0].try_recv().ok()).flatten()
        {
            now |= app.handle(m);
        }
        (dirty, drawn) = (true, drawn.filter(|_| !now));
        EDIT.store(app.overlay == 3, Relaxed);
    }
    Ok(())
}

/// Human-readable value: `%` with one decimal, bytes and B/s with binary suffixes, counts with
/// decimal ones; NA is `-`, and an unknown unit is shown raw after the number.
pub fn fmt(unit: &str, v: f64) -> String {
    let (step, sfx) = match unit {
        _ if v.is_nan() => return "-".into(),
        "%" => return format!("{v:.1}%"),
        "B" | "B/s" => (1024.0, "KMGTP"),
        "1" => (1000.0, "kMGTP"),
        _ => return format!("{v:.0} {unit}"),
    };
    let (mut x, mut i) = (v, 0);
    while x >= step && i < sfx.len() {
        (x, i) = (x / step, i + 1);
    }
    let (s, per) = (&sfx[i.saturating_sub(1)..i], if unit == "B/s" && i > 0 { "/s" } else { "" });
    if i > 0 && x < 10.0 { format!("{x:.1}{s}{per}") } else { format!("{x:.0}{s}{per}") }
}

/// `2s` or `500ms`; `-` for off.
fn interval(ms: u32, what: What) -> String {
    match what {
        What::Off => "-".into(),
        _ if ms.is_multiple_of(1000) => format!("{}s", ms / 1000),
        _ => format!("{ms}ms"),
    }
}

/// Which value columns to show: CPU% always, the others unless NA on every given row (§9.2).
fn shown(rows: &[Row]) -> Vec<usize> {
    (0..NCOL).filter(|&c| c == 0 || rows.iter().any(|r| !r.v[c].is_nan())).collect()
}

/// The two header lines: source and view, then the self-monitoring line (D40).
fn title(app: &App) -> [String; 2] {
    let (nodes, m, local) = (&app.st.nodes, &app.mon, app.opts.local);
    let procs: usize = nodes.iter().map(|n| n.procs.len()).sum();
    let stale = (0..).zip(nodes).filter(|&(i, _)| app.stale(i)).count();
    let (seen, all) = (nodes.len(), (app.expect as usize).max(nodes.len()));
    let src = match nodes.first() {
        Some(n) if local => format!("unjam  {}", n.host),
        _ if app.ended => format!("unjam {}  {seen}/{all} nodes (ended)", app.opts.source),
        _ => format!("unjam {}  {seen}/{all} nodes ({stale} stale)", app.opts.source),
    };
    let layout = ["step", if local { "tree" } else { "node" }, "flat"][app.view.layout as usize];
    let (iv, s, sort) =
        (interval(app.opts.interval_ms, What::Cg), app.now / 1000 % 86_400, SORTS[app.view.sort]);
    let arrow = if app.view.desc { '▼' } else { '▲' };
    let clock = format!("{:02}:{:02}:{:02}Z", s / 3600, s / 60 % 60, s % 60);
    let mode = match (&app.replay, &app.rec) {
        (Some(_), _) => ["  [⏸]", "  [▶]"][usize::from(app.playing)],
        (_, Some(_)) => "  ● rec",
        _ => "",
    };
    let l0 = format!(" {src}{mode}  {procs} procs  {iv}  {layout}  sort {sort} {arrow}  {clock}");
    let (cpu, rss) = (fmt("%", m.tui[0]), fmt("B", m.tui[1]));
    let [inb, msg, out] = [("B/s", 0), ("1", 1), ("B/s", 2)].map(|(u, i)| fmt(u, m.rate[i]));
    let mut l1 = format!(" tui {cpu} {rss} · in {inb} {msg} msg/s · out {out}");
    let top = nodes.iter().max_by(|a, b| a.agent[0].total_cmp(&b.agent[0])).filter(|_| !local);
    if let Some(n) = top {
        let (c, r) = (fmt("%", n.agent[0]), fmt("B", n.agent[1]));
        let _ = write!(l1, " · agents max {c} {r} ({})", n.host);
    }
    let (drop, err) =
        nodes.iter().fold((0, 0), |(d, e), n| (d + u64::from(n.dropped), e + n.errors));
    let _ = write!(l1, " · drop {drop} err {err}");
    [l0, l1]
}

/// The text cells of a row: indent plus fold glyph or guide, name, NODE, PID, INT W, then the
/// value columns (PIDS fallback marked `~`).
fn texts(app: &App, r: &Row) -> [String; 5 + NCOL] {
    let gl = &THEMES[app.opts.theme].glyphs;
    let glyph = match r.open {
        Some(o) => [gl.closed, gl.open][usize::from(o)],
        None if matches!(r.key, Key::Proc(..)) => gl.guide[usize::from(r.last)],
        None => "  ",
    };
    let own = matches!(r.key, Key::Cg(..) | Key::Proc(..));
    let mut out: [String; 5 + NCOL] = Default::default();
    out[0] = format!("{:w$}{glyph}", "", w = usize::from(r.depth) * 2);
    out[1] = r.name.to_string();
    out[2] = if own { app.st.nodes[r.node as usize].host.clone() } else { String::new() };
    out[3] = if let Key::Proc(_, p) = r.key { p.to_string() } else { String::new() };
    if let Some((tasks, pat)) = target(&app.st, r).filter(|_| !app.view.rules.is_empty()) {
        let (iv, what) = level(&app.view.rules, &tasks, &pat);
        out[4] = format!("{:>5} {}", interval(iv, what), ["o", "c", "p"][what as usize]);
    }
    for (c, (_, unit, _)) in COLS.iter().enumerate() {
        let approx = if c == 4 && r.approx && !r.v[c].is_nan() { "~" } else { "" };
        out[5 + c] = format!("{approx}{}", fmt(unit, r.v[c]));
    }
    out
}

/// Heat band of a CPU per mille of one core: idle, low, high, saturated.
fn band(pm: f64) -> usize {
    [50.0, 500.0, 900.0].iter().filter(|&&b| pm >= b).count()
}

/// A history bar: one glyph per sample, height relative to the row's peak (at least one core).
fn bar(t: &Theme, hist: &[u16]) -> Vec<Span<'static>> {
    let scale = u32::from(hist.iter().copied().max().unwrap_or(0).max(1000));
    let span = |h: u16| {
        let lvl = if h == 0 { 0 } else { 1 + (u32::from(h) * 7 / scale).min(7) as usize };
        let style = if h == 0 { t.dim } else { Style::new().fg(t.heat[band(f64::from(h))]) };
        Span::styled(t.glyphs.bars[lvl].to_string(), style)
    };
    hist.iter().map(|&h| span(h)).collect()
}

/// Draws one frame: header, heat strip or host bars, table, footer and any overlay.
pub fn draw(f: &mut Frame, app: &mut App) {
    let (t, area) = (&THEMES[app.opts.theme], f.area());
    f.render_widget(Block::new().style(t.base), area);
    let sizes =
        [1, 1, 1, 0, 1].map(|h| if h == 0 { Constraint::Fill(1) } else { Constraint::Length(h) });
    let [head, mon, strip, body, foot] = Split::vertical(sizes).areas(area);
    let ([l0, l1], err) = (title(app), app.st.nodes.iter().any(|n| n.errors > 0));
    f.render_widget(Paragraph::new(l0).style(t.header), head);
    f.render_widget(Paragraph::new(l1).style(if err { t.crit } else { t.dim }), mon);
    f.render_widget(Paragraph::new(Line::from(strip_line(app, t, strip.width))), strip);
    let height = usize::from(body.height.saturating_sub(1));
    app.view.window = height.saturating_sub(4).max(4);
    let limit = app.cursor.saturating_add(1).max(app.scroll + height);
    let (at, rows) = follow(&app.st, &app.view, app.sel, limit);
    app.cursor = at.unwrap_or(app.cursor).min(rows.len().saturating_sub(1));
    app.sel = rows.get(app.cursor).map(|r| r.key);
    app.scroll = app.scroll.min(app.cursor).max((app.cursor + 1).saturating_sub(height));
    table(f, app, t, &rows[app.scroll.min(rows.len())..rows.len().min(app.scroll + height)], body);
    let hint = app.log.front().map_or(String::new(), |l| format!(" │ {l}"));
    let filter = app.overlay == 3 || !app.view.filter.is_empty();
    let keys = if app.replay.is_some() { REPLAY } else { HINTS };
    let text = if filter { format!(" /{}▏", app.view.filter) } else { format!(" {keys}{hint}") };
    f.render_widget(Paragraph::new(text).style(t.dim.bg(t.header.bg.unwrap_or_default())), foot);
    let (w, h) = (area.width, area.height);
    let pop = Rect { x: w / 10, y: h / 10, width: w * 4 / 5, height: h * 4 / 5 };
    let lines = match app.overlay {
        1 => rows.get(app.cursor).map(|r| detail(app, r)),
        2 => Some(help(app)),
        _ => None,
    };
    if let Some(lines) = lines {
        f.render_widget(Clear, pop);
        let block = Block::bordered().style(t.base).border_style(t.tree);
        f.render_widget(Paragraph::new(lines.join("\n")).block(block), pop);
    }
}

/// Local mode: host CPU history and memory gauge (nord-host mock); Slurm mode: the node heat
/// strip, one cell per ⌈N / width⌉ nodes, dotted where no node has values (D41).
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "clamped bar widths")]
fn strip_line(app: &App, t: &Theme, width: u16) -> Vec<Span<'static>> {
    let (nodes, [used, total, ncpu]) = (&app.st.nodes, app.mon.host);
    let glyph =
        |i: usize, style: Style, n| Span::styled(t.glyphs.bars[i].to_string().repeat(n), style);
    let heat = |pm: f64| Style::new().fg(t.heat[band(pm)]);
    if !app.opts.local {
        let per = nodes.len().div_ceil(usize::from(width.saturating_sub(7)).max(1)).max(1);
        let cpu = |n: &state::Node| n.root_vals()?.get(0, Kind::Counter);
        let max = |c: &[state::Node]| c.iter().filter_map(cpu).max_by(f64::total_cmp);
        let cells = nodes
            .chunks(per)
            .map(|c| max(c).map_or(glyph(0, t.dim, 1), |us| glyph(8, heat(us / 1e3), 1)));
        return [Span::styled(" NODES ", t.dim)].into_iter().chain(cells).collect();
    }
    let v = nodes.first().and_then(state::Node::root_vals);
    let cpu = v.and_then(|v| v.get(0, Kind::Counter)).map_or(f64::NAN, |us| us / 1e4 / ncpu);
    let hist = v.map_or([0; HIST], state::Vals::hist).map(|h| (f64::from(h) / ncpu) as u16);
    let used = v.and_then(|v| v.get(1, Kind::Gauge)).unwrap_or(used);
    let full = if total > 0.0 { (used / total * 16.0).clamp(0.0, 16.0) as usize } else { 0 };
    let mut spans = vec![Span::styled(" CPU ", t.dim)];
    spans.extend(bar(t, &hist));
    let mem = [glyph(8, heat(0.0), full), glyph(0, t.dim, 16 - full)];
    spans.extend([Span::raw(format!(" {}  ", fmt("%", cpu))), Span::styled("MEM ", t.dim)]);
    spans.extend(mem);
    spans.push(Span::raw(format!(" {}/{}", fmt("B", used), fmt("B", total))));
    spans
}

/// The table for the visible rows only; all-NA value columns are hidden for this frame.
fn table(f: &mut Frame, app: &App, t: &Theme, vis: &[Row], area: Rect) {
    let (cols, hw) = (shown(vis), if area.width >= 100 { 16 } else { 8 });
    let node = app.view.layout == Layout::Flat && !app.opts.local;
    let cpu = if hw == 16 { "CPU history" } else { "CPU" };
    let mut head = vec![("NAME", 0), ("NODE", 8), ("PID", 7), (cpu, hw)];
    head.retain(|h| node || h.0 != "NODE");
    head.extend(cols.iter().map(|&c| (COLS[c].0, COLS[c].2)).chain([("  INT W", 7)]));
    let widths =
        head.iter().map(|h| if h.1 == 0 { Constraint::Fill(1) } else { Constraint::Length(h.1) });
    let rows = vis.iter().enumerate().map(|(i, r)| {
        let mut tx = texts(app, r).map(Some);
        let mut take = |i: usize| tx[i].take().unwrap_or_default();
        let marked = app.marks.iter().any(|m| m.0 == r.key);
        let look = [t.accent, t.base, t.marked]
            [if marked { 2 } else { usize::from(matches!(r.key, Key::Proc(..))) }];
        let mut cells = vec![Cell::from(Line::from(vec![
            Span::styled(take(0), t.tree),
            Span::styled(take(1), look),
        ]))];
        cells.extend(node.then(|| Cell::from(take(2))));
        cells.push(Cell::from(Line::from(take(3)).right_aligned()).style(t.dim));
        cells.push(Cell::from(Line::from(bar(t, &r.hist[HIST - usize::from(hw)..]))));
        cells.extend(cols.iter().map(|&c| Cell::from(Line::from(take(5 + c)).right_aligned())));
        cells.push(Cell::from(take(4)));
        let stale = matches!(r.key, Key::Cg(..) | Key::Proc(..)) && app.stale(r.node);
        let at = if app.scroll + i == app.cursor { 2 } else { usize::from(stale) };
        let look = [Style::new(), t.warn, t.selected][at];
        TRow::new(cells).style(look)
    });
    let header = TRow::new(head.iter().map(|h| Cell::from(h.0))).style(t.header);
    f.render_widget(Table::new(rows, widths).header(header).column_spacing(1).style(t.base), area);
}

/// Detail popup lines: every column the node announced, raw and as shown, plus path or cmdline.
fn detail(app: &App, r: &Row) -> Vec<String> {
    let n = &app.st.nodes[r.node as usize];
    let found = match r.key {
        Key::Cg(_, id) => n.cgs.get(&id).map(|c| (0, &c.vals, format!("cgroup /{}", c.path))),
        Key::Proc(_, pid) => {
            n.procs.get(&pid).map(|p| (1, &p.vals, format!("pid {pid} {}: {}", p.comm, p.cmd)))
        }
        _ => None,
    };
    let Some((ent, vals, what)) = found else {
        return vec![" cross-node row: sums over its nodes".into()];
    };
    let mut out = vec![format!(" {what}  ({})", n.host), String::new()];
    for (c, (name, unit, kind)) in app.st.cols.0[ent].iter().enumerate() {
        let (raw, shown) =
            (vals.v.get(c).copied().unwrap_or(NA), vals.get(c, *kind).unwrap_or(f64::NAN));
        let per = if *kind == Kind::Counter { "/s" } else { "" };
        out.push(match raw {
            NA => format!(" {name:<26} NA (not announced, or the controller or file is absent)"),
            _ => format!(" {name:<26} {raw:>16} {unit}   {}{per}", fmt(unit, shown)),
        });
    }
    out
}

/// Help pane: bindings, the per-node self-monitoring table and the log tail.
fn help(app: &App) -> Vec<String> {
    let keys = " Enter detail  a clear marks  0 reset rules  S reverse sort  Esc close";
    let mut out = vec![format!(" {HINTS}"), keys.into(), String::new()];
    out.push(format!(" {:<16}     CPU     RSS     OUT/s   MSG/s  DROP   ERR SKEW ms", "NODE"));
    for n in &app.st.nodes {
        let [c, r, o, m] =
            [("%", 0), ("B", 1), ("B/s", 2), ("1", 3)].map(|(u, i)| fmt(u, n.agent[i]));
        let skew = i128::from(n.prev[0]) - i128::from(n.seen);
        out.push(format!(
            " {:<16} {c:>7} {r:>7} {o:>9} {m:>7} {:>5} {:>5} {skew:>7}",
            n.host, n.dropped, n.errors
        ));
    }
    out.push(String::new());
    out.extend(app.log.iter().take(20).map(|l| format!(" {l}")));
    out
}

/// One `--batch` snapshot (`top -b`): header, every row of the current layout as text, and a
/// footer with node, stale, error and drop counts.
pub fn snapshot(app: &App, w: &mut dyn Write) -> io::Result<()> {
    let rows = state::rows(&app.st, &app.view, usize::MAX);
    let (cols, [l0, l1]) = (shown(&rows), title(app));
    let node = |s: &str| if app.opts.local { String::new() } else { format!(" {s:>7}") };
    let line = |out: &mut String, name: &str, tx: [&str; 3], v: &dyn Fn(usize) -> String| {
        let mut l = format!("{name:<44}{} {:>7}", node(tx[0]), tx[1]);
        for &c in &cols {
            let _ = write!(l, " {:>w$}", v(c), w = usize::from(COLS[c].2));
        }
        let _ = writeln!(out, "{}", format!("{l} {}", tx[2]).trim_end());
    };
    let mut out = format!("{l0}\n{l1}\n");
    line(&mut out, "NAME", ["NODE", "PID", "  INT W"], &|c| COLS[c].0.into());
    for r in &rows {
        let tx = texts(app, r);
        line(&mut out, &(tx[0].clone() + &tx[1]), [&tx[2], &tx[3], &tx[4]], &|c| tx[5 + c].clone());
    }
    let (nodes, n) = (&app.st.nodes, app.st.nodes.len());
    let stale = (0..).zip(nodes).filter(|&(i, _)| app.stale(i)).count();
    let errors: u64 = nodes.iter().map(|x| x.errors).sum();
    let dropped: u64 = nodes.iter().map(|x| u64::from(x.dropped)).sum();
    writeln!(w, "{out}nodes={n} stale={stale} errors={errors} dropped={dropped}\n")
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::mpsc;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    use super::*;

    /// One GiB for host memory.
    const GIB: f64 = 1_073_741_824.0;

    /// Midday, so the header clock is stable.
    const NOON: u64 = 43_200_000;

    /// A viewer with `theme` and the receiving end of its control channel.
    fn app(local: bool, theme: &str) -> (App, Receiver<String>) {
        let (ctl, rx) = mpsc::channel();
        let theme = THEMES.iter().position(|t| t.name == theme).unwrap();
        let opts = Opts { source: "42".into(), interval_ms: 1000, rules: vec![], theme, local };
        let mut app = App::new(opts, ctl);
        (app.now, app.mon.host) = (NOON, [3.0 * GIB, 8.0 * GIB, 4.0]);
        app.start(1);
        rx.try_iter().for_each(drop);
        (app, rx)
    }

    /// Feeds space-separated lines (`~` is a space inside a field) as node `node`'s output.
    fn feed(app: &mut App, node: u32, lines: &[String]) {
        let mut buf = Vec::new();
        for l in lines {
            let fields: Vec<String> = l.split(' ').map(|f| f.replace('~', " ")).collect();
            buf.extend_from_slice(format!("{node}\t{}\n", fields.join("\t")).as_bytes());
        }
        app.ingest(&buf);
    }

    /// One host: two slices, a service with sshd and a shell, 17 ticks of rising load.
    fn host(app: &mut App, node: u32, name: &str, load: u64) {
        let head = [
            &format!("H 1 0.1 {name} 42.0 1 0"),
            "D c 0 cg_cpu_usage_usec us counter",
            "D c 1 cg_mem_current_bytes B gauge",
            "D c 2 cg_pids_current 1 gauge",
            "D p 0 p_cpu_user_usec us counter",
            "D p 1 p_rss_bytes B gauge",
            "D p 2 p_threads 1 gauge",
            "G 1 0 ",
            "G 2 1 system.slice",
            "G 3 1 user.slice",
            "G 4 2 system.slice/sshd.service",
            "P 10 1 4 1 sshd sshd:~/usr/sbin/sshd~-D",
            "P 20 1 3 1 bash -bash",
        ];
        feed(app, node, &head.map(str::to_owned));
        let mut cpu = [0u64; 5];
        for t in 1..=17u64 {
            for (i, c) in cpu.iter_mut().enumerate() {
                *c += load * t * [40_000, 30_000, 10_000, 25_000, 5_000][i];
            }
            let lines = [
                format!("T {} 0 0 0 -", NOON - 17_000 + t * 1000),
                format!("g 1 0 {} {} -", cpu[0], 3u64 << 30),
                format!("g 2 0 {} {} 12", cpu[1], 1u64 << 30),
                format!("g 3 0 {} {} 3", cpu[2], 1 << 29),
                format!("g 4 0 {} {} 2", cpu[1], 1 << 25),
                format!("p 10 0 {} 8388608 3", cpu[3]),
                format!("p 20 0 {} 4194304 1", cpu[4]),
            ];
            feed(app, node, &lines);
        }
        app.st.derive();
    }

    /// The buffer as text lines, trailing blanks cut.
    fn text(b: &Buffer) -> String {
        let w = usize::from(b.area.width);
        let rows = b
            .content
            .chunks(w)
            .map(|r| r.iter().map(ratatui::buffer::Cell::symbol).collect::<String>());
        rows.map(|r| r.trim_end().to_owned() + "\n").collect()
    }

    fn frame(app: &mut App, w: u16, h: u16) -> Buffer {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, app)).unwrap();
        term.backend().buffer().clone()
    }

    /// Compares with `tests/fixtures/golden/<name>`; `UNJAM_BLESS=1` rewrites the file.
    fn golden(name: &str, got: &str) {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/golden").join(name);
        if std::env::var_os("UNJAM_BLESS").is_some() {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, got).unwrap();
        }
        let want = fs::read_to_string(&path).unwrap_or_default();
        assert!(want == got, "{name} differs (UNJAM_BLESS=1 rewrites it):\n{got}");
    }

    #[test]
    fn draw_local_mono() {
        let (mut app, _rx) = app(true, "mono");
        host(&mut app, 0, "vm", 1);
        golden("draw-local-mono.txt", &text(&frame(&mut app, 100, 14)));
        app.overlay = 1;
        golden("draw-detail-mono.txt", &text(&frame(&mut app, 100, 14)));
    }

    #[test]
    fn draw_slurm_narrow() {
        let (mut app, _rx) = app(false, "mono");
        for (i, n) in ["n1", "n2", "n3"].iter().enumerate() {
            host(&mut app, u32::try_from(i).unwrap(), n, 3 - i as u64);
        }
        golden("draw-slurm-mono.txt", &text(&frame(&mut app, 80, 16)));
    }

    #[test]
    fn cursor_follows_its_row_past_the_window() {
        let (mut app, _rx) = app(true, "mono");
        host(&mut app, 0, "vm", 1);
        let all = state::rows(&app.st, &app.view, usize::MAX);
        let (n, last) = (all.len(), (all.len() - 1, Some(all[all.len() - 1].key)));
        drop(all);
        app.sel = last.1;
        frame(&mut app, 100, 8);
        assert!(n > 4);
        assert_eq!((app.cursor, app.sel), last);
    }

    /// Per theme: the styles of the header, the table head, the cursor row, a process name and
    /// one history bar cell.
    #[test]
    fn theme_snapshots() {
        let mut out = String::new();
        for t in THEMES {
            let (mut app, _rx) = app(true, t.name);
            host(&mut app, 0, "vm", 1);
            let b = frame(&mut app, 100, 14);
            let _ = write!(out, "{}:", t.name);
            for (x, y) in [(1, 0), (1, 3), (5, 4), (8, 6), (5, 2)] {
                let c = &b[(x, y)];
                let _ = write!(out, " {}{:?}/{:?}/{:?}", c.symbol(), c.fg, c.bg, c.modifier);
            }
            out.push('\n');
        }
        golden("themes.txt", &out);
    }

    #[test]
    fn batch_snapshot() {
        let (mut app, _rx) = app(true, "nord");
        host(&mut app, 0, "vm", 1);
        let mut out = Vec::new();
        snapshot(&app, &mut out).unwrap();
        golden("batch-local.txt", &String::from_utf8(out).unwrap());
    }

    #[test]
    fn recording_takes_each_tick_before_the_next_ticks_structure() {
        let dir = std::env::temp_dir().join(format!("unjam-tui-rec-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let (mut app, _rx) = app(false, "mono");
        (app.rec_dir, app.now) = (Some(dir.clone()), 0);
        app.record();
        let lines =
            ["H 1 0.1 n1 42.0 1 0", "D p 0 p_rss_bytes B gauge", "G 1 0 ", "T 1000 0 0 0 -"];
        feed(&mut app, 0, &lines.map(str::to_owned));
        let next = ["P 50 1 1 1 sh sh", "T 2000 0 0 0 -", "p 50 0 777", "x 50", "T 3000 0 0 0 -"];
        feed(&mut app, 0, &next.map(str::to_owned));
        app.record();
        let (mut r, warns) = Reader::open(&[dir.join("n1.arrows")]).unwrap();
        assert!(warns.is_empty(), "{warns:?}");
        assert_eq!(r.times, [1000, 2000, 3000]);
        let mut st = State::new();
        let mut rss = || {
            let k = r.at.wrapping_add(1);
            r.goto(&mut st, k).unwrap();
            st.nodes[0].procs.get(&50).map(|p| p.vals.v[2])
        };
        assert_eq!([rss(), rss(), rss()], [None, Some(777), None], "pid 50 lives for tick 2000");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn corrupt_lines_ask_for_one_resync() {
        let (mut app, rx) = app(false, "mono");
        host(&mut app, 0, "n1", 1);
        let bad = ["g 99 0 1".to_owned(), "p 10 7 1".into(), "Q nonsense".into()];
        feed(&mut app, 0, &bad);
        feed(&mut app, 0, &bad);
        app.ingest(b"no tab here\n");
        let sent: Vec<String> = rx.try_iter().collect();
        assert_eq!(sent, ["S\t0\n"]);
        assert_eq!(app.st.nodes[0].errors, 6);
        assert!(app.log.front().unwrap().starts_with("junk: no tab"));
        app.now += 10_000;
        feed(&mut app, 0, &bad[..1]);
        assert_eq!(rx.try_iter().count(), 1, "again after 10 s");
    }

    #[test]
    fn other_proto_is_logged_and_ignored() {
        // A node with another proto used to be blank with no error and no log line.
        let (mut app, rx) = app(false, "mono");
        feed(&mut app, 0, &["H 2 9.0 n1 42.0 1 0".into(), "G 1 0 ".into()]);
        assert_eq!(app.log.front().unwrap(), "node 0: proto 2, need 1: ignored");
        assert!(app.st.nodes[0].cgs.is_empty());
        assert_eq!((app.st.nodes[0].errors, rx.try_iter().count()), (0, 0), "no resync loop");
    }

    #[test]
    fn srun_end_marks_every_node_ended() {
        let (mut app, _rx) = app(false, "mono");
        host(&mut app, 0, "n1", 1);
        app.start(4);
        assert!(title(&app)[0].contains(" 1/4 nodes (0 stale) "), "{}", title(&app)[0]);
        app.handle(Msg::End);
        assert!(app.ended && app.stale(0));
        assert!(title(&app)[0].contains(" 1/4 nodes (ended) "), "{}", title(&app)[0]);
        assert!(app.log.front().unwrap().starts_with("srun ended"));
    }

    #[test]
    fn a_node_with_every_subtree_off_is_stale_only_after_its_rescans() {
        let (mut app, _rx) = app(false, "mono");
        for (i, n) in ["n1", "n2"].iter().enumerate() {
            host(&mut app, u32::try_from(i).unwrap(), n, 1);
        }
        app.now = app.st.nodes[0].seen + 5_000;
        assert!(app.stale(0) && app.stale(1), "1 s ticks: stale after 3 s");
        upsert(&mut app.view.rules, Rule::parse("0\t.\t-\to").unwrap());
        assert!(!app.stale(0) && app.stale(1), "node 0 is off: it reports every 10 s");
        upsert(&mut app.view.rules, Rule::parse("0\tuser.slice\t2000\tc").unwrap());
        assert!(!app.stale(0), "an active subtree again: 3 × 2 s");
        app.now += 2_000;
        assert!(app.stale(0));
        for r in ["*\t.\t-\to", "0\tuser.slice\t-\to"] {
            upsert(&mut app.view.rules, Rule::parse(r).unwrap());
        }
        assert!(!app.stale(0) && !app.stale(1), "everything off: 3 × 10 s");
        app.now += 24_000;
        assert!(app.stale(0) && app.stale(1));
    }

    #[test]
    fn keys_send_rules() {
        let (mut app, rx) = app(false, "mono");
        host(&mut app, 0, "n1", 1);
        app.budget = Some(1 << 20);
        app.start(2);
        let start: Vec<String> = rx.try_iter().collect();
        assert_eq!(start, ["B\t*\t524288\n", "R\t*\t.\t1000\tp\n"]);
        let key = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        app.view.layout = Layout::Node;
        app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        let rows = state::rows(&app.st, &app.view, 9);
        let name = rows[app.cursor].name.to_string();
        assert_eq!(name, "system.slice");
        for c in ['p', '+', '-', '-', 'p', 'p'] {
            app.on_key(key(c));
        }
        let sent: Vec<String> = rx.try_iter().collect();
        assert_eq!(
            sent,
            [
                "R\t0\tsystem.slice\t1000\tc\n",
                "R\t0\tsystem.slice\t500\tc\n",
                "R\t0\tsystem.slice\t1000\tc\n",
                "R\t0\tsystem.slice\t2000\tc\n",
                "R\t0\tsystem.slice\t-\to\n",
                "R\t0\tsystem.slice\t1000\tp\n",
            ]
        );
        app.on_key(key(' '));
        app.on_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        app.on_key(key(' '));
        assert_eq!(app.marks.len(), 2);
        app.on_key(key('0'));
        let sent: Vec<String> = rx.try_iter().collect();
        assert_eq!(sent[..2], ["C\t*\n", "B\t*\t524288\n"], "the job's node count and --budget");
        assert!(app.marks.is_empty() && app.view.rules.len() == 1);
    }

    #[test]
    fn keys_act_on_the_highlighted_row_after_a_resort() {
        let (mut app, rx) = app(false, "mono");
        host(&mut app, 0, "n1", 1);
        app.view.layout = Layout::Node;
        let name = |app: &App| state::rows(&app.st, &app.view, 9)[app.cursor].name.to_string();
        app.cursor = state::rows(&app.st, &app.view, 9)
            .iter()
            .position(|r| r.name == "system.slice")
            .unwrap();
        frame(&mut app, 100, 14);
        let up =
            [format!("T {} 0 0 0 -", NOON + 1000), "g 2 0 4590000".into(), "g 3 0 2430000".into()];
        feed(&mut app, 0, &up);
        app.st.derive();
        assert_ne!(name(&app), "system.slice", "user.slice moved onto the cursor index");
        app.on_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
        assert_eq!(rx.try_iter().collect::<Vec<_>>(), ["R\t0\tsystem.slice\t1000\tc\n"]);
        assert_eq!(name(&app), "system.slice");
    }

    #[test]
    fn layout_sort_theme_and_filter_keys() {
        let (mut app, _rx) = app(true, "nord");
        let key = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        app.on_key(key('v'));
        assert_eq!(app.view.layout, Layout::Flat);
        app.on_key(key('v'));
        assert_eq!(app.view.layout, Layout::Node, "no step layout locally");
        app.on_key(key('s'));
        assert_eq!((app.view.sort, app.view.desc), (2, true));
        app.on_key(key('t'));
        assert_eq!(THEMES[app.opts.theme].name, "night");
        for c in ['/', 's', 's', 'h'] {
            app.on_key(key(c));
        }
        assert_eq!((app.overlay, app.view.filter.as_str()), (3, "ssh"));
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!((app.overlay, app.view.filter.as_str()), (0, "ssh"));
        for _ in 0..4 {
            app.on_key(key('s'));
        }
        assert_eq!((app.view.sort, app.view.desc), (0, false), "no NODE sort locally");
        app.opts.local = false;
        for _ in 0..6 {
            app.on_key(key('s'));
        }
        assert_eq!((SORTS[app.view.sort], app.view.desc), ("NODE", false));
    }

    #[test]
    fn heat_strip_groups_nodes() {
        let (mut app, _rx) = app(false, "mono");
        for i in 0..6 {
            host(&mut app, i, &format!("n{i}"), 1);
        }
        app.st.nodes.push(state::Node::default());
        let t = &THEMES[app.opts.theme];
        let strip = |w| strip_line(&app, t, w).iter().map(|s| s.content.to_string()).collect();
        let wide: String = strip(20);
        assert_eq!(wide, " NODES ██████·");
        let narrow: String = strip(10);
        assert_eq!(narrow, " NODES ██·", "cells of three nodes; the last holds the silent one");
    }

    #[test]
    fn values_and_columns() {
        assert_eq!(fmt("%", 12.345), "12.3%");
        assert_eq!(fmt("B", 3.0 * 1024.0 * 1024.0), "3.0M");
        assert_eq!(fmt("B/s", 512.0), "512");
        assert_eq!(fmt("B/s", 20_480.0), "20K/s");
        assert_eq!(fmt("1", 1500.0), "1.5k");
        assert_eq!(fmt("ms", 7.0), "7 ms");
        assert_eq!(fmt("B", f64::NAN), "-");
        assert_eq!(interval(500, What::Cg), "500ms");
        assert_eq!(interval(2000, What::Procs), "2s");
        let mut r = Row { v: [1.0, f64::NAN, f64::NAN, 2.0, f64::NAN], ..Row::default() };
        assert_eq!(shown(&[r.clone()]), [0, 3]);
        r.v[0] = f64::NAN;
        assert_eq!(shown(&[r]), [0, 3], "CPU% always shows");
        let big = default_rules(17, 1000);
        assert_eq!(big.len(), 2);
        assert_eq!((big[0].interval_ms, big[1].what), (2000, What::Off));
    }
}
