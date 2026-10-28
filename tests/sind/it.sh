#!/bin/sh
# SPDX-License-Identifier: LGPL-3.0-or-later
# SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany
#
# Local integration run: create the sind cluster, run the tests, and always delete the cluster.
# SLURM, EXTRA and JOBIDPATHS select the configuration (tests/sind/cluster.sh).
set -eu

trap 'mise run it:down' EXIT
mise run it:up
mise run it:test
