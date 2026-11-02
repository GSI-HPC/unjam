#!/usr/bin/env python3
# SPDX-License-Identifier: LGPL-3.0-or-later
# SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany
"""captions.py WORK OUT.vtt CLIP...: English WebVTT captions for the tour.

Motion-graphic clips take their text from TEXT; TUI shots take the captions that capture.py
stored with their frames, at the times they appear.
"""
import json, subprocess, sys

TEXT = {
    "Intro": ["unjam: see inside your Slurm job. Live, on every node."],
    "Problem": ["Your 4-node job is slow. Which node? Which process?"],
    "How": ["unjam --slurm starts one small agent per node in an overlap step.",
            "Each agent reads the cgroup v2 tree and streams it back. No daemon, no root."],
    "Record": ["unjam record writes one Arrow file per node.",
               "Replay it later with unjam --replay, or read it with pyarrow."],
    "Outro": ["Install with mise or dnf. Documentation: gsi-hpc.github.io/unjam"],
}


def dur(path):
    out = subprocess.run(["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", path],
                         check=True, capture_output=True, text=True).stdout
    return float(out)


def ts(t):
    return f"{int(t // 3600):02d}:{int(t % 3600 // 60):02d}:{t % 60:06.3f}"


work, out, clips = sys.argv[1], sys.argv[2], sys.argv[3:]
cues, start = [], 0.0
for c in clips:
    d = dur(f"{work}/clips/{c}.mp4")
    if c in TEXT:
        n = len(TEXT[c])
        cues += [(start + d * i / n, start + d * (i + 1) / n, s) for i, s in enumerate(TEXT[c])]
    else:
        prev = None
        for line in open(f"{work}/cap/{c}.jsonl"):
            f = json.loads(line)
            if f["cap"] != (prev[2] if prev else None):
                if prev and prev[2]:
                    cues.append((prev[0], start + f["t"], prev[2]))
                prev = (start + f["t"], None, f["cap"])
        if prev and prev[2]:
            cues.append((prev[0], start + d, prev[2]))
    start += d
with open(out, "w") as f:
    f.write("WEBVTT\n\n")
    for i, (a, b, s) in enumerate(cues, 1):
        f.write(f"{i}\n{ts(a)} --> {ts(min(b, start))}\n{s.replace('  ·  ', ': ')}\n\n")
