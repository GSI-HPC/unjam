#!/bin/sh
# SPDX-License-Identifier: LGPL-3.0-or-later
# SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany
#
# Checks the release binary size and the resolved crate count against docs/budget.txt.
# Usage: scripts/size.sh [--self-test] [BINARY]   (default BINARY: target/release/unjam)
# --self-test runs each check against a limit that must fail, then the real check.
set -eu

budget=${BUDGET:-docs/budget.txt}

limit() {
	awk -v key="$1" '$1 == key { print $2 }' "$budget"
}

crates() {
	cargo tree --locked --color never -e normal --prefix none --format '{p}' --target x86_64-unknown-linux-gnu |
		sed 's/ (\*)$//; s/ (proc-macro)$//' | grep -v '^unjam v' | sort -u | wc -l
}

# check BINARY MAX_BYTES MAX_CRATES: prints both numbers, fails if either exceeds its limit.
check() {
	bytes=$(stat -c %s "$1")
	count=$(crates)
	echo "binary: $bytes bytes (limit $2)"
	echo "crates: $count (limit $3)"
	ok=0
	[ "$bytes" -le "$2" ] || { echo "error: binary exceeds binary_bytes" >&2; ok=1; }
	[ "$count" -le "$3" ] || { echo "error: crate count exceeds crates" >&2; ok=1; }
	return $ok
}

self_test=0
if [ "${1:-}" = "--self-test" ]; then self_test=1; shift; fi
bin=${1:-target/release/unjam}
[ -f "$bin" ] || { echo "error: $bin not found; run 'mise run build'" >&2; exit 1; }
max_bytes=$(limit binary_bytes)
max_crates=$(limit crates)
[ -n "$max_bytes" ] && [ -n "$max_crates" ] || { echo "error: no limits in $budget" >&2; exit 1; }

if [ "$self_test" = 1 ]; then
	if check "$bin" 0 "$max_crates" >/dev/null 2>&1; then
		echo "self-test: binary limit 0 did not fail" >&2; exit 1
	fi
	if check "$bin" "$max_bytes" 0 >/dev/null 2>&1; then
		echo "self-test: crate limit 0 did not fail" >&2; exit 1
	fi
	echo "self-test: both limits fail when lowered"
fi

report=$(check "$bin" "$max_bytes" "$max_crates") || { echo "$report"; exit 1; }
echo "$report"
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
	printf '### Size\n\n```\n%s\n```\n' "$report" >>"$GITHUB_STEP_SUMMARY"
fi
echo "duplicate crates (information):"
cargo tree --locked --color never -d -e normal --target x86_64-unknown-linux-gnu 2>/dev/null || true
