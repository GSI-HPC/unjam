#!/usr/bin/env python3
# SPDX-License-Identifier: LGPL-3.0-or-later
# SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany
"""capture.py OUT: records the real unjam TUI for the tour, one OUT/<shot>.jsonl per shot.

Each shot runs unjam in a private tmux server, waits until the CPU history is full, then
plays its script (keys, caption, highlight) and samples the pane 15 times a second. A line
is {"t": seconds, "ansi": pane with SGR codes, "cap": caption, "hl": regex of rows to mark};
render.mjs turns the lines into video frames. Needs the bench (bench/setup.sh) and unjam on
PATH; bench/bin must come first in PATH.
"""
import json, os, subprocess, sys, threading, time

SIZE = (120, 30)
REC = "/tmp/unjam-tour-rec"

# shot: (command, warm-up seconds, [(time, action, value)]); actions: cap, hl, keys.
SHOTS = {
    "slurm": (["unjam", "--slurm=4242"], 34, [
        (0.0, "cap", "unjam --slurm  ·  all nodes of job 4242 in one tree"),
        (4.0, "hl", r"▾ node3 .* 1\d\.\d%"),
        (4.0, "cap", "node3 lags: 14 % CPU, the other nodes 90 %"),
        (8.5, "hl", None),
        (8.5, "keys", ["v"]),
        (8.5, "cap", "v  ·  step, node or flat layout"),
        (10.5, "keys", ["v"]),
        (10.5, "cap", "flat: every process, sorted by CPU"),
        (11.5, "hl", r"train\.py.*node3"),
        (15.0, "hl", None),
        (15.0, "keys", ["v", "/", "n", "o", "d", "e", "3", "Enter"]),
        (15.0, "cap", "/  ·  filter by name or path"),
        (19.0, "end", None),
    ]),
    "host": (["hostview", "unjam"], 18, [
        (0.0, "cap", "unjam  ·  any cgroup v2 host: slices, services, sessions"),
        (4.0, "keys", ["Down"] * 6),
        (5.0, "keys", ["Enter"]),
        (5.0, "cap", "Enter  ·  every metric of one process"),
        (9.0, "end", None),
    ]),
    "replay": (["sh", "-c", f"unjam --replay {REC}/*.arrows --at start"], 2, [
        (0.0, "cap", "unjam --replay  ·  scrub through a recorded job"),
        (0.5, "keys", ["Space"]),
        (7.5, "end", None),
    ]),
}


def tmux(name, *args, out=False):
    cmd = ["tmux", "-L", f"tour-{name}", "-f", "/dev/null", *args]
    return subprocess.run(cmd, check=True, capture_output=out, text=True).stdout if out else subprocess.run(cmd, check=True)


def start(name, argv):
    c, r = SIZE
    tmux(name, "new-session", "-d", "-s", "main", "-x", str(c), "-y", str(r), "-e", "TERM=xterm-256color",
         "-e", "COLORTERM=truecolor", "-e", f"PATH={os.environ['PATH']}", subprocess.list2cmdline(argv) + "; sleep 86400")
    tmux(name, "set", "-g", "status", "off")
    tmux(name, "set", "-ga", "terminal-overrides", ",*:Tc")


def record_job():
    """Records 40 s of job 4242 with the TUI's --record, the input of the replay shot."""
    subprocess.run(["rm", "-rf", REC])
    start("rec", ["unjam", "--slurm=4242", "--record", REC])
    time.sleep(40)
    tmux("rec", "send-keys", "-t", "main", "q")
    time.sleep(2)
    tmux("rec", "kill-server")


def shoot(name, out):
    argv, warm, script = SHOTS[name]
    start(name, argv)
    time.sleep(warm)
    state = {"cap": "", "hl": None, "done": False}
    lines, t0 = [], time.monotonic()

    def sample():
        while not state["done"]:
            ansi = tmux(name, "capture-pane", "-e", "-N", "-p", "-t", "main", out=True)
            lines.append({"t": round(time.monotonic() - t0, 3), "ansi": ansi, "cap": state["cap"], "hl": state["hl"]})
            time.sleep(1 / 15)

    th = threading.Thread(target=sample)
    th.start()
    for at, action, value in script:
        time.sleep(max(0.0, at - (time.monotonic() - t0)))
        if action == "keys":
            for k in value:
                tmux(name, "send-keys", "-t", "main", k)
                time.sleep(0.12)
        elif action == "end":
            state["done"] = True
        else:
            state[action] = value
    th.join()
    tmux(name, "kill-server")
    with open(os.path.join(out, f"{name}.jsonl"), "w") as f:
        for line in lines:
            f.write(json.dumps(line) + "\n")


if __name__ == "__main__":
    out = sys.argv[1]
    os.makedirs(out, exist_ok=True)
    names = sys.argv[2:] or list(SHOTS)
    if "replay" in names:
        record_job()
    for n in names:
        print(f"capture: {n}", file=sys.stderr)
        shoot(n, out)
