// SPDX-License-Identifier: LGPL-3.0-or-later
// SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany

use clap::Parser;

/// The cgroup v2 source, discovery and the Slurm job filter.
#[allow(dead_code, reason = "no command samples yet")]
mod cgroup;
/// Metric descriptors, the interchange event and the sampling rules.
#[allow(dead_code, reason = "no command uses the protocol yet")]
mod model;
/// The /proc source and the constants read from /proc.
#[allow(dead_code, reason = "no command samples yet")]
mod procfs;
/// The metric-source plug-in surface and the fd cache.
#[allow(dead_code, reason = "no command samples yet")]
mod source;
/// The agent ↔ TUI text codecs.
#[allow(dead_code, reason = "no command uses the protocol yet")]
mod wire;

/// unjam - Utility for Node/Job Analysis and Monitoring
#[derive(Parser)]
#[command(version, about)]
struct Cli {}

/// Entry point: parses the command line; the modes arrive with the code that implements them.
fn main() {
    let _cli = Cli::parse();
}
