#!/bin/bash
# SPDX-License-Identifier: LGPL-3.0-or-later
# SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany

# Stops every process of the tour scene and removes its cgroups.
U=${BENCH_UNIFIED:-/sys/fs/cgroup/unified}
for d in host node1 node2 node3 node4; do
  [ -d "$U/$d" ] || continue
  find "$U/$d" -name cgroup.procs -exec cat {} + | xargs -r kill 2>/dev/null
  sleep 0.5
  find "$U/$d" -depth -type d -exec rmdir {} + 2>/dev/null
done
true
