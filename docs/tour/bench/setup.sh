#!/bin/bash
# SPDX-License-Identifier: LGPL-3.0-or-later
# SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany

# Builds the tour scene in a cgroup2 mount (default /sys/fs/cgroup/unified, as on a hybrid
# host; BENCH_UNIFIED names another): a systemd-like host tree under /host and a 4-node Slurm
# job 4242 under /node1../node4, which bin/srun and bin/hostview present as their own roots.
# Needs root. bench/teardown.sh stops it again.
set -eu
U=${BENCH_UNIFIED:-/sys/fs/cgroup/unified}
H=$(cd "$(dirname "$0")" && pwd)
W=${TMPDIR:-/tmp}/unjam-tour-work
cc -O2 -pthread -o "$W" "$H/work.c"
place() { # cgroup WORK argv0 args...
  local cg=$1 work=$2; shift 2; mkdir -p "$U/$cg"
  ( echo $BASHPID > "$U/$cg/cgroup.procs"; cd /tmp; n=$1; shift; WORK=$work exec -a "$n" "$W" "$@" ) >/dev/null 2>&1 &
}
s=host/system.slice u=host/user.slice/user-1000.slice
place $s/sshd.service         "0 2 1 0"      sshd: /usr/sbin/sshd -D
place $s/munge.service        "0 3 1 0"      munged
place $s/nginx.service        "0.05 30 2 0"  "nginx: worker process"
place $s/postgresql.service   "0.1 200 4 64" postgres -D /var/lib/pgsql/data
place $u/session-3.scope      "0 10 1 0"     vim src/solver.cpp
place $u/user@1000.service/app.slice/build.scope "0.4 400 1 0" cc1plus -O2 src/solver.cpp
place $u/user@1000.service/app.slice/build.scope "0.3 350 1 0" cc1plus -O2 src/mesh.cpp
mkdir -p $U/host/init.scope
# Job 4242: step_0 runs 2 ranks per node; node3 is the straggler, node2 writes 512 KiB/s.
for n in 1 2 3 4; do
  J=node$n/system.slice/slurmstepd.scope/job_4242
  mkdir -p $U/node$n/system.slice/slurmstepd.scope/system
  place $J/step_extern/user/task_special "0 1 1 0" sleep infinity
  [ $n = 1 ] && place $J/step_batch/user/task_0 "0 4 1 0" /bin/bash slurm_script
  cpu=0.3; [ $n = 3 ] && cpu=0.04
  for t in 0 1; do
    place $J/step_0/user/task_$t "$cpu $((300 + 100*n)) 2 $((n == 2 ? 512 : 0))" \
      python3 train.py --epochs 90 --lr 3e-4 --data /scratch/imagenet
  done
done
echo ok
