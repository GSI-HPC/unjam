#!/usr/bin/env bash
# SPDX-License-Identifier: LGPL-3.0-or-later
# SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany
#
# Print the CHANGELOG.md section of one version (the release notes); fail if it is missing or empty.
# Usage: scripts/changelog-section.sh 0.2.0
set -euo pipefail

version=${1:?usage: changelog-section.sh VERSION}
notes=$(awk -v v="$version" '
  /^## / { on = (index($0, "## [" v "]") == 1); next }
  on
' "$(dirname "$0")/../CHANGELOG.md")

if ! grep -q '[^[:space:]]' <<<"$notes"; then
  echo "error: CHANGELOG.md has no section for $version" >&2
  exit 1
fi
printf '%s\n' "$notes"
