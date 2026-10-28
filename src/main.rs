// SPDX-License-Identifier: LGPL-3.0-or-later
// SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany

use clap::Parser;

/// Metric descriptors, the interchange event and the sampling rules.
#[allow(dead_code, reason = "no command uses the model yet")]
mod model;

/// unjam - Utility for Node/Job Analysis and Monitoring
#[derive(Parser)]
#[command(version, about)]
struct Cli {}

/// Entry point: parses the command line; the modes arrive with the code that implements them.
fn main() {
    let _cli = Cli::parse();
}
