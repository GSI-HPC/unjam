// SPDX-License-Identifier: LGPL-3.0-or-later
// SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany

//! Metric descriptors, the borrowed interchange event, and the sampling rules that the agent and
//! the TUI share, so that the INT and W columns match what the agents do (§4, §6).

use std::fmt;

/// Missing value (controller absent, file unreadable): keeps every value at 8 bytes, no `Option`.
pub const NA: u64 = u64::MAX;

/// How values of a metric are shown: counters as rates, gauges as they are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Counter,
    Gauge,
}

/// The entity a metric or source belongs to; indexes the per-entity column tables.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ent {
    Cgroup,
    Proc,
}

/// One metric descriptor: drives the wire `D` lines, the rate math and the record schema (R10).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Metric {
    pub name: &'static str,
    pub unit: &'static str,
    pub kind: Kind,
}

/// The single interchange type between sampler, wire, record, replay and state. It borrows its
/// data, so producing and consuming one allocates nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event<'a> {
    Hello { proto: u32, version: &'a str, host: &'a str, step: &'a str, ntasks: u32, btime: u64 },
    Desc { ent: Ent, idx: u16, name: &'a str, unit: &'a str, kind: Kind },
    Tick { ts_ms: u64, self_cpu_us: u64, out_bytes: u64, dropped: u32, self_rss: u64 },
    Cg { id: u64, parent: u64, path: &'a str },
    CgVals { id: u64, first: u16, v: &'a [u64] },
    CgGone(u64),
    Proc { pid: u32, start: u64, cg: u64, ppid: u32, comm: &'a str, cmd: &'a str },
    ProcVals { pid: u32, first: u16, v: &'a [u64] },
    ProcGone(u32),
    Warn(&'a str),
}

impl Event<'_> {
    /// Value lines may be dropped under backpressure without loss (absolute values, D8); every
    /// other line is structure or liveness and is always delivered (§7.5).
    pub fn is_value(&self) -> bool {
        matches!(self, Event::CgVals { .. } | Event::ProcVals { .. })
    }
}

/// Sampling level of a subtree, ordered by depth (wire `o`, `c`, `p`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum What {
    Off,
    Cg,
    Procs,
}

/// The agents a control line addresses: `*` or ranges of `SLURM_PROCID` such as `0-15,32`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskSet {
    All,
    Ranges(Vec<(u32, u32)>),
}

impl TaskSet {
    /// Parses `*` or comma-separated ranges; rejects empty, reversed and non-numeric parts.
    pub fn parse(s: &str) -> Result<Self, String> {
        if s == "*" {
            return Ok(Self::All);
        }
        let num = |t: &str| t.parse::<u32>().map_err(|_| format!("bad task set: {s:?}"));
        let mut ranges = Vec::new();
        for part in s.split(',') {
            let (a, b) = part.split_once('-').unwrap_or((part, part));
            let (a, b) = (num(a)?, num(b)?);
            if a > b {
                return Err(format!("bad task range: {part:?}"));
            }
            ranges.push((a, b));
        }
        Ok(Self::Ranges(ranges))
    }

    /// Whether the agent with this `SLURM_PROCID` must act on the line.
    pub fn contains(&self, task: u32) -> bool {
        match self {
            Self::All => true,
            Self::Ranges(r) => r.iter().any(|&(a, b)| (a..=b).contains(&task)),
        }
    }
}

impl fmt::Display for TaskSet {
    /// Writes the wire form that `TaskSet::parse` reads back.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self::Ranges(ranges) = self else { return f.write_str("*") };
        for (i, &(a, b)) in ranges.iter().enumerate() {
            let sep = if i == 0 { "" } else { "," };
            if a == b { write!(f, "{sep}{a}")? } else { write!(f, "{sep}{a}-{b}")? }
        }
        Ok(())
    }
}

/// One selective-sampling rule (R6, R7): which agents, which subtree, how often, how deep.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub tasks: TaskSet,
    pub pattern: Box<str>,
    pub interval_ms: u32,
    pub what: What,
}

impl Rule {
    /// Parses the fields of an `R` line after `R\t` (also the `--rule` text). `Off` takes the
    /// interval `-` and only `Off` does (D21); intervals round to 100 ms within 100 ms…60 s.
    pub fn parse(s: &str) -> Result<Self, String> {
        let mut f = s.split('\t');
        let (Some(tasks), Some(pattern), Some(interval), Some(what)) =
            (f.next(), f.next(), f.next(), f.next())
        else {
            return Err(format!("rule needs tasks, pattern, interval, level: {s:?}"));
        };
        let what = match what {
            "o" => What::Off,
            "c" => What::Cg,
            "p" => What::Procs,
            _ => return Err(format!("rule level must be o, c or p: {what:?}")),
        };
        let interval_ms = match (interval, what) {
            ("-", What::Off) => 0,
            ("-", _) => return Err("interval - needs level o".to_owned()),
            (_, What::Off) => return Err("level o takes interval -".to_owned()),
            (n, _) => {
                let n: u32 = n.parse().map_err(|_| format!("bad interval: {n:?}"))?;
                (n.saturating_add(50) / 100 * 100).clamp(100, 60_000)
            }
        };
        if pattern.is_empty() || pattern.split('/').any(str::is_empty) {
            return Err(format!("bad pattern: {pattern:?}"));
        }
        Ok(Self { tasks: TaskSet::parse(tasks)?, pattern: pattern.into(), interval_ms, what })
    }
}

impl fmt::Display for Rule {
    /// Writes the `R` fields that `Rule::parse` reads back.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}\t{}\t", self.tasks, self.pattern)?;
        match self.what {
            What::Off => f.write_str("-\to"),
            What::Cg => write!(f, "{}\tc", self.interval_ms),
            What::Procs => write!(f, "{}\tp", self.interval_ms),
        }
    }
}

/// A TUI → agent control line (§6.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Control {
    Rule(Rule),
    Clear(TaskSet),
    Resync(TaskSet),
    Budget(TaskSet, u64),
    Keepalive,
}

impl Control {
    /// Whether the agent with this `SLURM_PROCID` must act on the line; others ignore it.
    pub fn applies(&self, task: u32) -> bool {
        match self {
            Self::Rule(r) => r.tasks.contains(task),
            Self::Clear(t) | Self::Resync(t) | Self::Budget(t, _) => t.contains(task),
            Self::Keepalive => true,
        }
    }
}

/// Segment-wise prefix match: `.` matches everything, `*` any one segment, and a pattern of k
/// segments covers the whole subtree below the first k segments of `path` (root is `""`).
pub fn pattern_match(pat: &str, path: &str) -> bool {
    if pat == "." {
        return true;
    }
    let mut segs = path.split('/').filter(|s| !s.is_empty());
    pat.split('/').all(|p| segs.next().is_some_and(|s| p == "*" || p == s))
}

/// The one rule resolver used by agent and TUI: the last matching rule wins; no match is `Off`.
pub fn effective(rules: &[Rule], task: u32, path: &str) -> (u32, What) {
    rules
        .iter()
        .rev()
        .find(|r| r.tasks.contains(task) && pattern_match(&r.pattern, path))
        .map_or((0, What::Off), |r| (r.interval_ms, r.what))
}

/// Applies an `R` line to a rule list: replaces the rule with the same task set and pattern in
/// place, else appends. The agent and the TUI both use it, so the agent's list is the TUI's
/// list restricted to the rules that apply to it, and both resolve every path alike (§6.2).
pub fn upsert(rules: &mut Vec<Rule>, r: Rule) {
    match rules.iter_mut().find(|x| x.tasks == r.tasks && x.pattern == r.pattern) {
        Some(x) => *x = r,
        None => rules.push(r),
    }
}

/// Wall-clock phase alignment: the next multiple of `interval_ms` strictly after `now_ms`, so a
/// tick on an exact multiple schedules the following one (§6.1).
pub fn next_due(now_ms: u64, interval_ms: u32) -> u64 {
    let i = u64::from(interval_ms.max(1));
    (now_ms / i + 1) * i
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(s: &str) -> Rule {
        Rule::parse(s).unwrap()
    }

    #[test]
    fn task_sets() {
        for (text, task, want) in [
            ("*", 7, true),
            ("3", 3, true),
            ("3", 4, false),
            ("0-15,32,40-41", 15, true),
            ("0-15,32,40-41", 16, false),
            ("0-15,32,40-41", 32, true),
            ("0-15,32,40-41", 41, true),
        ] {
            let set = TaskSet::parse(text).unwrap();
            assert_eq!(set.contains(task), want, "{text} {task}");
            assert_eq!(set.to_string(), text);
        }
        for bad in ["", "a", "3-1", "1,,2", "-1", "1-"] {
            assert!(TaskSet::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn rule_parse() {
        let r = rule("0-1\tstep_0\t500\tp");
        assert_eq!((r.interval_ms, r.what, &*r.pattern), (500, What::Procs, "step_0"));
        assert_eq!(rule("*\t*/user\t-\to").what, What::Off);
        assert_eq!(rule("*\t.\t1049\tc").interval_ms, 1000);
        assert_eq!(rule("*\t.\t1050\tc").interval_ms, 1100);
        assert_eq!(rule("*\t.\t1\tc").interval_ms, 100);
        assert_eq!(rule("*\t.\t999999\tc").interval_ms, 60_000);
        for bad in [
            "*\t.\t-\tp",
            "*\t.\t500\to",
            "*\t.\t500\td",
            "*\t.\t500",
            "*\t\t500\tc",
            "*\ta//b\t500\tc",
            "x\t.\t500\tc",
        ] {
            assert!(Rule::parse(bad).is_err(), "{bad:?}");
        }
        for text in ["*\t.\t1000\tp", "3,5-6\tstep_0/user\t200\tc", "*\t*/user\t-\to"] {
            assert_eq!(rule(text).to_string(), text);
        }
    }

    #[test]
    fn patterns() {
        for (pat, path, want) in [
            (".", "", true),
            (".", "a/b", true),
            ("step_0", "", false),
            ("step_0", "step_0", true),
            ("step_0", "step_0/user/task_0", true),
            ("step_0", "step_01", false),
            ("*/user", "step_0", false),
            ("*/user", "step_0/user", true),
            ("*/user", "step_3/user/task_1", true),
            ("*/user", "step_3/slurm", false),
            ("a/b/c", "a/b", false),
        ] {
            assert_eq!(pattern_match(pat, path), want, "{pat} {path}");
        }
    }

    #[test]
    fn effective_last_match_wins() {
        let rules =
            [rule("*\t.\t2000\tc"), rule("*\t*/user\t-\to"), rule("1\tstep_0/user\t500\tp")];
        for (task, path, want) in [
            (0, "", (2000, What::Cg)),
            (0, "step_0", (2000, What::Cg)),
            (0, "step_0/user/task_0", (0, What::Off)),
            (1, "step_0/user/task_0", (500, What::Procs)),
            (1, "step_1/user", (0, What::Off)),
        ] {
            assert_eq!(effective(&rules, task, path), want, "{task} {path}");
        }
        assert_eq!(effective(&[], 0, ""), (0, What::Off));
    }

    #[test]
    fn upsert_keeps_agent_lists_in_step_with_the_tui() {
        let seq = ["*\t.\t2000\tc", "*\t*/user\t-\to", "*\t.\t1000\tc", "1\tstep_0/user\t500\tp"];
        let seq =
            seq.iter().chain(&["0-1\t.\t500\tp", "*\t*/user\t300\tp", "1\tstep_0/user\t-\to"]);
        let (mut tui, mut agents) = (vec![], [(0, vec![]), (1, vec![])]);
        for r in seq.map(|s| rule(s)) {
            for (_, list) in agents.iter_mut().filter(|a| r.tasks.contains(a.0)) {
                upsert(list, r.clone());
            }
            upsert(&mut tui, r);
            for (t, list) in &agents {
                for path in ["", "step_0", "step_0/user", "step_0/user/task_0", "step_1/user"] {
                    assert_eq!(effective(&tui, *t, path), effective(list, *t, path), "{t} {path}");
                }
            }
        }
        assert_eq!(effective(&tui, 0, "step_0/user/task_0"), (500, What::Procs));
        assert_eq!(tui.len(), 4);
    }

    #[test]
    fn next_due_table() {
        for (now, iv, want) in [
            (0, 1000, 1000),
            (999, 1000, 1000),
            (1000, 1000, 2000),
            (1001, 1000, 2000),
            (1_700_000_000_250, 500, 1_700_000_000_500),
            (1_700_000_000_500, 500, 1_700_000_001_000),
        ] {
            assert_eq!(next_due(now, iv), want, "{now} {iv}");
        }
    }

    #[test]
    fn control_scope() {
        assert!(Control::Keepalive.applies(9));
        assert!(Control::Clear(TaskSet::parse("1-2").unwrap()).applies(2));
        assert!(!Control::Resync(TaskSet::parse("1-2").unwrap()).applies(3));
        assert!(!Control::Rule(rule("0\t.\t1000\tp")).applies(1));
        assert!(Event::CgVals { id: 1, first: 0, v: &[] }.is_value());
        assert!(
            !Event::Tick { ts_ms: 0, self_cpu_us: 0, out_bytes: 0, dropped: 0, self_rss: 0 }
                .is_value()
        );
    }
}
