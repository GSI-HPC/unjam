#!/bin/bash
# SPDX-License-Identifier: LGPL-3.0-or-later
# SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany

# Builds the tour video for the docs site: site/public/media/tour.{mp4,webm,vtt} and
# tour-poster.jpg. See README.md.
#   build.sh            render from the captures in $W/cap (capture them first if missing)
#   build.sh capture    capture the TUI shots again (needs root and the bench)
set -euo pipefail
H=$(cd "$(dirname "$0")" && pwd)
O=${TOUR_OUT:-$H/../../site/public/media}
W=${TOUR_WORK:-${TMPDIR:-/tmp}/unjam-tour}
MANIM=${MANIM:-manim}
BG=0x2E3440
mkdir -p "$W/clips" "$O"

if [ "${1:-}" = capture ] || [ ! -f "$W/cap/slurm.jsonl" ]; then
  "$H/bench/teardown.sh"
  "$H/bench/setup.sh"
  PATH="$H/bench/bin:$PATH" python3 "$H/capture.py" "$W/cap"
  "$H/bench/teardown.sh"
fi

# Motion graphics.
(cd "$H" && "$MANIM" render -qh --fps 30 --media_dir "$W/manim" scenes.py Intro Problem How Record Outro)
for s in Intro Problem How Record Outro; do cp "$W/manim/videos/scenes/1080p30/$s.mp4" "$W/clips/$s.mp4"; done

# TUI shots: PNG frames held for their capture time, faded in and out of the background.
shot() { # name window-title
  node "$H/render.mjs" "$W/cap/$1.jsonl" "$W/frames/$1" "$2"
  local d
  d=$(awk '/^duration/{s+=$2} END{print s}' "$W/frames/$1/frames.txt")
  ffmpeg -v error -y -f concat -safe 0 -i "$W/frames/$1/frames.txt" \
    -vf "fps=30,format=yuv420p,fade=in:d=0.4:color=$BG,fade=out:st=$(echo "$d - 0.4" | bc):d=0.4:color=$BG" \
    -c:v libx264 -crf 16 -preset fast "$W/clips/$1.mp4"
}
shot slurm '$ unjam --slurm=4242'
shot host '$ unjam'
shot replay '$ unjam --replay rec/*.arrows'

order=(Intro Problem How slurm host Record replay Outro)
printf "file '%s'\n" "${order[@]/%/.mp4}" > "$W/clips/order.txt"
ffmpeg -v error -y -f concat -safe 0 -i "$W/clips/order.txt" -c copy "$W/tour-master.mp4"
ffmpeg -v error -y -i "$W/tour-master.mp4" -c:v libx264 -preset slow -crf 20 -pix_fmt yuv420p \
  -movflags +faststart -an "$O/tour.mp4"
ffmpeg -v error -y -i "$W/tour-master.mp4" -c:v libvpx-vp9 -b:v 0 -crf 34 -row-mt 1 -deadline good \
  -pix_fmt yuv420p -an "$O/tour.webm"
# Poster: the straggler highlight of the slurm shot.
start=$(for c in Intro Problem How; do ffprobe -v error -show_entries format=duration -of csv=p=0 "$W/clips/$c.mp4"; done | paste -sd+ | bc)
ffmpeg -v error -y -ss "$(echo "$start + 6" | bc)" -i "$W/tour-master.mp4" -frames:v 1 -q:v 3 "$O/tour-poster.jpg"
python3 "$H/captions.py" "$W" "$O/tour.vtt" "${order[@]}"
ls -l "$O"/tour*
