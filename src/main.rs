// SPDX-License-Identifier: LGPL-3.0-or-later
// SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany

use std::{io::Write as _, path::PathBuf, process::ExitCode, time::Duration};

use clap::{Args, Parser, Subcommand};

/// The sampler, the shared run loop, the output path and the local-mode backend.
mod agent;
/// The cgroup v2 source, discovery and the Slurm job filter.
mod cgroup;
/// Metric descriptors, the interchange event and the sampling rules.
mod model;
/// The /proc source and the constants read from /proc.
mod procfs;
/// Record and replay: the recorder, the Arrow journal writer and reader, and TUI recording.
mod record;
/// Slurm attach: job resolution, the srun line, the srun child and its teardown.
mod slurm;
/// The metric-source plug-in surface and the fd cache.
mod source;
/// The TUI data store and the rows of the three layouts.
mod state;
/// Theme slots, glyphs and the compiled-in themes.
mod theme;
/// The viewer: event loop, keys, drawing and `--batch` output.
mod tui;
/// The agent ↔ TUI text codecs.
mod wire;

/// unjam - Utility for Node/Job Analysis and Monitoring
///
/// Shows this host's cgroup v2 tree with its processes, a Slurm job (--slurm) or recordings
/// (--replay).
#[derive(Parser, Debug)]
#[allow(clippy::doc_markdown, reason = "the doc comments are clap help text, not rustdoc")]
#[command(version, about, args_conflicts_with_subcommands = true)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
    /// Attach to a Slurm job (default: $SLURM_JOB_ID, else your only running job).
    #[arg(long, value_name = "JOBID", num_args = 0..=1, require_equals = true,
          default_missing_value = "", group = "source")]
    slurm: Option<String>,
    /// Replay recordings, one .arrows file per node.
    #[arg(long, value_name = "FILE.arrows", num_args = 1.., group = "source")]
    replay: Vec<PathBuf>,
    /// Cgroup to show instead of the whole cgroup2 mount (local mode).
    #[arg(long, conflicts_with = "source")]
    root: Option<PathBuf>,
    /// Sampling interval [default: 1s locally, 2s for --slurm].
    #[arg(long, value_parser = parse_dur)]
    interval: Option<Duration>,
    /// Sampling rule, the text of an R control line without "R<TAB>"; replaces the defaults.
    #[arg(long = "rule", value_name = "RULE", value_parser = model::Rule::parse)]
    rules: Vec<model::Rule>,
    /// Colour theme (nord, night, mocha, gruvbox, solar, amber, mono) [env: UNJAM_THEME].
    #[arg(long)]
    theme: Option<String>,
    /// Print N text snapshots instead of the TUI, then exit.
    #[arg(long, value_name = "N")]
    batch: Option<u32>,
    /// Replay: start at TIME: start, end, +DUR, -DUR or HH:MM[:SS] (UTC).
    #[arg(long, value_name = "TIME", requires = "replay", allow_hyphen_values = true)]
    at: Option<String>,
    #[command(flatten)]
    srun: SlurmArgs,
}

/// Attach-only flags; each requires --slurm (D43).
#[derive(Args, Debug)]
struct SlurmArgs {
    /// Launch agents on these nodes only.
    #[arg(long, requires = "slurm")]
    nodelist: Option<String>,
    /// Total agent output budget in bytes per second (K, M, G suffixes) [default: 4M].
    #[arg(long, requires = "slurm", value_parser = parse_size)]
    budget: Option<u64>,
    /// Agent binary on the nodes [default: this executable].
    #[arg(long, requires = "slurm")]
    agent: Option<PathBuf>,
    /// Agents exit after this many seconds without control input.
    #[arg(long, requires = "slurm", hide = true)]
    agent_keepalive: Option<u64>,
    /// Extra srun argument (repeatable).
    #[arg(long, requires = "slurm", allow_hyphen_values = true)]
    srun_arg: Vec<String>,
    /// Copy the agent to DIR/unjam.<job>.<uid> on the nodes with srun --bcast (no shared
    /// filesystem needed); DIR must allow executables.
    #[arg(long, value_name = "DIR", requires = "slurm", num_args = 0..=1, require_equals = true,
          default_missing_value = "/tmp")]
    bcast: Option<String>,
    /// Record what the TUI receives, one .arrows file per node in DIR.
    #[arg(long, value_name = "DIR", requires = "slurm")]
    record: Option<PathBuf>,
    /// Print the srun command line and exit.
    #[arg(long, requires = "slurm")]
    print_srun: bool,
}

/// Subcommands besides the viewer: the recorder and the hidden per-node agent.
#[derive(Subcommand, Debug)]
enum Cmd {
    /// Record a job step: run CMD and write its cgroup and process metrics to an .arrows file.
    Record(RecordArgs),
    /// Per-node sampler: control lines on stdin, event lines on stdout (started by srun).
    #[command(hide = true)]
    Agent {
        /// Cgroup to sample instead of the Slurm job or the whole cgroup2 mount.
        #[arg(long)]
        root: Option<PathBuf>,
        /// Exit after this many seconds without control input.
        #[arg(long, default_value_t = 120)]
        keepalive: u64,
        /// Remove the own executable at start (a copy made by srun --bcast).
        #[arg(long)]
        unlink_self: bool,
    },
}

/// `unjam record` flags (§3.5).
#[derive(Args, Debug)]
struct RecordArgs {
    /// Output path; %j job id, %s step id, %n host name, %% a percent sign.
    #[arg(short, long, default_value = "unjam-%j.%s-%n.arrows")]
    output: String,
    /// Sampling interval.
    #[arg(short, long, default_value = "5s", value_parser = parse_dur)]
    interval: Duration,
    /// Sample cgroups only.
    #[arg(long)]
    no_procs: bool,
    /// Cgroup to sample instead of the own step's tasks or the own cgroup.
    #[arg(long)]
    root: Option<PathBuf>,
    /// The command to run.
    #[arg(last = true, required = true, value_name = "CMD")]
    cmd: Vec<String>,
}

/// `500ms`, `2s`, `1m` for clap, without a humantime dependency.
fn parse_dur(s: &str) -> Result<Duration, String> {
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let n: u64 = s[..split].parse().map_err(|_| format!("bad duration: {s:?}"))?;
    match &s[split..] {
        "ms" => Ok(Duration::from_millis(n)),
        "s" | "" => Ok(Duration::from_secs(n)),
        "m" => Ok(Duration::from_secs(n * 60)),
        _ => Err(format!("bad duration: {s:?} (use ms, s or m)")),
    }
}

/// `4M`, `512K`, `1G` or plain bytes, for `--budget`.
fn parse_size(s: &str) -> Result<u64, String> {
    let num = s.strip_suffix(['K', 'M', 'G']).unwrap_or(s);
    let shift = 10 * ["", "K", "M", "G"].iter().position(|&u| u == &s[num.len()..]).unwrap_or(0);
    num.parse::<u64>().map(|n| n << shift).map_err(|_| format!("bad size: {s:?}"))
}

/// The viewer: resolves theme and interval, then runs the chosen source.
fn viewer(cli: Cli) -> Result<(), String> {
    let env = |k| std::env::var(k).ok();
    let no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
    let theme = theme::pick(cli.theme.as_deref(), env("UNJAM_THEME").as_deref(), no_color)?;
    let default = Duration::from_secs(if cli.slurm.is_some() { 2 } else { 1 });
    let interval_ms = model::interval_ms(cli.interval.unwrap_or(default).as_millis());
    let (source, local) = ("unjam".into(), cli.slurm.is_none() && cli.replay.is_empty());
    let opts = tui::Opts { source, interval_ms, rules: cli.rules, theme, local };
    match &cli.slurm {
        _ if !local && cli.slurm.is_none() => {
            record::replay(&cli.replay, cli.at.as_deref(), opts, cli.batch)
        }
        Some(job) => slurm::view(job, &cli.srun, opts, cli.batch),
        None => agent::local(opts, cli.root, cli.batch),
    }
}

/// Entry point: dispatches the command line; errors go to stderr with exit code 1. `record`
/// passes the app's exit code through.
fn main() -> ExitCode {
    let cli = Cli::parse();
    let res = match cli.cmd {
        Some(Cmd::Agent { root, keepalive, unlink_self }) => {
            agent::main(root, keepalive, unlink_self).map(|()| ExitCode::SUCCESS)
        }
        Some(Cmd::Record(a)) => record::run_record(&a),
        None => viewer(cli).map(|()| ExitCode::SUCCESS),
    };
    match res {
        Ok(code) => code,
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "unjam: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
#[allow(unsafe_code, reason = "GlobalAlloc is an unsafe trait; counting needs an implementation")]
mod alloc_count {
    use std::{
        alloc::{GlobalAlloc, Layout, System},
        cell::Cell,
    };

    thread_local! {
        static ALLOCS: Cell<u64> = const { Cell::new(0) };
    }

    struct Counting;

    // SAFETY: every call is forwarded unchanged to `System`, which upholds the `GlobalAlloc`
    // contract. The counter is a const-initialised thread-local `Cell` without destructor, so
    // updating it neither allocates nor re-enters the allocator.
    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let _ = ALLOCS.try_with(|n| n.set(n.get() + 1));
            // SAFETY: the caller's `layout` contract is passed on unchanged.
            unsafe { System.alloc(layout) }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            // SAFETY: `ptr` was returned by `System.alloc` (through `alloc`) with this `layout`.
            unsafe { System.dealloc(ptr, layout) }
        }
    }

    #[global_allocator]
    static GLOBAL: Counting = Counting;

    /// Allocations made so far by the calling thread.
    pub fn allocs() -> u64 {
        ALLOCS.with(Cell::get)
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;
    use clap::error::ErrorKind;

    use super::*;

    fn parse(args: &str) -> Result<Cli, ErrorKind> {
        Cli::try_parse_from(std::iter::once("unjam").chain(args.split(' '))).map_err(|e| e.kind())
    }

    #[test]
    fn command_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn viewer_flags() {
        let cli = parse("--root /sys/fs/cgroup/x --interval 500ms --theme mono --batch 2").unwrap();
        assert_eq!(cli.root, Some(PathBuf::from("/sys/fs/cgroup/x")));
        assert_eq!(cli.interval, Some(Duration::from_millis(500)));
        assert_eq!((cli.theme.as_deref(), cli.batch), (Some("mono"), Some(2)));
        let cli = parse("--rule *\t.\t2000\tc --rule 0-3\t*/user\t-\to");
        assert_eq!(cli.unwrap().rules.len(), 2);
        assert!(parse("--rule *\t.\tsoon\tc").is_err());
        assert!(parse("--interval 2h").is_err());
    }

    #[test]
    fn slurm_takes_an_optional_attached_job_id() {
        assert_eq!(parse("--slurm").unwrap().slurm.as_deref(), Some(""));
        assert_eq!(parse("--slurm=123").unwrap().slurm.as_deref(), Some("123"));
        assert!(parse("--slurm 123").is_err(), "the job id must be attached");
        let cli = parse("--slurm --budget 512K --srun-arg --exclusive --bcast").unwrap();
        assert_eq!(
            (cli.srun.budget, cli.srun.srun_arg, cli.srun.bcast.as_deref()),
            (Some(512 << 10), vec!["--exclusive".into()], Some("/tmp"))
        );
        assert_eq!(
            parse("--slurm --bcast=/var/tmp").unwrap().srun.bcast.as_deref(),
            Some("/var/tmp")
        );
    }

    #[test]
    fn attach_flags_require_slurm_and_sources_conflict() {
        assert_eq!(parse("--budget 4M").err(), Some(ErrorKind::MissingRequiredArgument));
        assert_eq!(parse("--print-srun").err(), Some(ErrorKind::MissingRequiredArgument));
        assert_eq!(parse("--at 10:00").err(), Some(ErrorKind::MissingRequiredArgument));
        assert_eq!(parse("--slurm --replay a.arrows").err(), Some(ErrorKind::ArgumentConflict));
        assert_eq!(parse("--replay a.arrows --root /x").err(), Some(ErrorKind::ArgumentConflict));
        assert_eq!(parse("--replay a.arrows b.arrows --at 10:00").unwrap().replay.len(), 2);
    }

    #[test]
    fn subcommands() {
        let cli = parse("record -i 1s -- srun ./a.out -n 4").unwrap();
        let Some(Cmd::Record(a)) = cli.cmd else { panic!() };
        assert_eq!(
            (a.output.as_str(), a.interval),
            ("unjam-%j.%s-%n.arrows", Duration::from_secs(1))
        );
        assert_eq!(a.cmd, ["srun", "./a.out", "-n", "4"]);
        assert!(parse("record").is_err(), "a command is required");
        let cli = parse("agent --keepalive 5").unwrap();
        assert!(matches!(
            cli.cmd,
            Some(Cmd::Agent { keepalive: 5, root: None, unlink_self: false })
        ));
        assert!(parse("agent --batch 1").is_err());
        assert_eq!(parse_size("4M"), Ok(4 << 20));
        assert!(parse_size("4X").is_err());
        assert_eq!((parse_size("2G"), parse_size("512")), (Ok(2 << 30), Ok(512)));
        assert!(parse_size("4KK").is_err() && parse_size("K").is_err());
    }
}
