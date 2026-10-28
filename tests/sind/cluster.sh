#!/bin/sh
# SPDX-License-Identifier: LGPL-3.0-or-later
# SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany
#
# Prints the sind cluster config of the integration tests (schema: sind docs
# configuration/cluster-config.md). slurm.main goes into slurm.conf, slurm.cgroup into
# cgroup.conf.
#   SLURM       Slurm release line of the node image: 25.11 or 26.05 (default)
#   EXTRA       non-empty: EnableExtraControllers=io,pids (IO and PIDS in job cgroups)
#   JOBIDPATHS  non-empty: CgroupJobIdPaths=yes (26.05 only; job_<id> instead of SLUID dirs)
set -eu

SLURM=${SLURM:-26.05}
EXTRA=${EXTRA:-}
JOBIDPATHS=${JOBIDPATHS:-}
# Fixed per sind release: the image of that release line at the pinned sind version.
SIND_VERSION=v0.15.0

case "$SLURM" in
25.11 | 26.05) ;;
*) echo "error: SLURM must be 25.11 or 26.05, not '$SLURM'" >&2; exit 2 ;;
esac
if [ -n "$JOBIDPATHS" ] && [ "$SLURM" != 26.05 ]; then
	echo "error: CgroupJobIdPaths needs Slurm 26.05" >&2; exit 2
fi

# WaitTime=10: agents must survive another agent's exit past the site wait time (IT 4).
# Two CPUs and 1 GiB per worker: DefMemPerCPU 512 MiB, two 1-CPU jobs per worker (IT 2 decoys).
cat <<EOF
kind: Cluster
name: unjam
defaults:
  image: ghcr.io/gsi-hpc/sind-node:${SIND_VERSION}-${SLURM}
  cpus: 2
  memory: 1g
users:
  - alice
  - bob
nodes:
  - role: controller
  - role: submitter
  - role: worker
    count: 3
slurm:
  main: |
    WaitTime=10
  cgroup: |
    ConstrainRAMSpace=yes
    ${EXTRA:+EnableExtraControllers=io,pids}
    ${JOBIDPATHS:+CgroupJobIdPaths=yes}
EOF
