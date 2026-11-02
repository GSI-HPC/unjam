#!/usr/bin/env python3
# SPDX-License-Identifier: LGPL-3.0-or-later
# SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany
"""Record a terminal session for the docs' terminal player (src/data/term/*.json).

    capture.py OUT.json COLSxROWS -- CMD [ARGS...] < steps.txt

CMD runs in a private tmux server. Each step line makes frames:
    WAIT n caption           one frame per second for n seconds
    KEY [@secs] k1 k2 | cap  send tmux keys (Down, Enter, Escape, '?', /train), then one frame
                             that shows for secs (default 1.6)
    SLEEP secs               wait without a frame
Lines starting with # are comments.
"""
import json, os, shlex, subprocess, sys, time

out, size, sep, *cmd = sys.argv[1:]
assert sep == "--" and cmd, __doc__
cols, rows = size.split("x")
tmux = ["tmux", "-L", f"unjam-capture-{os.getpid()}", "-f", "/dev/null"]
shell = f"{shlex.join(cmd)}; sleep 86400"
subprocess.run(tmux + ["new-session", "-d", "-s", "main", "-x", cols, "-y", rows,
                       "-e", "TERM=xterm-256color", "-e", "COLORTERM=truecolor", shell], check=True)
subprocess.run(tmux + ["set", "-g", "status", "off"], check=True)
subprocess.run(tmux + ["set", "-ga", "terminal-overrides", ",*:Tc"], check=True)
frames = []


def grab(caption, hold=1.0, keys=""):
    pane = tmux + ["capture-pane", "-e", "-N", "-p", "-t", "main"]
    a = subprocess.run(pane, capture_output=True, text=True, check=True).stdout.rstrip("\n")
    a = "\n".join(line.rstrip(" ") for line in a.split("\n"))
    frames.append({"a": a, "c": caption, "k": keys, "d": hold})


try:
    for line in sys.stdin:
        line = line.rstrip("\n")
        if not line or line.startswith("#"):
            continue
        op, _, rest = line.partition(" ")
        if op == "SLEEP":
            time.sleep(float(rest))
        elif op == "WAIT":
            secs, _, cap = rest.partition(" ")
            for _ in range(int(secs)):
                time.sleep(1)
                grab(cap)
        elif op == "KEY":
            keys, _, cap = rest.partition("|")
            ks, hold = keys.split(), 1.6
            if ks and ks[0].startswith("@"):
                hold, ks = float(ks[0][1:]), ks[1:]
            for k in ks:
                subprocess.run(tmux + ["send-keys", "-t", "main", k], check=True)
                time.sleep(0.12)
            time.sleep(0.5)
            grab(cap.strip(), hold, " ".join(ks))
        else:
            sys.exit(f"unknown step: {line}")
finally:
    subprocess.run(tmux + ["kill-server"])

with open(out, "w") as f:
    json.dump({"size": size, "frames": frames}, f, ensure_ascii=False)
print(len(frames), "frames", file=sys.stderr)
