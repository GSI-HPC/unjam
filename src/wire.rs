// SPDX-License-Identifier: LGPL-3.0-or-later
// SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany

//! Text codecs: agent → TUI event lines and TUI → agent control lines (§6.2, §7). Every event
//! line is at most 1000 bytes, starts with the agent's node index and carries tab-separated
//! fields; NA is `-`.

use std::fmt::Write as _;

use crate::model::{Control, Ent, Event, Kind, NA, Rule, TaskSet};

/// Line grammar version sent in `H`; a TUI refuses nodes whose proto differs (§7.2).
pub const PROTO: u32 = 1;
/// Hard cap per line including `\n`: 24 bytes under slurmstepd's 1024-byte message size (D7).
pub const MAX_LINE: usize = 1000;
/// Cap for short strings (version, host, step, name, unit, comm), so fixed-field lines always fit.
const SHORT: usize = 64;
/// Room one more value needs: a tab and up to 20 digits.
const VAL_ROOM: usize = 21;

/// Appends `ev` for `node` as one line, or several `g`/`p` lines with increasing `first` when the
/// values do not fit one line. Allocates only if `buf` must grow.
pub fn write(buf: &mut Vec<u8>, node: u32, ev: &Event) {
    let (tag, id, first, v) = match *ev {
        Event::CgVals { id, first, v } => (b'g', id, first, v),
        Event::ProcVals { pid, first, v } => (b'p', u64::from(pid), first, v),
        _ => return line(buf, node, ev),
    };
    let mut i = 0;
    loop {
        let start = head(buf, node, tag);
        num(buf, id);
        num(buf, u64::from(first) + i as u64);
        while i < v.len() && buf.len() - start + VAL_ROOM < MAX_LINE {
            num(buf, v[i]);
            i += 1;
        }
        buf.push(b'\n');
        if i >= v.len() {
            break;
        }
    }
}

/// Writes every non-value event; strings are sanitized and the last one is cut to the cap.
fn line(buf: &mut Vec<u8>, node: u32, ev: &Event) {
    let tag = match ev {
        Event::Hello { .. } => b'H',
        Event::Desc { .. } => b'D',
        Event::Tick { .. } => b'T',
        Event::Cg { .. } => b'G',
        Event::CgGone(_) => b'X',
        Event::Proc { .. } => b'P',
        Event::ProcGone(_) => b'x',
        _ => b'W',
    };
    let start = head(buf, node, tag);
    let rest = |buf: &Vec<u8>| (MAX_LINE - 2).saturating_sub(buf.len() - start);
    match *ev {
        Event::Hello { proto, version, host, step, ntasks, btime } => {
            num(buf, proto.into());
            for s in [version, host, step] {
                text(buf, s, SHORT);
            }
            num(buf, ntasks.into());
            num(buf, btime);
        }
        Event::Desc { ent, idx, name, unit, kind } => {
            text(buf, if ent == Ent::Cgroup { "c" } else { "p" }, 1);
            num(buf, idx.into());
            text(buf, name, SHORT);
            text(buf, unit, SHORT);
            text(buf, if kind == Kind::Counter { "counter" } else { "gauge" }, 7);
        }
        Event::Tick { ts_ms, self_cpu_us, out_bytes, dropped, self_rss } => {
            for n in [ts_ms, self_cpu_us, out_bytes, dropped.into(), self_rss] {
                num(buf, n);
            }
        }
        Event::Cg { id, parent, path } => {
            num(buf, id);
            num(buf, parent);
            text(buf, path, rest(buf));
        }
        Event::Proc { pid, start: st, cg, ppid, comm, cmd } => {
            for n in [pid.into(), st, cg, ppid.into()] {
                num(buf, n);
            }
            text(buf, comm, SHORT);
            text(buf, cmd, rest(buf));
        }
        Event::CgGone(id) => num(buf, id),
        Event::ProcGone(pid) => num(buf, pid.into()),
        Event::Warn(t) => text(buf, t, rest(buf)),
        Event::CgVals { .. } | Event::ProcVals { .. } => {}
    }
    buf.push(b'\n');
}

/// Starts a line with `<node>\t<tag>` and returns where it began, for the length cap.
fn head(buf: &mut Vec<u8>, node: u32, tag: u8) -> usize {
    let start = buf.len();
    digits(buf, node.into());
    buf.extend_from_slice(&[b'\t', tag]);
    start
}

/// Appends `\t` and a decimal number, or `-` for NA.
fn num(buf: &mut Vec<u8>, n: u64) {
    buf.push(b'\t');
    if n == NA { buf.push(b'-') } else { digits(buf, n) }
}

/// Decimal digits without `format!`, so the hot path stays allocation-free.
#[allow(clippy::cast_possible_truncation, reason = "n % 10 < 10 always fits a u8")]
fn digits(buf: &mut Vec<u8>, n: u64) {
    if n >= 10 {
        digits(buf, n / 10);
    }
    buf.push(b'0' + (n % 10) as u8);
}

/// Appends `\t` and `s` sanitized and cut to at most `max` bytes.
fn text(buf: &mut Vec<u8>, s: &str, max: usize) {
    buf.push(b'\t');
    sanitize(s, max, buf);
}

/// Makes a string tab- and newline-safe: bytes below 0x20 become `?`, and it is cut at a char
/// boundary to at most `max` bytes. Paths and command lines are display-only, so cutting is safe.
pub fn sanitize(s: &str, max: usize, out: &mut Vec<u8>) {
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    out.extend(s.as_bytes()[..end].iter().map(|&b| if b < 0x20 { b'?' } else { b }));
}

/// Demultiplexes a received line by the node prefix that the agent wrote itself (D6).
#[cfg_attr(not(test), expect(dead_code, reason = "the TUI receive path decodes; not wired yet"))]
pub fn split_node(line: &[u8]) -> Option<(u32, &[u8])> {
    let tab = line.iter().position(|&b| b == b'\t')?;
    let node = u32::try_from(field(std::str::from_utf8(&line[..tab]).ok()?)?).ok()?;
    Some((node, &line[tab + 1..]))
}

/// Decodes one line (after `split_node`, with or without `\n`). Values land in `vals`, which the
/// event borrows. Extra trailing fields are ignored, so `H`, `D` and `T` can grow without a
/// proto bump; `None` means malformed.
#[cfg_attr(not(test), expect(dead_code, reason = "the TUI receive path decodes; not wired yet"))]
pub fn parse<'a>(line: &'a [u8], vals: &'a mut Vec<u64>) -> Option<Event<'a>> {
    let line = std::str::from_utf8(line.strip_suffix(b"\n").unwrap_or(line)).ok()?;
    let (tag, rest) = line.split_once('\t').unwrap_or((line, ""));
    let mut f = rest.split('\t');
    let mut n = || f.next().and_then(field);
    Some(match tag {
        "H" => hello(rest)?,
        "D" => desc(rest)?,
        "T" => Event::Tick {
            ts_ms: n()?,
            self_cpu_us: n()?,
            out_bytes: n()?,
            dropped: u32::try_from(n()?).ok()?,
            self_rss: n().unwrap_or(NA),
        },
        "G" => Event::Cg { id: n()?, parent: n()?, path: rest.splitn(3, '\t').nth(2)? },
        "X" => Event::CgGone(n()?),
        "x" => Event::ProcGone(u32::try_from(n()?).ok()?),
        "P" => proc(rest)?,
        "W" => Event::Warn(rest),
        "g" | "p" => {
            let (id, first) = (n()?, u16::try_from(n()?).ok()?);
            vals.clear();
            for v in f {
                vals.push(field(v)?);
            }
            let v: &'a Vec<u64> = vals;
            if tag == "g" {
                Event::CgVals { id, first, v }
            } else {
                Event::ProcVals { pid: u32::try_from(id).ok()?, first, v }
            }
        }
        _ => return None,
    })
}

/// Decodes the fields of an `H` line.
fn hello(rest: &str) -> Option<Event<'_>> {
    let mut f = rest.split('\t');
    let proto = f.next()?.parse().ok()?;
    let (version, host, step) = (f.next()?, f.next()?, f.next()?);
    let ntasks = f.next()?.parse().ok()?;
    Some(Event::Hello { proto, version, host, step, ntasks, btime: field(f.next()?)? })
}

/// Decodes the fields of a `D` line.
fn desc(rest: &str) -> Option<Event<'_>> {
    let mut f = rest.split('\t');
    let ent = match f.next()? {
        "c" => Ent::Cgroup,
        "p" => Ent::Proc,
        _ => return None,
    };
    let idx = f.next()?.parse().ok()?;
    let (name, unit) = (f.next()?, f.next()?);
    let kind = match f.next()? {
        "counter" => Kind::Counter,
        "gauge" => Kind::Gauge,
        _ => return None,
    };
    Some(Event::Desc { ent, idx, name, unit, kind })
}

/// Decodes the fields of a `P` line; the command line is the rest of the line.
fn proc(rest: &str) -> Option<Event<'_>> {
    let mut f = rest.splitn(6, '\t');
    let mut n = || f.next().and_then(|s| s.parse::<u64>().ok());
    let (id, start, cg, parent) = (n()?, n()?, n()?, n()?);
    let (comm, cmd) = (f.next()?, f.next()?);
    let pid = u32::try_from(id).ok()?;
    Some(Event::Proc { pid, start, cg, ppid: u32::try_from(parent).ok()?, comm, cmd })
}

/// A numeric field: decimal digits or `-` for NA.
fn field(s: &str) -> Option<u64> {
    if s == "-" {
        return Some(NA);
    }
    s.bytes().all(|b| b.is_ascii_digit()).then(|| s.parse().ok())?
}

/// Encodes a control line (`R`, `C`, `S`, `B` or `K`, §6.2) with its `\n`.
#[cfg_attr(not(test), expect(dead_code, reason = "the TUI sends control lines; not wired yet"))]
pub fn write_control(buf: &mut String, c: &Control) {
    let _ = match c {
        Control::Rule(r) => writeln!(buf, "R\t{r}"),
        Control::Clear(t) => writeln!(buf, "C\t{t}"),
        Control::Resync(t) => writeln!(buf, "S\t{t}"),
        Control::Budget(t, b) => writeln!(buf, "B\t{t}\t{b}"),
        Control::Keepalive => writeln!(buf, "K"),
    };
}

/// Decodes a control line; `Err` becomes one `W` line on the agent and changes nothing.
pub fn parse_control(line: &str) -> Result<Control, String> {
    let line = line.trim_end_matches(['\n', '\r']);
    let (tag, rest) = line.split_once('\t').unwrap_or((line, ""));
    match tag {
        "R" => Rule::parse(rest).map(Control::Rule),
        "C" => TaskSet::parse(rest).map(Control::Clear),
        "S" => TaskSet::parse(rest).map(Control::Resync),
        "B" => {
            let (t, b) = rest.split_once('\t').ok_or("B needs tasks and bytes per second")?;
            let b = b.parse().map_err(|_| format!("bad budget: {b:?}"))?;
            Ok(Control::Budget(TaskSet::parse(t)?, b))
        }
        "K" => Ok(Control::Keepalive),
        _ => Err(format!("unknown control line: {line:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::What;

    fn round_trip(ev: &Event) {
        let mut buf = Vec::new();
        write(&mut buf, 42, ev);
        assert_eq!(buf.split(|&b| b == b'\n').count(), 2, "one line: {buf:?}");
        assert!(buf.len() <= MAX_LINE);
        let (node, rest) = split_node(&buf).unwrap();
        assert_eq!(node, 42);
        let mut vals = Vec::new();
        assert_eq!(parse(rest, &mut vals).as_ref(), Some(ev), "{}", String::from_utf8_lossy(&buf));
    }

    #[test]
    fn every_variant_round_trips() {
        let v = [1, NA, 0, u64::MAX - 1];
        for ev in [
            Event::Hello {
                proto: 1,
                version: "0.1.0",
                host: "n1",
                step: "42.0",
                ntasks: 3,
                btime: 9,
            },
            Event::Desc {
                ent: Ent::Proc,
                idx: 4,
                name: "p_state",
                unit: "char",
                kind: Kind::Gauge,
            },
            Event::Desc { ent: Ent::Cgroup, idx: 0, name: "cg_x", unit: "us", kind: Kind::Counter },
            Event::Tick { ts_ms: 1, self_cpu_us: 2, out_bytes: 3, dropped: 4, self_rss: 5 },
            Event::Cg { id: 7, parent: 0, path: "" },
            Event::Cg { id: 8, parent: 7, path: "step_0/user" },
            Event::CgVals { id: 8, first: 0, v: &v },
            Event::CgVals { id: 8, first: 3, v: &[] },
            Event::CgGone(8),
            Event::Proc { pid: 9, start: 10, cg: 8, ppid: 1, comm: "x) (y", cmd: "a b  c" },
            Event::ProcVals { pid: 9, first: 0, v: &v },
            Event::ProcGone(9),
            Event::Warn("no io: permission denied"),
        ] {
            round_trip(&ev);
        }
    }

    #[test]
    fn extra_trailing_fields_are_ignored() {
        let mut vals = Vec::new();
        let ev = parse(b"T\t1\t2\t3\t4\t5\t6\n", &mut vals).unwrap();
        assert_eq!(
            ev,
            Event::Tick { ts_ms: 1, self_cpu_us: 2, out_bytes: 3, dropped: 4, self_rss: 5 }
        );
        let ev = parse(b"T\t1\t2\t3\t4", &mut vals).unwrap();
        assert!(matches!(ev, Event::Tick { self_rss: NA, .. }), "old agents: rss is NA");
        assert!(matches!(
            parse(b"H\t1\tv\th\t-\t2\t3\textra", &mut vals),
            Some(Event::Hello { .. })
        ));
        assert!(matches!(
            parse(b"D\tc\t0\tn\tus\tgauge\tcost", &mut vals),
            Some(Event::Desc { .. })
        ));
    }

    #[test]
    fn malformed_lines() {
        let mut vals = Vec::new();
        for bad in
            [&b"Q\t1"[..], b"T\t1\t2", b"g\t1\t0\tx", b"x\t-1", b"D\tq\t0\tn\tu\tgauge", b"\xff"]
        {
            assert_eq!(parse(bad, &mut vals), None, "{bad:?}");
        }
        assert_eq!(split_node(b"junk line"), None);
        assert_eq!(split_node(b"+1\tT"), None);
        assert_eq!(split_node(b"\tT"), None);
        assert_eq!(split_node(b"17\tX\t3"), Some((17, &b"X\t3"[..])));
    }

    #[test]
    fn strings_are_sanitized_and_capped() {
        let mut out = Vec::new();
        sanitize("a\tb\nc\u{1}é", 64, &mut out);
        assert_eq!(out, "a?b?c?é".as_bytes());
        out.clear();
        sanitize("éé", 3, &mut out);
        assert_eq!(out, "é".as_bytes(), "cut at a char boundary");

        let long = "ü/".repeat(2000);
        for ev in [
            Event::Cg { id: u64::MAX - 1, parent: u64::MAX - 1, path: &long },
            Event::Proc { pid: u32::MAX, start: 1, cg: 2, ppid: 3, comm: &long, cmd: &long },
            Event::Warn(&long),
            Event::Hello {
                proto: 1,
                version: &long,
                host: &long,
                step: &long,
                ntasks: 1,
                btime: 1,
            },
        ] {
            let mut buf = Vec::new();
            write(&mut buf, u32::MAX, &ev);
            assert!(buf.len() <= MAX_LINE, "{} bytes", buf.len());
            let (_, rest) = split_node(&buf).unwrap();
            assert!(parse(rest, &mut Vec::new()).is_some());
        }
    }

    #[test]
    fn values_split_with_offsets() {
        let v: Vec<u64> = (0..200).map(|i| if i % 7 == 0 { NA } else { u64::MAX - i }).collect();
        let mut buf = Vec::new();
        write(&mut buf, 999, &Event::ProcVals { pid: 5, first: 10, v: &v });
        let mut got = vec![NA; 210];
        let mut lines = 0;
        for line in buf.split_inclusive(|&b| b == b'\n') {
            assert!(line.len() <= MAX_LINE);
            lines += 1;
            let mut vals = Vec::new();
            let (_, rest) = split_node(line).unwrap();
            let Some(Event::ProcVals { pid: 5, first, v }) = parse(rest, &mut vals) else {
                panic!()
            };
            got[usize::from(first)..usize::from(first) + v.len()].copy_from_slice(v);
        }
        assert!(lines >= 4, "{lines} lines");
        assert_eq!(&got[10..], &v[..]);
    }

    #[test]
    fn control_round_trips() {
        let r = Rule::parse("0-3\tstep_0\t500\tp").unwrap();
        for c in [
            Control::Rule(r),
            Control::Rule(Rule::parse("*\t*/user\t-\to").unwrap()),
            Control::Clear(TaskSet::All),
            Control::Resync(TaskSet::parse("2,4-5").unwrap()),
            Control::Budget(TaskSet::All, 4_194_304),
            Control::Keepalive,
        ] {
            let mut s = String::new();
            write_control(&mut s, &c);
            assert!(s.ends_with('\n'));
            assert_eq!(parse_control(&s), Ok(c));
        }
        let rule = parse_control(&format!("R\t{}", "1-2\t.\t2000\tc")).unwrap();
        assert!(matches!(rule, Control::Rule(Rule { what: What::Cg, interval_ms: 2000, .. })));
        for bad in ["Z", "R\t*\t.\t-\tp", "B\t*", "B\t*\tx", "C\t3-1"] {
            assert!(parse_control(bad).is_err(), "{bad:?}");
        }
    }
}
