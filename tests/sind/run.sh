#!/usr/bin/env bash
# SPDX-License-Identifier: LGPL-3.0-or-later
# SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany
#
# Integration tests on the sind cluster "unjam" (tests/sind/cluster.sh). Needs a running
# cluster (`mise run it:up`) and the static binary (`mise run build:musl`). The workspace is
# mounted at /data on every node.
#   EXTRA  non-empty when the cluster enables io and pids in job cgroups
set -euo pipefail

CLUSTER=unjam
BIN=/data/target/$(uname -m)-unknown-linux-musl/release/unjam
failures=0

# Runs a command on the submitter as alice.
as_alice() { sind exec "$CLUSTER" --user alice -- "$@"; }

ok() { echo "ok - $1"; }
not_ok() { echo "not ok - $1"; failures=$((failures + 1)); }

# Smoke 1: Slurm answers and shows the three workers.
if out=$(sind exec "$CLUSTER" -- sinfo --noheader --format='%D %T'); then
	echo "$out"
	ok "sinfo"
else
	not_ok "sinfo"
fi

# Smoke 2: a three-node step runs as a cluster user, one task per worker.
if out=$(as_alice srun -N3 --ntasks-per-node=1 hostname) &&
	[ "$(sort -u <<<"$out" | wc -l)" -eq 3 ]; then
	ok "srun -N3 hostname as alice"
else
	not_ok "srun -N3 hostname as alice: ${out:-no output}"
fi

# Smoke 3: the job cgroup's subtree_control. Found by structure, as cgroup::job_root does: strip
# /step_* from the task's own cgroup, the 0:: line (26.05 job directories are SLUIDs, not
# job_<id>; hosts with leftover v1 hierarchies list more lines).
# shellcheck disable=SC2016 # expanded by the shell on the node
probe='p=$(sed -n "s/^0:://p" /proc/self/cgroup); cat "/sys/fs/cgroup${p%/step_*}/cgroup.subtree_control"'
if ctl=$(as_alice srun -N1 sh -c "$probe"); then
	echo "job cgroup.subtree_control: $ctl"
	if [ -z "${EXTRA:-}" ]; then
		ok "job subtree_control readable (io and pids not required)"
	elif grep -qw io <<<"$ctl" && grep -qw pids <<<"$ctl"; then
		ok "job subtree_control has io and pids"
	else
		not_ok "job subtree_control lacks io or pids with EnableExtraControllers"
	fi
else
	not_ok "read job subtree_control"
fi

# Smoke 4: the static binary from the workspace runs on a worker.
if out=$(sind ssh "alice@worker-0.$CLUSTER" -- "$BIN" --version); then
	ok "unjam runs on a worker: $out"
else
	not_ok "unjam --version on worker-0 ($BIN)"
fi

echo "failures: $failures"
[ "$failures" -eq 0 ]
