#!/usr/bin/env bash
# SPDX-License-Identifier: LGPL-3.0-or-later
# SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany
#
# Integration tests on the sind cluster "unjam" (tests/sind/cluster.sh). Needs a running
# cluster (`mise run it:up`) and the static binary (`mise run build:musl`). The workspace is
# mounted at /data on every node. Captures go to target/sind-*.txt (CI uploads them on failure).
#   SLURM       Slurm line of the cluster: 25.11 or 26.05 (default)
#   EXTRA       non-empty when the cluster enables io and pids in job cgroups
#   JOBIDPATHS  non-empty when the cluster sets CgroupJobIdPaths=yes
set -euo pipefail

CLUSTER=unjam
BIN=/data/target/$(uname -m)-unknown-linux-musl/release/unjam
OUT=target
SLURM=${SLURM:-26.05}
failures=0
mkdir -p "$OUT"

# Runs a command on the submitter as alice or bob.
as_alice() { sind exec "$CLUSTER" --user alice -- "$@"; }
as_bob() { sind exec "$CLUSTER" --user bob -- "$@"; }
# Runs a command on worker N as alice.
on_worker() { local n=$1; shift; sind ssh "alice@worker-$n.$CLUSTER" -- "$@"; }

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

# The last --batch snapshot of a capture: from its last header line to the footer.
last() { awk '/^ unjam /{s=""} {s=s $0 "\n"} END{printf "%s", s}' "$1"; }

# IT 1: local mode on a worker, on real procfs and kernfs. The host tree must show system.slice
# and process rows, and the fd-cache self-check must pass (no "stale data" warning).
if on_worker 0 "$BIN" --batch 2 >"$OUT/sind-it1.txt" 2>&1; then
	snap=$(last "$OUT/sind-it1.txt")
	echo "$snap"
	if grep -q 'system\.slice' <<<"$snap"; then ok "IT 1: system.slice"; else not_ok "IT 1: system.slice"; fi
	if grep -qE '[└├] .* [0-9]+ +[0-9.]+%' <<<"$snap"; then
		ok "IT 1: process rows"
	else
		not_ok "IT 1: process rows"
	fi
	if grep -q 'stale data' "$OUT/sind-it1.txt"; then
		not_ok "IT 1: fd-cache self-check failed"
	else
		ok "IT 1: fd-cache self-check passed"
	fi
else
	not_ok "IT 1: unjam --batch 2 on worker-0"
	cat "$OUT/sind-it1.txt"
fi

# Fails unless every given job is RUNNING within 60 s (decoys must really share the nodes).
wait_running() {
	local deadline=$((SECONDS + 60)) jobs
	jobs=$(IFS=,; echo "$*")
	until [ "$(as_alice squeue -h -j "$jobs" -o %T | grep -c '^RUNNING$')" -eq $# ]; do
		[ "$SECONDS" -lt "$deadline" ] || return 1
		sleep 2
	done
}

# Whether the step of job $1 named unjam (the agents) is listed.
agents_up() { as_alice squeue -h -s -j "$1" -o %j | grep -qx unjam; }

# The viewer on job $1 with further flags; the capture goes to $OUT/sind-<name>.txt.
attach() {
	local name=$1 job=$2
	shift 2
	as_alice "$BIN" --slurm="$job" "$@" >"$OUT/sind-$name.txt" 2>&1
}

# Target job: one sleep per worker. Decoys on the same workers: one of alice, one of bob.
J=$(as_alice sbatch --parsable -N3 --ntasks-per-node=1 --cpus-per-task=1 --mem=256M \
	--wrap 'srun sleep 600')
A=$(as_alice sbatch --parsable -N1 -w worker-0 -n1 --mem=256M --wrap 'srun sleep 601')
B=$(as_bob sbatch --parsable -N1 -w worker-1 -n1 --mem=256M --wrap 'srun sleep 602')
trap 'as_alice scancel -u alice || true; as_bob scancel -u bob || true' EXIT
if wait_running "$J" "$A" "$B"; then
	ok "jobs $J $A $B running"
else
	not_ok "jobs $J $A $B not all running"
	as_alice squeue
fi

# IT 2: attach isolation. Agents start in a new step of J; they must see J only, without their
# own step and without the decoys that share worker-0 and worker-1.
if attach it2 "$J" --batch 3; then
	snap=$(last "$OUT/sind-it2.txt")
	echo "$snap"
	if grep -q '^nodes=3 ' <<<"$snap"; then ok "IT 2: 3 nodes"; else not_ok "IT 2: 3 nodes"; fi
	for n in 0 1 2; do
		if grep -qE "task_[0-9]+ +worker-$n " <<<"$snap" &&
			grep -qE "sleep 600 +worker-$n +[0-9]+ " <<<"$snap"; then
			ok "IT 2: task cgroup with sleep 600 on worker-$n"
		else
			not_ok "IT 2: task cgroup with sleep 600 on worker-$n"
		fi
	done
	if grep -qE 'sleep 60[12]' "$OUT/sind-it2.txt"; then
		not_ok "IT 2: a decoy job is visible"
	else
		ok "IT 2: decoys of alice and bob invisible"
	fi
	if grep -qE '[^/]step_[1-9]' "$OUT/sind-it2.txt"; then
		not_ok "IT 2: the agents' own step is visible"
	else
		ok "IT 2: own step excluded"
	fi
	# PIDS, then IO-R/s, of the step_0 row, counted from the end (INT and W are last).
	read -r pids ior < <(awk '$1 == "step_0" || $2 == "step_0" {print $(NF-2), $(NF-4)}' <<<"$snap") ||
		true
	if [ -n "${EXTRA:-}" ]; then
		if grep -q 'IO-R/s' <<<"$snap" && [ "$ior" != - ] && [ "$pids" != - ]; then
			ok "IT 2: io and pids values on step_0 (PIDS $pids, IO-R/s $ior)"
		else
			not_ok "IT 2: io and pids values on step_0 (PIDS ${pids:-?}, IO-R/s ${ior:-?})"
		fi
	# Without the controllers, IO-R/s shows for processes (/proc/<pid>/io) but not for cgroups.
	elif { ! grep -q 'IO-R/s' <<<"$snap" || [ "$ior" = - ]; } && [ "${pids#\~}" != "$pids" ]; then
		ok "IT 2: no io and pids controllers: step_0 IO-R/s ${ior:-hidden}, PIDS from threads $pids"
	else
		not_ok "IT 2: without io and pids: step_0 IO-R/s - and PIDS marked ~ (${ior:-?}, ${pids:-?})"
	fi
else
	not_ok "IT 2: unjam --slurm=$J --batch 3"
	cat "$OUT/sind-it2.txt"
fi
as_alice scancel "$A" || true
as_bob scancel "$B" || true

# IT 2: the job directory under which the agents found J, by Slurm line and naming scheme.
scope=/sys/fs/cgroup/system.slice/slurmstepd.scope
dirs=$(on_worker 0 ls "$scope")
if [ "$SLURM" = 25.11 ] || [ -n "${JOBIDPATHS:-}" ]; then
	want="job_$J"
	grep -qx "$want" <<<"$dirs" && found=yes || found=no
else
	want="SLUID names, no job_*"
	grep -q '^job_' <<<"$dirs" && found=no || found=yes
fi
if [ $found = yes ]; then
	ok "IT 2: job directory: $want"
else
	not_ok "IT 2: job directory: $want; found: $(xargs <<<"$dirs")"
fi

# IT 3: selective rules. Node 0 samples processes of step_0 at 500 ms, nodes 1 and 2 cgroups only.
if attach it3 "$J" --batch 3 --rule $'0\tstep_0\t500\tp' --rule $'1-2\t.\t2000\tc'; then
	snap=$(last "$OUT/sind-it3.txt")
	if grep -qE "sleep 600 +worker-0 +[0-9]+ " <<<"$snap" &&
		grep -qE 'worker-0 .* 500ms p$' <<<"$snap"; then
		ok "IT 3: processes at 500 ms on node 0"
	else
		not_ok "IT 3: processes at 500 ms on node 0"
	fi
	if ! grep -qE 'worker-[12] +[0-9]+ ' <<<"$snap" &&
		grep -qE 'worker-1 .* 2s c$' <<<"$snap" && grep -qE 'worker-2 .* 2s c$' <<<"$snap"; then
		ok "IT 3: cgroups only at 2 s on nodes 1 and 2"
	else
		not_ok "IT 3: cgroups only at 2 s on nodes 1 and 2"
	fi
else
	not_ok "IT 3: unjam with --rule"
	cat "$OUT/sind-it3.txt"
fi

# IT 4a: the agents' step is gone within 5 s of the viewer's exit; the job keeps running.
gone=no
for _ in 1 2 3 4 5; do
	agents_up "$J" || { gone=yes; break; }
	sleep 1
done
if [ $gone = yes ] && [ "$(as_alice squeue -h -j "$J" -o %T)" = RUNNING ]; then
	ok "IT 4a: agent step gone within 5 s, job still running"
else
	not_ok "IT 4a: agent step gone within 5 s ($gone), job still running"
fi

# IT 4b: one agent killed; the others keep ticking past WaitTime=10 (--kill-on-bad-exit=0 and
# --wait=0 on a --jobid step). 3 nodes, exactly one stale, at the end of a 25 s run.
attach it4b "$J" --interval 1s --batch 25 &
viewer=$!
sleep 4
on_worker 2 pkill -u alice -f "'unjam agent'" || true
if wait "$viewer" && grep -q '^nodes=3 stale=1 ' <(last "$OUT/sind-it4b.txt"); then
	ok "IT 4b: other agents survive a killed agent past WaitTime"
else
	not_ok "IT 4b: other agents survive a killed agent past WaitTime"
	tail -5 "$OUT/sind-it4b.txt"
fi

# IT 4c: a SIGKILLed viewer leaves no agents behind (stdin EOF, keepalive as the backstop).
attach it4c "$J" --agent-keepalive 5 --batch 1000 &
viewer=$!
sleep 4
agents_up "$J" && up=yes || up=no
as_alice pkill -KILL -u alice -f 'agent-keepalive 5' || true
wait "$viewer" || true
gone=no
for _ in $(seq 15); do
	agents_up "$J" || { gone=yes; break; }
	sleep 1
done
if [ $up = yes ] && [ $gone = yes ]; then
	ok "IT 4c: agent step gone within 15 s of SIGKILL"
else
	not_ok "IT 4c: agent step up ($up), gone within 15 s of SIGKILL ($gone)"
fi

# IT 8: no shared filesystem needed. The viewer runs from a submitter-local copy that no worker
# has; --bcast copies it, and each agent removes its copy. sind mounts /tmp noexec, hence
# /var/tmp.
as_alice cp "$BIN" /var/tmp/unjam-local
if as_alice /var/tmp/unjam-local --slurm="$J" --bcast=/var/tmp --batch 2 >"$OUT/sind-it8.txt" 2>&1 &&
	grep -q '^nodes=3 ' <(last "$OUT/sind-it8.txt"); then
	ok "IT 8: 3 nodes through --bcast"
else
	not_ok "IT 8: 3 nodes through --bcast"
	cat "$OUT/sind-it8.txt"
fi
for n in 0 1 2; do
	if on_worker "$n" ls /var/tmp | grep -q "^unjam\.$J\."; then
		not_ok "IT 8: broadcast copy left on worker-$n"
	else
		ok "IT 8: no broadcast copy left on worker-$n"
	fi
done

# IT 5: framing stress, 3 nodes × 500 processes at 100 ms. No decode errors; agent RSS < 16 MiB.
F=$(as_alice sbatch --parsable -N3 --ntasks-per-node=1 --cpus-per-task=1 --mem=256M \
	--wrap 'srun sh -c "for i in \$(seq 500); do sleep 900 & done; wait"')
wait_running "$F" || not_ok "IT 5: job $F not running"
sleep 5
if attach it5 "$F" --rule $'*\t.\t100\tp' --budget 64M --batch 30; then
	snap=$(last "$OUT/sind-it5.txt")
	head -2 <<<"$snap"
	procs=$(sed -n '1s/.* \([0-9]*\) procs .*/\1/p' <<<"$snap")
	rss=$(sed -n '2s/.*agents max [^ ]* \([^ ]*\) .*/\1/p' <<<"$snap")
	small=$(awk -v r="$rss" 'BEGIN {
		n = r + 0; u = substr(r, length(r)); m = (u == "M") ? n : (u == "K" || u ~ /[0-9]/) ? 0 : 99
		print (m < 16) ? "yes" : "no" }')
	if grep -q ' errors=0 ' <<<"$snap" && [ "${procs:-0}" -ge 1500 ] && [ "$small" = yes ]; then
		ok "IT 5: errors=0 with $procs processes, agent RSS $rss"
	else
		not_ok "IT 5: errors=0, ≥ 1500 processes (${procs:-?}), agent RSS < 16M (${rss:-?})"
	fi
else
	not_ok "IT 5: unjam --slurm=$F stress run"
	tail -5 "$OUT/sind-it5.txt"
fi

echo "failures: $failures"
[ "$failures" -eq 0 ]
