#!/bin/sh
# SPDX-License-Identifier: LGPL-3.0-or-later
# SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany

# Makes the site's fonts in src/fonts/ from the upstream variable fonts, cut to the glyphs the
# site needs. Needs fontTools with brotli (pip install fonttools brotli).
#   Inter 4.1:            https://github.com/rsms/inter/releases (InterVariable*.ttf)
#   JetBrains Mono 2.304: https://github.com/JetBrains/JetBrainsMono/releases (fonts/variable)
# Usage: scripts/fonts.sh DIR   (DIR holds the three .ttf files)
set -eu
src=${1:?usage: scripts/fonts.sh DIR}
out=$(dirname "$0")/../src/fonts

# Latin-1 and Latin Extended-A, punctuation, and the symbols the pages use.
text='U+0020-007E,U+00A0-017F,U+2000-206F,U+20AC,U+2122,U+2190-2199,U+2212,U+2215,U+2248,U+2260,U+2264-2265,U+0394,U+03BC,U+03C0,U+03C6,U+25B2,U+25B6,U+25BC,U+25CF,U+2713'
# The mono font adds the arrows, box-drawing, block and braille cells of the terminal screens.
cells='U+2190-21FF,U+2300-23FF,U+2400-243F,U+2500-25FF,U+2800-28FF'

# Inter keeps its optical-size axis: headings get the display cut, body text the text cut.
for f in InterVariable:inter InterVariable-Italic:inter-italic; do
  pyftsubset "$src/${f%%:*}.ttf" --unicodes="$text" --flavor=woff2 \
    --layout-features='kern,liga,calt,ccmp,locl,mark,mkmk,tnum,cv11,case,frac,sups,subs' \
    --output-file="$out/${f#*:}.woff2"
done
# No code ligatures: an option such as --overlap shows both hyphens.
pyftsubset "$src/JetBrainsMono[wght].ttf" --unicodes="$text,$cells" --flavor=woff2 \
  --layout-features='ccmp,locl,mark,mkmk' --output-file="$out/jetbrains-mono.woff2"
