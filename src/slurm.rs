// SPDX-License-Identifier: LGPL-3.0-or-later
// SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany

//! Slurm attach: job resolution, the one place the srun command line is built, the srun child
//! with its reader and control threads, and the staged teardown (§3.2, §3.3, D43).

use std::ffi::OsString;
use std::hash::{BuildHasher, RandomState};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::time::{Duration, Instant};
use std::{fs, thread};

use crate::SlurmArgs;
use crate::tui::{self, App, Msg, Opts};

/// Idle time after which the control thread sends `K`, well inside the agents' keepalive.
const KEEPALIVE: Duration = Duration::from_secs(30);

/// The squeue/scontrol result the srun line needs: the numeric job id, the agent count and the
/// owner's uid.
#[derive(Debug)]
pub struct Job {
    pub id: String,
    pub nodes: u32,
    pub owner: u32,
}

/// Runs a Slurm command and returns its stdout; its stderr goes to ours.
fn run(cmd: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(cmd).args(args).stderr(Stdio::inherit()).output();
    let out = out.map_err(|e| format!("{cmd}: {e}"))?;
    if !out.status.success() {
        return Err(format!("{cmd} {} failed ({})", args.join(" "), out.status));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// A `squeue` row: job id, node count, state, owner uid and name.
type Row = (String, u32, String, u32, String);

/// `squeue` rows; array tasks resolve to the real id.
fn squeue(args: &[&str]) -> Result<Vec<Row>, String> {
    let text = run("squeue", &[&["--noheader", "--format=%A %D %T %U %j"], args].concat())?;
    let row = |l: &str| {
        let mut f = l.splitn(5, ' ');
        let (id, n, state) = (f.next()?.into(), f.next()?.parse().ok()?, f.next()?.into());
        Some((id, n, state, f.next()?.parse().ok()?, f.next().unwrap_or("").into()))
    };
    Ok(text.lines().filter_map(row).collect())
}

/// The running row with id `want`, else the only running row. `Err` holds the running rows
/// when none or several fit: an array job id lists every task plus the pending rest.
fn running(mut rows: Vec<Row>, want: &str) -> Result<Row, Vec<Row>> {
    rows.retain(|r| r.2 == "RUNNING");
    let i = rows.iter().position(|r| r.0 == want).or((rows.len() == 1).then_some(0));
    i.map(|i| rows.swap_remove(i)).ok_or(rows)
}

/// Lists the running jobs `rows` and exits 2 (no terminal is set up yet): `what` names no
/// single one (D43).
fn pick(rows: &[Row], what: &str) -> ! {
    let mut e = io::stderr().lock();
    let _ = writeln!(e, "unjam: {} running jobs {what}; pick one with --slurm=JOBID", rows.len());
    for (id, n, .., name) in rows {
        let _ = writeln!(e, "  {id:>10}  {n:>5} nodes  {name}");
    }
    std::process::exit(2)
}

/// D43 job resolution: the `--slurm` value, else `$SLURM_JOB_ID`, else the only running job
/// of `squeue --me`.
fn resolve(arg: &str) -> Result<String, String> {
    let env = || std::env::var("SLURM_JOB_ID").ok().filter(|j| !j.is_empty());
    if let Some(id) = Some(arg.to_owned()).filter(|a| !a.is_empty()).or_else(env) {
        return Ok(id);
    }
    let mine = squeue(&["--me", "--states=RUNNING"])?;
    Ok(running(mine, "").unwrap_or_else(|rows| pick(&rows, "of yours")).0)
}

/// The job to attach to; it must be running. An array job id with several running tasks lists
/// them like [`resolve`]. `--nodelist` sets the agent count (§3.2).
pub fn job(arg: &str, nodelist: Option<&str>) -> Result<Job, String> {
    let want = resolve(arg)?;
    let rows = squeue(&[&format!("--jobs={want}")])?;
    let state = rows.first().map(|r| r.2.clone()).ok_or_else(|| format!("job {want} not found"))?;
    let (id, nodes, _, owner, _) = running(rows, &want).or_else(|r| match r.len() {
        0 => Err(format!("job {want} is {state}, not RUNNING")),
        _ => pick(&r, &format!("in array {want}")),
    })?;
    let Some(list) = nodelist else { return Ok(Job { id, nodes, owner }) };
    let n = run("scontrol", &["show", "hostnames", list])?.lines().count();
    Ok(Job { id, nodes: u32::try_from(n).unwrap_or(u32::MAX), owner })
}

/// The srun argv, `srun` first (§3.2). With `--bcast` srun copies the agent to
/// `DIR/unjam.<job>.<uid>.<nonce>`, and the copy removes itself at start (D36).
pub fn srun_args(job: &Job, a: &SlurmArgs, exe: PathBuf, uid: u32, nonce: u64) -> Vec<OsString> {
    let (id, n) = (&job.id, job.nodes);
    let mut v: Vec<String> = vec!["srun".into(), format!("--jobid={id}"), "--overlap".into()];
    v.extend([format!("--nodes={n}"), format!("--ntasks={n}"), "--ntasks-per-node=1".into()]);
    v.extend(a.nodelist.as_ref().map(|l| format!("--nodelist={l}")));
    let fixed = ["--cpu-bind=none", "--mpi=none", "--input=all", "--quiet", "--kill-on-bad-exit=0"];
    v.extend(fixed.iter().chain(&["--wait=0", "--job-name=unjam", "--chdir=/"]).map(|&s| s.into()));
    let dir = a.bcast.as_ref().map(|d| d.trim_end_matches('/'));
    v.extend(dir.map(|d| format!("--bcast={d}/unjam.{id}.{uid}.{nonce:016x}")));
    v.extend(a.srun_arg.iter().cloned());
    let mut v: Vec<OsString> = v.into_iter().map(Into::into).collect();
    v.extend([a.agent.clone().unwrap_or(exe).into(), "agent".into()]);
    v.extend(a.agent_keepalive.iter().flat_map(|k| ["--keepalive".into(), k.to_string().into()]));
    v.extend(a.bcast.as_ref().map(|_| "--unlink-self".into()));
    v
}

/// Owns the srun child for the staged teardown. Control lines reach srun's stdin through the
/// control thread, which holds the only handle to it.
pub struct Remote(Child);

/// Starts srun in its own process group (terminal SIGINT and SIGTSTP never reach it), without
/// inherited `SLURM_*`/`SRUN_*` input variables except `SLURM_CONF`, plus the threads that
/// feed `tx` and forward `ctl`.
pub fn spawn(argv: &[OsString], tx: &SyncSender<Msg>, ctl: Receiver<String>) -> io::Result<Remote> {
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]).process_group(0);
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    for (k, _) in std::env::vars_os() {
        let s = k.to_string_lossy();
        if (s.starts_with("SLURM_") || s.starts_with("SRUN_")) && s != "SLURM_CONF" {
            cmd.env_remove(&k);
        }
    }
    let mut child = cmd.spawn()?;
    let pipe = || io::Error::other("srun pipe missing");
    let (stdin, out) =
        (child.stdin.take().ok_or_else(pipe)?, child.stdout.take().ok_or_else(pipe)?);
    let err = child.stderr.take().ok_or_else(pipe)?;
    thread::spawn(move || control(stdin, &ctl));
    let (t_out, t_err) = (tx.clone(), tx.clone());
    thread::spawn(move || lines(out, &t_out));
    thread::spawn(move || {
        let mut it = BufReader::new(err).lines().map_while(Result::ok);
        it.try_for_each(|l| t_err.send(Msg::Stderr(l)))
    });
    Ok(Remote(child))
}

/// Forwards control lines to srun, which broadcasts them to every agent, and sends `K` when
/// idle. Returns once the viewer drops its sender: closing stdin is teardown step 1.
fn control(mut w: ChildStdin, ctl: &Receiver<String>) {
    let next = || match ctl.recv_timeout(KEEPALIVE) {
        Err(RecvTimeoutError::Timeout) => Some("K\n".into()),
        r => r.ok(),
    };
    let _ = std::iter::from_fn(next).try_for_each(|l: String| w.write_all(l.as_bytes()));
}

/// The stdout reader (§7.3): whole lines in chunks of up to 64 KiB, `Msg::End` at EOF. A
/// "line" that fills the whole buffer is junk and dropped.
fn lines(mut r: impl Read, tx: &SyncSender<Msg>) {
    let (mut buf, mut keep) = (vec![0; 1 << 16], 0);
    while let Ok(n @ 1..) = r.read(&mut buf[keep..]) {
        let end = keep + n;
        let cut = buf[..end].iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
        if cut > 0 && tx.send(Msg::Lines(buf[..cut].to_vec())).is_err() {
            return;
        }
        buf.copy_within(cut..end, 0);
        keep = if end - cut == buf.len() { 0 } else { end - cut };
    }
    let _ = tx.send(Msg::End);
}

impl Remote {
    /// Teardown after stdin was closed (§3.3): wait up to 3 s for srun, then `scancel` the
    /// agents' step, wait 3 s more, then SIGKILL.
    pub fn stop(mut self, step: Option<&str>) {
        let mut gone = |ms: u64| {
            let end = Instant::now() + Duration::from_millis(ms);
            while matches!(self.0.try_wait(), Ok(None)) && Instant::now() < end {
                thread::sleep(Duration::from_millis(50));
            }
            !matches!(self.0.try_wait(), Ok(None))
        };
        if gone(3000) {
            return;
        }
        if let Some(s) = step {
            let _ = Command::new("scancel").args(["--quiet", s]).status();
        }
        if !gone(3000) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

/// `unjam --slurm[=JOBID]`: resolves the job, starts one agent per node and runs the viewer on
/// their output, then tears down. Exits 1 with srun's stderr tail if no agent ever reported.
pub fn view(arg: &str, a: &SlurmArgs, opts: Opts, batch: Option<u32>) -> Result<(), String> {
    let job = job(arg, a.nodelist.as_deref())?;
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    // srun opens the copy with O_TRUNC, not O_EXCL: a name other users cannot guess keeps them
    // from planting the file. RandomState's keys come from the OS random source.
    let nonce = RandomState::new().hash_one(job.id.as_str());
    let uid = fs::metadata("/proc/self").map_or(0, |m| m.uid());
    // A job-private /tmp (job_container/tmpfs) belongs to the job's owner, who could swap the copy.
    if a.bcast.is_some() && job.owner != uid {
        return Err(format!("--bcast: job owner {} could swap the copy; use --agent", job.owner));
    }
    let argv = srun_args(&job, a, exe, uid, nonce);
    if a.print_srun {
        let line: Vec<_> = argv.iter().map(|s| s.to_string_lossy()).collect();
        return writeln!(io::stdout(), "{}", line.join(" ")).map_err(|e| e.to_string());
    }
    let ((tx, rx), (ctl, ctl_rx)) = (mpsc::sync_channel(64), mpsc::channel());
    let remote = spawn(&argv, &tx, ctl_rx).map_err(|e| format!("srun: {e}"))?;
    let mut app = App::new(Opts { source: job.id.clone(), ..opts }, ctl.clone());
    app.budget = a.budget;
    app.start(job.nodes);
    if let Some(dir) = a.record.clone() {
        app.rec_dir = Some(dir);
        app.record();
    }
    drop(ctl);
    let res = tui::view(&mut app, &tx, &rx, batch);
    let step = app.st.nodes.iter().find(|n| n.step.contains('.')).map(|n| n.step.clone());
    let log = || app.log.iter().rev().cloned().collect::<Vec<_>>().join("\n");
    let failed = app.st.nodes.is_empty().then(log);
    drop(app);
    // Keep the readers reading srun's pipes, so srun can forward stdin EOF and exit (§3.3).
    thread::spawn(move || rx.into_iter().for_each(drop));
    remote.stop(step.as_deref());
    res.map_err(|e| format!("tui: {e}"))?;
    failed.map_or(Ok(()), |tail| Err(format!("no agent reported; srun said:\n{tail}")))
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;
    use crate::Cli;

    /// The argv for `flags`, as `--print-srun` shows it.
    fn argv(flags: &str, nodes: u32) -> String {
        let cli = Cli::try_parse_from(["unjam", "--slurm=42"].into_iter().chain(flags.split(' ')))
            .unwrap();
        let job = Job { id: "42".into(), nodes, owner: 1000 };
        let v = srun_args(&job, &cli.srun, "/opt/unjam".into(), 1000, 0xbc);
        v.iter().map(|s| s.to_string_lossy()).collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn srun_args_golden() {
        let fixed = "--cpu-bind=none --mpi=none --input=all --quiet --kill-on-bad-exit=0 --wait=0 \
                     --job-name=unjam --chdir=/";
        let base =
            |n| format!("srun --jobid=42 --overlap --nodes={n} --ntasks={n} --ntasks-per-node=1");
        assert_eq!(argv("--batch 1", 3), format!("{} {fixed} /opt/unjam agent", base(3)));
        assert_eq!(
            argv("--nodelist=n[1-2]", 2),
            format!("{} --nodelist=n[1-2] {fixed} /opt/unjam agent", base(2))
        );
        assert_eq!(
            argv("--srun-arg=--exclusive --srun-arg --gres=none --agent /x/unjam", 3),
            format!("{} {fixed} --exclusive --gres=none /x/unjam agent", base(3))
        );
        assert_eq!(
            argv("--agent-keepalive 5", 3),
            format!("{} {fixed} /opt/unjam agent --keepalive 5", base(3))
        );
        let bcast = "--bcast=/tmp/unjam.42.1000.00000000000000bc /opt/unjam agent --unlink-self";
        assert_eq!(argv("--bcast", 3), format!("{} {fixed} {bcast}", base(3)));
        let bcast =
            "--bcast=/var/tmp/unjam.42.1000.00000000000000bc /opt/unjam agent --unlink-self";
        assert_eq!(argv("--bcast=/var/tmp/", 3), format!("{} {fixed} {bcast}", base(3)));
    }

    /// A `squeue` row for the tests.
    fn row(id: &str, state: &str) -> Row {
        (id.into(), 2, state.into(), 1000, "job".into())
    }

    #[test]
    fn array_job_id_resolves_to_its_running_task() {
        // `squeue --jobs=1234` for `--array=1-4%2`: the pending rest keeps the array id and sorts
        // first, the running tasks have their own ids.
        let ids = |r: Result<Row, Vec<Row>>| r.map_err(|v| v.into_iter().map(|r| r.0).collect());
        let pending = row("1234", "PENDING");
        let two = vec![pending.clone(), row("1235", "RUNNING"), row("1236", "RUNNING")];
        assert_eq!(
            ids(running(two.clone(), "1234")),
            Err::<Row, Vec<_>>(vec!["1235".into(), "1236".into()])
        );
        assert_eq!(ids(running(two[..2].to_vec(), "1234")), Ok(row("1235", "RUNNING")));
        assert_eq!(ids(running(vec![pending], "1234")), Err(vec![]));
        // Once every task runs, the last one started carries the array id.
        let all = vec![row("1235", "RUNNING"), row("1234", "RUNNING"), row("1236", "RUNNING")];
        assert_eq!(ids(running(all, "1234")), Ok(row("1234", "RUNNING")));
        assert_eq!(
            ids(running(vec![row("1240", "RUNNING")], "1234_6")),
            Ok(row("1240", "RUNNING"))
        );
    }

    #[test]
    fn reader_splits_whole_lines_and_ends() {
        let (tx, rx) = mpsc::sync_channel(8);
        let mut data = b"0\tT 1\n1\tT".to_vec();
        data.extend_from_slice(b" 2\n2\tpartial");
        lines(&data[..], &tx);
        let got: Vec<_> = rx.try_iter().collect();
        assert!(matches!(&got[..], [Msg::Lines(b), Msg::End] if b == b"0\tT 1\n1\tT 2\n"));
    }
}
