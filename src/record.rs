// SPDX-License-Identifier: LGPL-3.0-or-later
// SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany

//! Record and replay (§3.5, §3.6, §8): the recorder that wraps a job step, the Arrow IPC stream
//! journal that it and TUI recording write, and the reader whose batch index replay seeks in.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Cursor, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use arrow_array::builder::{ArrayBuilder, StringBuilder, TimestampMillisecondBuilder as TsBuilder};
use arrow_array::builder::{UInt8Builder, UInt32Builder, UInt64Builder};
use arrow_array::cast::AsArray;
use arrow_array::types::{TimestampMillisecondType as Ms, UInt8Type, UInt32Type, UInt64Type};
use arrow_array::{Array, PrimitiveArray as Prim, RecordBatch, StringArray};
use arrow_ipc::writer::{IpcWriteOptions, StreamWriter};
use arrow_ipc::{CompressionType, reader::StreamReader};
use arrow_schema::DataType::{self, Timestamp, UInt8, UInt32, UInt64, Utf8};
use arrow_schema::{Field, Schema, SchemaRef, TimeUnit};
use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};

use crate::agent::{Opts, Sampler, now_ms, step_id};
use crate::model::{self, Control, Ent, Event, Kind, NA, Rule, TaskSet, What};
use crate::state::{Columns, Layout, Node, Proc, State, Vals};
use crate::tui::{self, App};
use crate::{RecordArgs, cgroup, procfs, wire};

/// Rows per batch at most: bounds the recorder's builders to about 3 MB (§8.3).
pub const MAX_ROWS: usize = 8192;
/// Batch age at most: the loss window of a killed recorder (§8.3).
const MAX_MS: u64 = 10_000;
/// Fixed columns before the metric columns: ts, kind, cg, pid, start, ppid, path, comm, cmdline.
const FIXED: usize = 9;
/// History a backward seek replays: twice the longest rule interval (60 s), so the sample
/// before the cursor's of every entity, and with it its rates, is applied (§8.4).
const BACK_MS: u64 = 120_000;
/// Milliseconds per day, for `--at HH:MM`.
const DAY: u64 = 86_400_000;
/// Recording after a signal at most: Slurm may signal only the task leader, so the app may never
/// stop; ending well inside the default `KillWait` (30 s) still writes the end-of-stream marker.
const GRACE_MS: u64 = 5_000;

/// The journal schema: identity and keyframe columns, then one nullable `UInt64` per metric
/// column of the store with unit, kind and source as field metadata (§8.2).
pub fn schema(cols: &Columns, meta: HashMap<String, String>) -> Schema {
    let ts = Timestamp(TimeUnit::Millisecond, Some("UTC".into()));
    let fixed: [(&str, DataType); FIXED] = [
        ("ts", ts),
        ("kind", UInt8),
        ("cg", UInt64),
        ("pid", UInt32),
        ("start", UInt64),
        ("ppid", UInt32),
        ("path", Utf8),
        ("comm", Utf8),
        ("cmdline", Utf8),
    ];
    let mut fields: Vec<Field> =
        fixed.into_iter().enumerate().map(|(i, (n, t))| Field::new(n, t, i > 2)).collect();
    for (cols, source) in cols.0.iter().zip(["cgroup", "proc"]) {
        for (name, unit, kind) in cols {
            let kind = if *kind == Kind::Counter { "counter" } else { "gauge" };
            let m = [("unjam.unit", &**unit), ("unjam.kind", kind), ("unjam.source", source)];
            let m = HashMap::from(m.map(|(k, v)| (k.to_owned(), v.to_owned())));
            fields.push(Field::new(&**name, UInt64, true).with_metadata(m));
        }
    }
    Schema::new_with_metadata(fields, meta)
}

/// The output pattern: `%j` job id, `%s` step id, `%n` host name, `%%`; missing values are `na`.
pub fn expand(pat: &str, job: Option<&str>, step: Option<&str>, host: &str) -> PathBuf {
    let (job, step) = (job.unwrap_or("na"), step.unwrap_or("na"));
    let host = if host.is_empty() { "na" } else { host };
    let p = pat.replace("%%", "\0").replace("%j", job).replace("%s", step).replace("%n", host);
    p.replace('\0', "%").into()
}

/// Arrow builders of one journal plus per-batch keyframe tracking (§8.3).
pub struct Writer {
    w: StreamWriter<BufWriter<File>>,
    schema: SchemaRef,
    ts: TsBuilder,
    kind: UInt8Builder,
    /// `cg` and `start`, then one per metric column (cgroup columns first).
    u64s: Vec<UInt64Builder>,
    /// `pid` and `ppid`; `path`, `comm` and `cmdline`.
    ids: [UInt32Builder; 2],
    strs: [StringBuilder; 3],
    /// Number of cgroup metric columns.
    width: usize,
    /// Entities that have their keyframe row in the open batch, the batch's first tick and the
    /// last tick written.
    keyed: HashSet<(u64, u64)>,
    since: u64,
    last: u64,
}

impl Writer {
    /// Creates `path` (never overwriting; owner-only, as it holds command lines) and writes the
    /// schema message at once. Node `i`'s identity and the `meta` pairs become `unjam.*` schema
    /// metadata.
    pub fn create(path: &Path, st: &State, i: usize, meta: &[(&str, String)]) -> io::Result<Self> {
        let n = &st.nodes[i];
        let btime = if n.btime == NA { "na".into() } else { n.btime.to_string() };
        let fixed = [("schema", "1".into()), ("version", env!("CARGO_PKG_VERSION").into())];
        let node = [("host", n.host.clone()), ("step", n.step.clone()), ("btime", btime)];
        let all = meta.iter().cloned().chain(fixed).chain(node);
        let schema = schema(&st.cols, all.map(|(k, v)| (format!("unjam.{k}"), v)).collect());
        let lz4 = IpcWriteOptions::default().try_with_compression(Some(CompressionType::LZ4_FRAME));
        let file =
            BufWriter::new(File::options().write(true).create_new(true).mode(0o600).open(path)?);
        let mut w =
            StreamWriter::try_new_with_options(file, &schema, lz4.map_err(io::Error::other)?)
                .map_err(io::Error::other)?;
        w.flush().map_err(io::Error::other)?;
        let u64s = (FIXED - 2..schema.fields().len()).map(|_| UInt64Builder::new()).collect();
        let (ts, schema) = (TsBuilder::new().with_timezone("UTC"), Arc::new(schema));
        let (ids, strs, kind) = (Default::default(), Default::default(), UInt8Builder::new());
        let (width, keyed) = (st.cols.0[0].len(), HashSet::new());
        Ok(Self { w, schema, ts, kind, u64s, ids, strs, width, keyed, since: 0, last: 0 })
    }

    /// Appends node `i`'s tick at its last `T` time as a complete snapshot: every cgroup in
    /// path order (parents first), then every process. Values only where they were taken at
    /// that tick, so a TUI recording keeps the rule intervals it saw (§8.2, §8.5). A tick
    /// already written is skipped.
    pub fn node(&mut self, st: &State, i: usize) -> io::Result<()> {
        let n = &st.nodes[i];
        if std::mem::replace(&mut self.last, n.prev[0]) == n.prev[0] {
            return Ok(());
        }
        if !self.kind.is_empty() && n.prev[0].saturating_sub(self.since) >= MAX_MS {
            self.cut()?;
        }
        let mut cgs: Vec<_> = n.cgs.iter().collect();
        cgs.sort_unstable_by(|a, b| a.1.path.cmp(&b.1.path));
        for (&id, c) in cgs {
            self.row(n.prev[0], id, None, &c.path, &c.vals)?;
        }
        let mut procs: Vec<_> = n.procs.iter().collect();
        procs.sort_unstable_by_key(|p| *p.0);
        procs
            .into_iter()
            .try_for_each(|(&pid, p)| self.row(n.prev[0], p.cg, Some((pid, p)), "", &p.vals))
    }

    /// One row; keyframe columns only on the entity's first row in the batch.
    fn row(
        &mut self,
        ts: u64,
        cg: u64,
        p: Option<(u32, &Proc)>,
        path: &str,
        v: &Vals,
    ) -> io::Result<()> {
        self.since = if self.kind.is_empty() { ts } else { self.since };
        let key = self.keyed.insert(p.map_or((cg, NA), |(pid, p)| (u64::from(pid), p.start)));
        let kf = p.filter(|_| key).map(|p| p.1);
        self.ts.append_value(i64::try_from(ts).unwrap_or(i64::MAX));
        self.kind.append_value(if p.is_some() { 2 } else { 1 });
        self.u64s[0].append_value(cg);
        self.u64s[1].append_option(kf.map(|p| p.start));
        self.ids[0].append_option(p.map(|p| p.0));
        self.ids[1].append_option(kf.map(|p| p.ppid));
        self.strs[0].append_option((key && p.is_none()).then_some(path));
        self.strs[1].append_option(kf.map(|p| &*p.comm));
        self.strs[2].append_option(kf.map(|p| &*p.cmd));
        let own = if p.is_some() { self.width..usize::MAX } else { 0..self.width };
        for (j, b) in self.u64s[2..].iter_mut().enumerate() {
            let x = own.contains(&j).then(|| v.v.get(j - own.start)).flatten();
            b.append_option(x.copied().filter(|&x| x != NA && v.t[0] == ts));
        }
        if self.kind.len() >= MAX_ROWS { self.cut() } else { Ok(()) }
    }

    /// Writes the open batch and flushes it to the file (§8.3).
    #[allow(clippy::similar_names, reason = "pid and ppid are the kernel's names")]
    pub fn cut(&mut self) -> io::Result<()> {
        let ([pid, ppid], [path, comm, cmd]) = (&mut self.ids, &mut self.strs);
        let Some(([cg, start], vals)) = self.u64s.split_first_chunk_mut::<2>() else {
            return Ok(());
        };
        if self.kind.is_empty() {
            return Ok(());
        }
        let fixed: [&mut dyn ArrayBuilder; FIXED] =
            [&mut self.ts, &mut self.kind, cg, pid, start, ppid, path, comm, cmd];
        let all = fixed.into_iter().chain(vals.iter_mut().map(|b| b as &mut dyn ArrayBuilder));
        let batch =
            RecordBatch::try_new(Arc::clone(&self.schema), all.map(ArrayBuilder::finish).collect());
        self.keyed.clear();
        let batch = batch.map_err(io::Error::other)?;
        self.w.write(&batch).and_then(|()| self.w.flush()).map_err(io::Error::other)
    }

    /// Writes the last batch and the end-of-stream marker.
    pub fn finish(mut self) -> io::Result<()> {
        self.cut()?;
        self.w.finish().and_then(|()| self.w.flush()).map_err(io::Error::other)
    }
}

/// A `Read` that counts the bytes it hands out. `StreamReader::try_new` does not buffer, so
/// the count is the exact offset of the next message (§8.4).
struct Counting(BufReader<File>, u64);

impl Read for Counting {
    /// Reads and counts.
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        let k = self.0.read(b)?;
        self.1 += k as u64;
        Ok(k)
    }
}

/// The typed columns of one decoded batch: fixed ones by name, metrics with their entity.
struct Cols<'a> {
    ts: &'a Prim<Ms>,
    kind: &'a Prim<UInt8Type>,
    u64s: [&'a Prim<UInt64Type>; 2],
    ids: [&'a Prim<UInt32Type>; 2],
    strs: [&'a StringArray; 3],
    metrics: Vec<(Ent, &'a Prim<UInt64Type>)>,
}

impl<'a> Cols<'a> {
    /// Looks the columns up; `None` if a fixed one is missing or mistyped. Unknown columns
    /// are ignored; metric columns are the `cg_*` and `p_*` ones (§8.2).
    fn new(b: &'a RecordBatch) -> Option<Self> {
        let col = |n: &str| b.column_by_name(n);
        let (u64s, ids) = (|n| col(n)?.as_primitive_opt(), |n| col(n)?.as_primitive_opt());
        let strs = |n| col(n)?.as_string_opt();
        let ent = |n: &str| match n.split_once('_')?.0 {
            "cg" => Some(Ent::Cgroup),
            "p" => Some(Ent::Proc),
            _ => None,
        };
        let metrics = b.schema_ref().fields().iter().zip(b.columns());
        Some(Self {
            ts: col("ts")?.as_primitive_opt()?,
            kind: col("kind")?.as_primitive_opt()?,
            u64s: [u64s("cg")?, u64s("start")?],
            ids: [ids("pid")?, ids("ppid")?],
            strs: [strs("path")?, strs("comm")?, strs("cmdline")?],
            metrics: metrics
                .filter_map(|(f, c)| Some((ent(f.name())?, c.as_primitive_opt()?)))
                .collect(),
        })
    }
}

/// One node's journal: schema message, batch index and replay progress.
struct Track {
    path: PathBuf,
    head: Vec<u8>,
    schema: SchemaRef,
    /// `(offset, first ts)` per batch; the next batch to load; the loaded batch and next row.
    index: Vec<(u64, u64)>,
    next: usize,
    batch: Option<RecordBatch>,
    row: usize,
    /// Cgroup ids by path, to link keyframed cgroups to their parents.
    paths: HashMap<Box<str>, u64>,
    /// The tick being applied and the cgroups and processes it listed.
    tick: (u64, HashSet<u64>, HashSet<u32>),
}

impl Track {
    /// Indexes a journal, decoding only `ts`. Returns its tick times and, if it lacks the
    /// end-of-stream marker, the length of its valid prefix.
    fn open(path: &Path) -> Result<(Self, Vec<u64>, Option<u64>), String> {
        let e = |x: &dyn std::fmt::Display| format!("{}: {x}", path.display());
        let file = Counting(BufReader::new(File::open(path).map_err(|x| e(&x))?), 0);
        let mut r = StreamReader::try_new(file, Some(vec![0])).map_err(|x| e(&x))?;
        let (mut index, mut times, mut at) = (vec![], vec![], r.get_ref().1);
        let mut head = vec![0; usize::try_from(at).map_err(|x| e(&x))?];
        File::open(path).and_then(|mut f| f.read_exact(&mut head)).map_err(|x| e(&x))?;
        let complete = loop {
            at = r.get_ref().1;
            match r.next() {
                Some(Ok(b)) => {
                    let Some(ts) = b.column(0).as_primitive_opt::<Ms>() else { break false };
                    let first = times.len();
                    times.extend(ts.values().iter().map(|&t| u64::try_from(t).unwrap_or(0)));
                    index.extend(times.get(first).map(|&t| (at, t)));
                }
                Some(Err(_)) => break false,
                None => break r.get_ref().1 >= at + 4,
            }
        };
        let schema = StreamReader::try_new(Cursor::new(&head), None).map_err(|x| e(&x))?.schema();
        times.dedup();
        let (path, paths, tick) = (path.to_owned(), HashMap::new(), Default::default());
        let t = Self { path, head, schema, index, next: 0, batch: None, row: 0, paths, tick };
        Ok((t, times, (!complete).then_some(at)))
    }

    /// Decodes batch `k` alone: the schema message chained to the file at the batch's offset.
    fn load(&mut self, k: usize) -> Result<(), String> {
        let bad = |x: &dyn std::fmt::Display| format!("{}: batch {k}: {x}", self.path.display());
        let mut f = File::open(&self.path).map_err(|x| bad(&x))?;
        f.seek(SeekFrom::Start(self.index[k].0)).map_err(|x| bad(&x))?;
        let src = Cursor::new(self.head.as_slice()).chain(BufReader::new(f));
        let mut r = StreamReader::try_new(src, None).map_err(|x| bad(&x))?;
        let b = r.next().ok_or_else(|| bad(&"missing"))?.map_err(|x| bad(&x))?;
        (self.batch, self.row, self.next) = (Some(b), 0, k + 1);
        Ok(())
    }

    /// Empties node `i` and announces the journal's identity and columns (`H`, `D`).
    fn reset(&mut self, st: &mut State, i: u32) {
        if let Some(n) = st.nodes.get_mut(i as usize) {
            *n = Node::default();
        }
        let m = self.schema.metadata();
        let get = |k: &str| m.get(&format!("unjam.{k}")).map_or("-", String::as_str);
        let (version, host, step) = (get("version"), get("host"), get("step"));
        let btime = get("btime").parse().unwrap_or(NA);
        let hello = Event::Hello { proto: wire::PROTO, version, host, step, ntasks: 1, btime };
        let _ = st.apply(i, &hello, 0);
        let mut idx = [0u16; 2];
        for f in self.schema.fields().iter().skip(FIXED) {
            let ent = if f.name().starts_with("p_") { Ent::Proc } else { Ent::Cgroup };
            let fm = |k: &str| f.metadata().get(k).map_or("", String::as_str);
            let kind = if fm("unjam.kind") == "counter" { Kind::Counter } else { Kind::Gauge };
            let (name, unit, idx) = (f.name().as_str(), fm("unjam.unit"), &mut idx[ent as usize]);
            let _ = st.apply(i, &Event::Desc { ent, idx: *idx, name, unit, kind }, 0);
            *idx += 1;
        }
        (self.batch, self.next, self.tick) = (None, 0, Default::default());
        self.paths.clear();
    }

    /// Applies the rows up to time `t` into node `i`, loading later batches as needed.
    fn advance(&mut self, st: &mut State, i: u32, t: u64) -> Result<(), String> {
        let mut vals = Vec::new();
        loop {
            let Some(b) = self.batch.as_ref().filter(|b| self.row < b.num_rows()).cloned() else {
                if self.index.get(self.next).is_none_or(|e| e.1 > t) {
                    break;
                }
                self.load(self.next)?;
                continue;
            };
            let c = Cols::new(&b).ok_or_else(|| format!("{}: bad columns", self.path.display()))?;
            while self.row < b.num_rows() && c.ts.value(self.row) <= i64::try_from(t).unwrap_or(0) {
                self.apply(st, i, &c, &mut vals);
                self.row += 1;
            }
            if self.row < b.num_rows() {
                break;
            }
        }
        self.gone(st, i);
        Ok(())
    }

    /// Applies one row as the events a live agent would have sent.
    #[allow(clippy::similar_names, reason = "pid and ppid are the kernel's names")]
    fn apply(&mut self, st: &mut State, i: u32, c: &Cols, vals: &mut Vec<u64>) {
        let (r, [cg, start], [pid, ppid], [path, comm, cmd]) = (self.row, c.u64s, c.ids, c.strs);
        let ts = u64::try_from(c.ts.value(r)).unwrap_or(0);
        if ts != self.tick.0 {
            self.gone(st, i);
            self.tick = (ts, HashSet::new(), HashSet::new());
            let _ = st.apply(i, &bare_tick(ts), ts);
        }
        let (proc, id) = (c.kind.value(r) == 2, cg.value(r));
        let mine = c.metrics.iter().filter(|m| m.0 == if proc { Ent::Proc } else { Ent::Cgroup });
        vals.clear();
        vals.extend(mine.map(|m| if m.1.is_valid(r) { m.1.value(r) } else { NA }));
        let v = vals.iter().any(|&x| x != NA).then_some(&vals[..]);
        if proc {
            let pid = pid.value(r);
            if comm.is_valid(r) {
                let (start, ppid, comm, cmd) =
                    (start.value(r), ppid.value(r), comm.value(r), cmd.value(r));
                let _ = st.apply(i, &Event::Proc { pid, start, cg: id, ppid, comm, cmd }, ts);
            }
            let _ = v.map(|v| st.apply(i, &Event::ProcVals { pid, first: 0, v }, ts));
            self.tick.2.insert(pid);
        } else {
            if path.is_valid(r) {
                let path = path.value(r);
                let up = path.rsplit_once('/').map_or("", |p| p.0);
                let parent = if path.is_empty() { 0 } else { *self.paths.get(up).unwrap_or(&NA) };
                self.paths.insert(path.into(), id);
                let _ = st.apply(i, &Event::Cg { id, parent, path }, ts);
            }
            let _ = v.map(|v| st.apply(i, &Event::CgVals { id, first: 0, v }, ts));
            self.tick.1.insert(id);
        }
    }

    /// Removes what the tick being applied did not list: each tick is a complete snapshot.
    fn gone(&self, st: &mut State, i: u32) {
        let (ts, cgs, procs) = &self.tick;
        let Some(n) = st.nodes.get(i as usize).filter(|_| *ts > 0) else { return };
        let p: Vec<u32> = n.procs.keys().filter(|p| !procs.contains(p)).copied().collect();
        let c: Vec<u64> = n.cgs.keys().filter(|c| !cgs.contains(c)).copied().collect();
        let gone = p.into_iter().map(Event::ProcGone).chain(c.into_iter().map(Event::CgGone));
        gone.for_each(|ev| st.apply(i, &ev, *ts).unwrap_or_default());
    }
}

/// Multi-file replay: one track per node journal and one shared cursor over their sampling
/// rounds (§8.4).
pub struct Reader {
    tracks: Vec<Track>,
    pub times: Vec<u64>,
    pub at: usize,
}

impl Reader {
    /// Opens and indexes every journal; the warnings name truncated ones. Nodes stamp a round
    /// a few ms apart, so ticks closer than half the shortest tick gap of any journal form one
    /// round, which the cursor visits at its latest tick.
    pub fn open(paths: &[PathBuf]) -> Result<(Self, Vec<String>), String> {
        let (mut tracks, mut times, mut warns, mut gap) = (vec![], vec![], vec![], u64::MAX);
        for p in paths {
            let (t, ts, cut) = Track::open(p)?;
            warns.extend(cut.map(|n| format!("{}: truncated at byte {n}", p.display())));
            gap = ts.windows(2).map(|w| w[0].abs_diff(w[1])).fold(gap, u64::min);
            tracks.push(t);
            times.extend(ts);
        }
        // Newest first, so that each round keeps its latest tick.
        times.sort_unstable_by(|a, b| b.cmp(a));
        times.dedup_by(|t, round| *round - *t < gap.div_ceil(2));
        times.reverse();
        let none = || "no complete batch in the recordings".to_owned();
        (!times.is_empty())
            .then_some((Self { tracks, times, at: usize::MAX }, warns))
            .ok_or_else(none)
    }

    /// The cursor time.
    pub fn now(&self) -> u64 {
        self.times[self.at.min(self.times.len() - 1)]
    }

    /// The last tick at or before `t`, at least the first.
    pub fn find(&self, t: u64) -> usize {
        self.times.partition_point(|&x| x <= t).saturating_sub(1)
    }

    /// Moves the cursor to tick `k`. Forward applies the ticks in between; backward rebuilds
    /// each node from the batch holding `BACK_MS` before the tick, so rates span batch
    /// boundaries. Each node shows its last tick at or before the cursor.
    pub fn goto(&mut self, st: &mut State, k: usize) -> Result<(), String> {
        let k = k.min(self.times.len() - 1);
        let (t, back) = (self.times[k], k < self.at || self.at == usize::MAX);
        for (i, tr) in (0..).zip(&mut self.tracks) {
            if back {
                tr.reset(st, i);
                tr.next = tr.index.partition_point(|e| e.1 + BACK_MS <= t).saturating_sub(1);
            }
            tr.advance(st, i, t)?;
        }
        self.at = k;
        Ok(())
    }
}

/// The `T` event of a recorded or replayed tick: there is no agent to monitor.
fn bare_tick(ts_ms: u64) -> Event<'static> {
    Event::Tick { ts_ms, self_cpu_us: NA, out_bytes: NA, dropped: 0, self_rss: NA }
}

/// `--at`: `start`, `end`, `+DUR` after the start, `-DUR` before the end, or `HH:MM[:SS]` UTC
/// on the first day of the recording (the next day if that is before its start).
fn at(s: &str, first: u64, last: u64) -> Result<u64, String> {
    let dur = |d: &str| crate::parse_dur(d).map(|d| u64::try_from(d.as_millis()).unwrap_or(0));
    let bad = || format!("bad --at {s:?} (start, end, +DUR, -DUR or HH:MM[:SS])");
    let hms: Result<Vec<u64>, _> = s.split(':').map(str::parse).collect();
    let secs = match (s, hms.as_deref()) {
        ("start", _) => return Ok(first),
        ("end", _) => return Ok(last),
        _ if s.starts_with('+') => return Ok(first + dur(&s[1..])?),
        _ if s.starts_with('-') => return Ok(last.saturating_sub(dur(&s[1..])?)),
        (_, Ok([h, m])) => h * 3600 + m * 60,
        (_, Ok([h, m, sec])) => h * 3600 + m * 60 + sec,
        _ => return Err(bad()),
    };
    let t = first / DAY * DAY + secs * 1000;
    Ok(if t < first { t + DAY } else { t })
}

/// `unjam --replay FILE…`: `--batch N` prints N snapshots one tick apart from `--at`;
/// otherwise the TUI with the replay keys. Warnings go to stderr or the log.
pub fn replay(
    paths: &[PathBuf],
    at_: Option<&str>,
    opts: tui::Opts,
    n: Option<u32>,
) -> Result<(), String> {
    let (mut r, warns) = Reader::open(paths)?;
    let (first, last) = (r.times[0], r.times[r.times.len() - 1]);
    let k = r.find(at_.map_or(Ok(first), |s| at(s, first, last))?);
    let ((tx, rx), (ctl, _)) = (mpsc::sync_channel(64), mpsc::channel());
    let mut app = App::new(tui::Opts { source: "--replay".into(), ..opts }, ctl);
    app.view.layout = Layout::Node;
    r.goto(&mut app.st, k)?;
    let Some(n) = n else {
        app.log.extend(warns);
        app.replay = Some(r);
        let res = tui::view(&mut app, &tx, &rx, None, &mut io::sink());
        return res.map_err(|e| format!("tui: {e}"));
    };
    let mut err = io::stderr().lock();
    for w in &warns {
        let _ = writeln!(err, "unjam: {w}");
    }
    snapshots(&mut app, &mut r, n, &mut io::stdout().lock())
}

/// `--batch N` for replay: a snapshot per tick from the cursor, at most N, until the end.
fn snapshots(app: &mut App, r: &mut Reader, n: u32, out: &mut dyn Write) -> Result<(), String> {
    for i in 0..n {
        if i > 0 && r.at + 1 >= r.times.len() {
            break;
        }
        if i > 0 {
            r.goto(&mut app.st, r.at + 1)?;
        }
        (app.now, ()) = (r.now(), app.st.derive());
        tui::snapshot(app, out).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// TUI recording (D41): one journal per node in `dir`, fed the ticks the viewer receives.
pub struct Rec {
    pub dir: PathBuf,
    meta: [(&'static str, String); 3],
    files: HashMap<u32, Option<Writer>>,
}

impl Rec {
    /// A recording into `dir` (created owner-only if missing) of a viewer on `job`.
    pub fn new(dir: PathBuf, job: &str, interval_ms: u32) -> Result<Self, String> {
        let made = fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir);
        made.map_err(|e| format!("record {}: {e}", dir.display()))?;
        let iv = interval_ms.to_string();
        let meta = [("source", "tui".into()), ("job", job.into()), ("interval_ms", iv)];
        Ok(Self { dir, meta, files: HashMap::new() })
    }

    /// Writes node `i`'s last complete tick, creating `DIR/<host>.arrows` on first use (`/` in
    /// the agent-reported host becomes `_`, so the file stays in `DIR`); a node whose file
    /// cannot be created is skipped from then on.
    pub fn node(&mut self, st: &State, i: u32) -> Result<(), String> {
        let n = &st.nodes[i as usize];
        if n.prev[0] == 0 || n.host.is_empty() {
            return Ok(());
        }
        let path = || self.dir.join(format!("{}.arrows", n.host.replace('/', "_")));
        let e = |x: io::Error| format!("record {}: {x}", path().display());
        if !self.files.contains_key(&i) {
            let w = Writer::create(&path(), st, i as usize, &self.meta);
            let (w, err) = w.map_or_else(|x| (None, Err(e(x))), |w| (Some(w), Ok(())));
            self.files.insert(i, w);
            err?;
        }
        let Some(Some(w)) = self.files.get_mut(&i) else { return Ok(()) };
        w.node(st, i as usize).map_err(e)
    }

    /// Writes every node's last tick and closes the journals; returns the errors.
    pub fn finish(mut self, st: &State) -> Vec<String> {
        let nodes = 0..u32::try_from(st.nodes.len()).unwrap_or(0);
        let mut errs: Vec<String> = nodes.filter_map(|i| self.node(st, i).err()).collect();
        let done = self.files.into_values().flatten().map(Writer::finish);
        errs.extend(done.filter_map(Result::err).map(|e| format!("record: {e}")));
        errs
    }
}

/// What the recorder samples, and the cgroups its lifetime rule watches.
struct Scope {
    root: PathBuf,
    /// The own cgroup (the task cgroup in a step); none with an explicit `--root`.
    own: Option<PathBuf>,
    /// Whether the root is the own step's `user` directory, whose other tasks also count.
    step: bool,
}

/// The scope (§3.5): `--root`, else the own step's `user` directory (every task of the step
/// on this node), else the own cgroup. Also the sampling environment.
fn scope(root: Option<PathBuf>) -> Result<(Scope, crate::source::Env), String> {
    let (mount, me) = cgroup::self_cgroup()?;
    let explicit = root.is_some();
    let own = if explicit { None } else { Some(cgroup::v2_check(mount, mount, &me, false)?) };
    let step = own.as_ref().and_then(|_| cgroup::record_root(mount, &me).ok());
    let root = root.or_else(|| step.clone()).or_else(|| own.clone()).unwrap_or_default();
    let env = procfs::env("/proc".into(), cgroup::v2_check(mount, &root, &me, explicit)?);
    Ok((Scope { root, own, step: step.is_some() }, env))
}

/// The pids in a cgroup, sorted.
fn pids(dir: &Path) -> Vec<u32> {
    let mut out = Vec::new();
    cgroup::procs(&fs::read(dir.join("cgroup.procs")).unwrap_or_default(), &mut out);
    out
}

/// The lifetime rule (§3.5) once the app has exited: the own cgroup holds no process but the
/// recorder and those there before the app started (daemonized descendants keep it alive),
/// and in a step every other task cgroup reports `populated 0`.
fn quiet(sc: &Scope, before: &[u32]) -> bool {
    let Some(own) = &sc.own else { return true };
    let me = std::process::id();
    let alone = pids(own).iter().all(|p| *p == me || before.binary_search(p).is_ok());
    let idle = |d: &Path| {
        fs::read_to_string(d.join("cgroup.events")).map_or(true, |e| e.contains("populated 0"))
    };
    let dirs = fs::read_dir(&sc.root).into_iter().flatten().flatten().map(|e| e.path());
    alone && (!sc.step || dirs.filter(|d| d != own && d.is_dir()).all(|d| idle(&d)))
}

/// `unjam record`: tasks with `SLURM_LOCALID` ≠ 0 become the app (`exec`). Otherwise the app
/// runs as a child while the scope is sampled into the journal until the lifetime rule holds;
/// if recording cannot start, the app runs unrecorded after a warning on stderr. Signals set a
/// flag that cuts the open batch and are not forwarded (D19); recording then ends at the
/// lifetime rule or after `GRACE_MS`, whichever comes first. Returns the app's exit code, or
/// 128 + signal (§3.5).
pub fn run_record(a: &RecordArgs) -> Result<ExitCode, String> {
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let mut app = Command::new(&a.cmd[0]);
    app.args(&a.cmd[1..]);
    let started = var("SLURM_LOCALID").is_none_or(|l| l == "0").then(|| start(a, &var));
    if let Some(Err(e)) = &started {
        let _ = writeln!(io::stderr(), "unjam: record: {e}; running unrecorded");
    }
    let Some(Ok((sc, mut st, mut s, mut w))) = started else {
        return Err(format!("{}: {}", a.cmd[0], app.exec()));
    };
    let sig = Arc::new(AtomicUsize::new(0));
    for n in [SIGTERM, SIGINT, SIGHUP] {
        let reg = signal_hook::flag::register_usize(n, Arc::clone(&sig), n.unsigned_abs() as usize);
        reg.map_err(|e| format!("signals: {e}"))?;
    }
    let before = sc.own.as_deref().map(pids).unwrap_or_default();
    let mut child = app.spawn().map_err(|e| format!("{}: {e}", a.cmd[0]))?;
    let (mut status, mut grace, werr) = (None, u64::MAX, |e: io::Error| format!("record: {e}"));
    while (status.is_none() || !quiet(&sc, &before)) && now_ms() < grace {
        let now = now_ms();
        if now >= s.next_due(now) {
            let _ = st.apply(0, &bare_tick(now), now);
            s.tick(now, &mut |e| st.apply(0, e, now).unwrap_or_default());
            w.node(&st, 0).map_err(werr)?;
        }
        if sig.load(Relaxed) != 0 && grace == u64::MAX {
            (grace, ()) = (now + GRACE_MS, w.cut().map_err(werr)?);
        }
        status = status.or(child.try_wait().map_err(|e| e.to_string())?);
        let wait = s.next_due(now).saturating_sub(now_ms()).min(200);
        std::thread::sleep(Duration::from_millis(wait));
    }
    w.finish().map_err(werr)?;
    let code = status.map_or_else(
        || 128 + i32::try_from(sig.load(Relaxed)).unwrap_or(0),
        |st| st.code().unwrap_or_else(|| 128 + st.signal().unwrap_or(0)),
    );
    Ok(ExitCode::from(u8::try_from(code & 0xff).unwrap_or(1)))
}

/// Recorder setup: scope, a sampler with one rule over everything, the store and the journal.
fn start(
    a: &RecordArgs,
    var: &dyn Fn(&str) -> Option<String>,
) -> Result<(Scope, State, Sampler, Writer), String> {
    let (sc, env) = scope(a.root.clone())?;
    let (job, step) = (var("SLURM_JOB_ID"), var("SLURM_STEP_ID"));
    let id = step_id(var);
    let opts =
        Opts { node: 0, ntasks: 1, step: id, root: sc.root.clone(), exclude: None, cap: 1024 };
    let (mut s, mut st, now) = (Sampler::new(env, opts), State::new(), now_ms());
    let ms = model::interval_ms(a.interval.as_millis());
    let what = if a.no_procs { What::Cg } else { What::Procs };
    let rule = Rule { tasks: TaskSet::All, pattern: ".".into(), interval_ms: ms, what };
    s.control(Control::Rule(rule), now, &mut |_| {});
    s.snapshot(now, &mut |e| st.apply(0, e, now).unwrap_or_default());
    let path = expand(&a.output, job.as_deref(), step.as_deref(), &st.nodes[0].host);
    let job = job.unwrap_or_else(|| "na".into());
    let (root, ms) = (sc.root.display().to_string(), ms.to_string());
    let meta = [("source", "record".into()), ("job", job), ("root", root), ("interval_ms", ms)];
    let w = Writer::create(&path, &st, 0, &meta).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok((sc, st, s, w))
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::*;
    use crate::theme::THEMES;

    /// A temporary directory, removed on drop.
    struct Tmp(PathBuf);

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn tmp(name: &str) -> Tmp {
        let d = std::env::temp_dir().join(format!("unjam-rec-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        Tmp(d)
    }

    /// Applies space-separated wire lines (`~` is a space inside a field) as node `node`.
    fn feed(st: &mut State, node: u32, lines: &[String]) {
        let mut vals = Vec::new();
        for l in lines {
            let line = l.split(' ').map(|f| f.replace('~', " ")).collect::<Vec<_>>().join("\t");
            let ev = wire::parse(line.as_bytes(), &mut vals).unwrap();
            st.apply(node, &ev, 0).unwrap();
        }
    }

    /// The announcement of a host with a three-level cgroup tree and a shell.
    fn hello(st: &mut State, node: u32, host: &str) {
        let head = [
            &format!("H 1 0.1 {host} 7.0 1 1700000000"),
            "D c 0 cg_cpu_usage_usec us counter",
            "D c 1 cg_mem_current_bytes B gauge",
            "D c 2 cg_x_future_count 1 gauge",
            "D p 0 p_cpu_user_usec us counter",
            "D p 1 p_rss_bytes B gauge",
            "G 1 0 ",
            "G 2 1 a",
            "G 3 2 a/b",
            "P 10 100 3 1 sh sh~-c~x",
        ];
        feed(st, node, &head.map(str::to_owned));
    }

    /// Tick `t` (seconds, stamped `skew` ms late) of node `node`: a cgroup and a `dd` that live
    /// for ticks 5 to 11, and pid 11 reused by `dd2` from tick 15. The future-source column is
    /// NA from tick 20.
    fn tick(st: &mut State, node: u32, t: u64, skew: u64) {
        let ms = 1_000_000 + t * 1000 + skew;
        let mut l = vec![format!("T {ms} 0 0 0 -")];
        match t {
            5 => l.extend(["G 4 1 c".into(), "P 11 200 4 10 dd dd~if=/dev/zero".into()]),
            12 => l.extend(["x 11".into(), "X 4".into()]),
            15 => l.push("P 11 300 3 1 dd2 dd2".into()),
            _ => {}
        }
        let fut = if t >= 20 { "-".into() } else { t.to_string() };
        let cgs = [1, 2, 3].into_iter().chain((5..12).contains(&t).then_some(4));
        l.extend(
            cgs.map(|c| format!("g {c} 0 {} {} {fut}", c * t * 1000 + u64::from(node), 4096 * c)),
        );
        let procs = [10].into_iter().chain((5..12).contains(&t).then_some(11));
        l.extend(
            procs.chain((t >= 15).then_some(11)).map(|p| format!("p {p} 0 {} 8192", p * t * 500)),
        );
        feed(st, node, &l);
    }

    /// Records `n` ticks of host `host` to `dir/<host>.arrows`; returns the source store.
    fn journal(dir: &Path, host: &str, n: u64) -> (PathBuf, State) {
        skewed(dir, host, n, 0)
    }

    /// `journal` with every tick stamped `skew` ms late, as a node's agent wakes up late.
    fn skewed(dir: &Path, host: &str, n: u64, skew: u64) -> (PathBuf, State) {
        let (mut st, path) = (State::new(), dir.join(format!("{host}.arrows")));
        hello(&mut st, 0, host);
        let mut w = None;
        for t in 1..=n {
            tick(&mut st, 0, t, skew);
            let meta = [("source", "test".into())];
            let w = w.get_or_insert_with(|| Writer::create(&path, &st, 0, &meta).unwrap());
            w.node(&st, 0).unwrap();
        }
        w.unwrap().finish().unwrap();
        (path, st)
    }

    /// A node as sorted text: what replay must reproduce (history rings and trailing NA
    /// values excluded).
    fn dump(n: &Node) -> String {
        let trim = |v: &[u64]| v[..v.iter().rposition(|&x| x != NA).map_or(0, |i| i + 1)].to_vec();
        let mut cgs: Vec<_> = n.cgs.iter().collect();
        cgs.sort_by_key(|c| c.0);
        let mut out = format!("{} {} {}\n", n.host, n.step, n.prev[0]);
        for (id, c) in cgs {
            let mut kids = c.kids.clone();
            kids.sort_unstable();
            let v = &c.vals;
            let (vv, pv) = (trim(&v.v), trim(&v.pv));
            let _ = writeln!(
                out,
                "G {id} {} {:?} {kids:?} {:?} {vv:?} {pv:?} {:?}",
                c.parent, c.path, c.procs, v.t
            );
        }
        let mut procs: Vec<_> = n.procs.iter().collect();
        procs.sort_by_key(|p| p.0);
        for (pid, p) in procs {
            let v = &p.vals;
            let (vv, pv) = (trim(&v.v), trim(&v.pv));
            let _ = writeln!(
                out,
                "P {pid} {} {} {} {} {} {vv:?} {pv:?} {:?}",
                p.start, p.cg, p.ppid, p.comm, p.cmd, v.t
            );
        }
        out
    }

    fn batches(path: &Path) -> Vec<RecordBatch> {
        StreamReader::try_new(File::open(path).unwrap(), None)
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    #[test]
    fn journals_and_recording_dirs_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tmp("mode");
        let rec = Rec::new(dir.0.join("a/b"), "42", 1000).unwrap();
        let (path, _) = journal(&rec.dir, "n1", 1);
        for p in [&dir.0.join("a"), &rec.dir, &path] {
            let mode = fs::metadata(p).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0, "{}: {mode:o}", p.display());
        }
    }

    #[test]
    fn a_host_name_cannot_leave_the_recording_dir() {
        let dir = tmp("host");
        let mut st = State::new();
        for (i, host) in (0..).zip(["../../x", "/abs"]) {
            hello(&mut st, i, host);
            tick(&mut st, i, 1, 0);
        }
        let rec = Rec::new(dir.0.join("rec"), "42", 1000).unwrap();
        assert!(rec.finish(&st).is_empty());
        let mut names: Vec<_> = fs::read_dir(dir.0.join("rec")).unwrap().flatten().collect();
        names.sort_by_key(fs::DirEntry::file_name);
        let names: Vec<_> = names.iter().map(|e| e.file_name().into_string().unwrap()).collect();
        assert_eq!(names, [".._.._x.arrows", "_abs.arrows"]);
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1, "nothing outside rec/");
    }

    #[test]
    fn round_trip_schema_and_field_metadata() {
        let dir = tmp("trip");
        let (path, src) = journal(&dir.0, "n1", 25);
        let (mut r, warns) = Reader::open(std::slice::from_ref(&path)).unwrap();
        assert!(warns.is_empty(), "{warns:?}");
        assert_eq!(r.times.len(), 25);
        let mut st = State::new();
        r.goto(&mut st, usize::MAX).unwrap();
        assert_eq!(dump(&st.nodes[0]), dump(&src.nodes[0]));
        let schema = StreamReader::try_new(File::open(&path).unwrap(), None).unwrap().schema();
        let m = schema.metadata();
        for (k, v) in [
            ("schema", "1"),
            ("host", "n1"),
            ("step", "7.0"),
            ("btime", "1700000000"),
            ("source", "test"),
        ] {
            assert_eq!(m.get(&format!("unjam.{k}")).map(String::as_str), Some(v), "{k}");
        }
        assert!(m.contains_key("unjam.version"));
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(
            names[..FIXED],
            ["ts", "kind", "cg", "pid", "start", "ppid", "path", "comm", "cmdline"]
        );
        for (name, unit, kind, source) in [
            ("cg_cpu_usage_usec", "us", "counter", "cgroup"),
            ("cg_x_future_count", "1", "gauge", "cgroup"),
            ("p_rss_bytes", "B", "gauge", "proc"),
        ] {
            let f = schema.field_with_name(name).unwrap();
            assert!(f.is_nullable() && *f.data_type() == DataType::UInt64);
            let fm = |k: &str| f.metadata().get(k).map(String::as_str);
            assert_eq!(
                [fm("unjam.unit"), fm("unjam.kind"), fm("unjam.source")],
                [Some(unit), Some(kind), Some(source)]
            );
        }
        let b = &batches(&path)[1];
        let fut = b.column_by_name("cg_x_future_count").unwrap();
        assert!(fut.is_valid(0) && fut.null_count() > 0, "NA becomes null");
    }

    #[test]
    fn keyframes_on_the_first_row_per_entity_and_batch() {
        let dir = tmp("key");
        let (path, _) = journal(&dir.0, "n1", 25);
        let all = batches(&path);
        assert_eq!(all.iter().map(RecordBatch::num_rows).collect::<Vec<_>>(), [52, 48, 25]);
        for b in &all {
            let c = Cols::new(b).unwrap();
            let mut seen = HashSet::new();
            for r in 0..b.num_rows() {
                let (proc, pid) = (c.kind.value(r) == 2, c.ids[0].value(r));
                let dd2 = proc && pid == 11 && c.ts.value(r) >= 1_015_000;
                let key = (proc, if proc { u64::from(pid) } else { c.u64s[0].value(r) }, dd2);
                let first = seen.insert(key);
                let kf = if proc { c.strs[1].is_valid(r) } else { c.strs[0].is_valid(r) };
                assert_eq!(kf, first, "row {r}: {key:?}");
                assert_eq!(
                    kf,
                    if proc { c.u64s[1].is_valid(r) && c.ids[1].is_valid(r) } else { kf }
                );
            }
        }
    }

    #[test]
    fn a_batch_is_cut_at_8192_rows() {
        let dir = tmp("cut");
        let mut st = State::new();
        hello(&mut st, 0, "big");
        feed(&mut st, 0, &(100..5100).map(|c| format!("G {c} 1 x{c}")).collect::<Vec<_>>());
        let path = dir.0.join("big.arrows");
        let mut w = None;
        for t in 1..=2 {
            let mut l = vec![format!("T {} 0 0 0 -", t * 1000)];
            l.extend((100..5100).map(|c| format!("g {c} 0 {t} 1 1")));
            feed(&mut st, 0, &l);
            let w = w.get_or_insert_with(|| Writer::create(&path, &st, 0, &[]).unwrap());
            w.node(&st, 0).unwrap();
        }
        w.unwrap().finish().unwrap();
        let all = batches(&path);
        let rows = 2 * (5000 + 3 + 1);
        assert_eq!(
            all.iter().map(RecordBatch::num_rows).collect::<Vec<_>>(),
            [MAX_ROWS, rows - MAX_ROWS]
        );
        let paths = all[1].column_by_name("path").unwrap();
        assert_eq!(paths.null_count(), 1, "every cgroup is keyframed again after a cut");
    }

    #[test]
    fn seek_equals_linear_replay() {
        let dir = tmp("seek");
        let (p1, _) = journal(&dir.0, "n1", 40);
        let (p2, _) = journal(&dir.0, "n2", 33);
        let paths = [p1, p2];
        let (mut lin, _) = Reader::open(&paths).unwrap();
        let (mut seek, _) = Reader::open(&paths).unwrap();
        let (mut a, mut b) = (State::new(), State::new());
        lin.goto(&mut a, 0).unwrap();
        seek.goto(&mut b, 39).unwrap();
        for k in 1..40 {
            lin.goto(&mut a, k).unwrap();
            if [1, 9, 10, 11, 12, 19, 20, 25, 32, 33, 39].contains(&k) {
                seek.goto(&mut b, k).unwrap();
                for i in 0..2 {
                    assert_eq!(dump(&a.nodes[i]), dump(&b.nodes[i]), "tick {k} node {i}");
                }
            }
        }
        assert_eq!(a.nodes[1].prev[0], 1_033_000, "n2 shows its last tick");
    }

    #[test]
    fn nodes_stamped_ms_apart_share_one_cursor_step() {
        let dir = tmp("rounds");
        let skews = [("n1", 0), ("n2", 3), ("n3", 7)].into_iter();
        let (paths, _): (Vec<_>, Vec<_>) = skews.map(|(h, s)| skewed(&dir.0, h, 10, s)).unzip();
        let (mut r, _) = Reader::open(&paths).unwrap();
        assert_eq!(r.times, (1..=10).map(|t| 1_000_007 + t * 1000).collect::<Vec<_>>());
        let mut st = State::new();
        for k in 0..10 {
            r.goto(&mut st, k).unwrap();
            let shown: Vec<u64> = st.nodes.iter().map(|n| n.prev[0]).collect();
            let want = [0, 3, 7].map(|s| 1_001_000 + k as u64 * 1000 + s);
            assert_eq!(shown, want, "every node advances one tick per step");
        }
    }

    #[test]
    fn backward_seek_keeps_the_rates_of_slow_entities() {
        let dir = tmp("slow");
        let (mut st, path) = (State::new(), dir.0.join("slow.arrows"));
        hello(&mut st, 0, "slow");
        let mut w = None;
        for t in 1..=100 {
            let mut l = vec![format!("T {} 0 0 0 -", 1_000_000 + t * 1000)];
            let cgs = [1, 2].into_iter().chain((t % 30 == 0).then_some(3));
            l.extend(cgs.map(|c| format!("g {c} 0 {} 4096 1", c * t * 1000)));
            feed(&mut st, 0, &l);
            let w = w.get_or_insert_with(|| Writer::create(&path, &st, 0, &[]).unwrap());
            w.node(&st, 0).unwrap();
        }
        w.unwrap().finish().unwrap();
        let paths = [path];
        let ((mut lin, _), (mut seek, _)) =
            (Reader::open(&paths).unwrap(), Reader::open(&paths).unwrap());
        let (mut a, mut b) = (State::new(), State::new());
        for k in 0..=94 {
            lin.goto(&mut a, k).unwrap();
        }
        seek.goto(&mut b, 99).unwrap();
        seek.goto(&mut b, 94).unwrap();
        assert_eq!(dump(&b.nodes[0]), dump(&a.nodes[0]), "cgroup 3 is sampled every 30 s");
    }

    #[test]
    fn truncation_yields_the_valid_prefix() {
        let dir = tmp("trunc");
        let (path, _) = journal(&dir.0, "n1", 35);
        let full = fs::read(&path).unwrap();
        let (t, ..) = Track::open(&path).unwrap();
        let mut ends: Vec<u64> = t.index.iter().skip(1).map(|e| e.0).collect();
        ends.push(full.len() as u64 - 8);
        let cut = dir.0.join("cut.arrows");
        let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
        for i in 0..200 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let at =
                if i == 0 { full.len() } else { usize::try_from(x % full.len() as u64).unwrap() };
            fs::write(&cut, &full[..at]).unwrap();
            let complete = ends.iter().filter(|&&e| e <= at as u64).count();
            match Reader::open(std::slice::from_ref(&cut)) {
                Ok((mut r, warns)) => {
                    assert_eq!(r.tracks[0].index.len(), complete, "cut at {at}");
                    assert_eq!(warns.is_empty(), at == full.len(), "cut at {at}: {warns:?}");
                    if at < full.len() {
                        let want = format!("truncated at byte {}", ends[complete - 1]);
                        assert!(warns[0].ends_with(&want), "cut at {at}: {warns:?}");
                    }
                    r.goto(&mut State::new(), usize::MAX).unwrap();
                }
                Err(_) => assert_eq!(complete, 0, "cut at {at}"),
            }
        }
    }

    #[test]
    fn replay_batch_golden() {
        let dir = tmp("golden");
        let (p1, _) = journal(&dir.0, "n1", 14);
        let (p2, _) = journal(&dir.0, "n2", 7);
        let (mut r, _) = Reader::open(&[p1, p2]).unwrap();
        let theme = THEMES.iter().position(|t| t.name == "mono").unwrap();
        let opts = tui::Opts {
            source: "--replay".into(),
            interval_ms: 1000,
            rules: vec![],
            theme,
            local: false,
        };
        let mut app = App::new(opts, mpsc::channel().0);
        app.view.layout = Layout::Node;
        let k = r.find(at("+5s", r.times[0], r.times[13]).unwrap());
        r.goto(&mut app.st, k).unwrap();
        let mut out = Vec::new();
        snapshots(&mut app, &mut r, 3, &mut out).unwrap();
        let got = String::from_utf8(out).unwrap();
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/golden/batch-replay.txt");
        if std::env::var_os("UNJAM_BLESS").is_some() {
            fs::write(&path, &got).unwrap();
        }
        assert!(
            fs::read_to_string(&path).unwrap_or_default() == got,
            "UNJAM_BLESS=1 rewrites it:\n{got}"
        );
    }

    #[test]
    fn output_pattern_and_at() {
        assert_eq!(
            expand("rec/%j.%s-%n.arrows", Some("42"), Some("0"), "n1"),
            PathBuf::from("rec/42.0-n1.arrows")
        );
        assert_eq!(expand("%j-%s-%n-%%-%x%", None, None, ""), PathBuf::from("na-na-na-%-%x%"));
        let (first, last) = (DAY + 3_600_000, DAY + 7_200_000);
        for (s, want) in [
            ("start", first),
            ("end", last),
            ("+90s", first + 90_000),
            ("-1m", last - 60_000),
            ("01:30", DAY + 5_400_000),
            ("00:30:01", 2 * DAY + 1_801_000),
        ] {
            assert_eq!(at(s, first, last), Ok(want), "{s}");
        }
        for bad in ["soon", "1:2:3:4", "+2h", ""] {
            assert!(at(bad, first, last).is_err(), "{bad}");
        }
    }
}
