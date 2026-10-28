// SPDX-License-Identifier: LGPL-3.0-or-later
// SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany

use std::{io::Write as _, path::PathBuf, process::ExitCode};

use clap::{Parser, Subcommand};

/// The sampler, the shared run loop and the output path.
mod agent;
/// The cgroup v2 source, discovery and the Slurm job filter.
mod cgroup;
/// Metric descriptors, the interchange event and the sampling rules.
mod model;
/// The /proc source and the constants read from /proc.
mod procfs;
/// The metric-source plug-in surface and the fd cache.
mod source;
/// The agent ↔ TUI text codecs.
mod wire;

/// unjam - Utility for Node/Job Analysis and Monitoring
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

/// Subcommands besides the viewer: the hidden per-node agent.
#[derive(Subcommand)]
enum Cmd {
    /// Per-node sampler: control lines on stdin, event lines on stdout (started by srun).
    #[command(hide = true)]
    Agent {
        /// Cgroup to sample instead of the Slurm job or the whole cgroup2 mount.
        #[arg(long)]
        root: Option<PathBuf>,
        /// Exit after this many seconds without control input.
        #[arg(long, default_value_t = 120)]
        keepalive: u64,
    },
}

/// Entry point: dispatches the command line; errors go to stderr with exit code 1.
fn main() -> ExitCode {
    let res = match Cli::parse().cmd {
        Some(Cmd::Agent { root, keepalive }) => agent::main(root, keepalive),
        None => Ok(()),
    };
    match res {
        Ok(()) => ExitCode::SUCCESS,
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
