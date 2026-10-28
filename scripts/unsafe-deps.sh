#!/bin/sh
# SPDX-License-Identifier: LGPL-3.0-or-later
# SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany
#
# Checks the "Dependencies" table of docs/unsafe.md against the resolved dependency tree: every
# crate whose source contains the word `unsafe` needs a row, and no row may name a crate that
# is not in the tree. The search covers source text, so cfg'd-out code counts too.
# Usage: scripts/unsafe-deps.sh [--print | --self-test]
#   --print      print stub rows for crates that lack one
#   --self-test  expect failure for a ledger copy with one row removed and with a stale row
set -eu

ledger=${LEDGER:-docs/unsafe.md}
registry="${CARGO_HOME:-$HOME/.cargo}/registry/src"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# Writes the sorted names of in-tree crates with `unsafe` in their source to $tmp/have.
scan() {
	cargo fetch --locked --target x86_64-unknown-linux-gnu >/dev/null 2>&1 || true
	cargo tree --locked --color never -e normal --prefix none --format '{p}' --target x86_64-unknown-linux-gnu |
		sed 's/ (\*)$//; s/ (proc-macro)$//' | grep -v '^unjam v' | sort -u >"$tmp/tree"
	: >"$tmp/have"
	while read -r name version; do
		dir=$(ls -d "$registry"/*/"$name-${version#v}" 2>/dev/null | head -n 1)
		if [ -z "$dir" ]; then
			echo "error: no source for $name $version under $registry" >&2
			return 1
		fi
		if grep -rqw --include='*.rs' unsafe "$dir"; then echo "$name" >>"$tmp/have"; fi
	done <"$tmp/tree"
	sort -u -o "$tmp/have" "$tmp/have"
}

# rows LEDGER: prints the sorted crate names of the Dependencies table.
rows() {
	awk '/^# Dependencies/ { d = 1; next } /^# / { d = 0 }
		d && /^\| `/ { split($0, f, "`"); print f[2] }' "$1" | sort -u
}

# check LEDGER: fails on missing or stale rows.
check() {
	rows "$1" >"$tmp/rows"
	missing=$(comm -23 "$tmp/have" "$tmp/rows")
	stale=$(comm -13 "$tmp/have" "$tmp/rows")
	[ -z "$missing" ] || echo "error: crates with unsafe but no row in $1:" $missing >&2
	[ -z "$stale" ] || echo "error: rows for crates not in the tree:" $stale >&2
	[ -z "$missing" ] && [ -z "$stale" ] || return 1
	echo "unsafe ledger: $(wc -l <"$tmp/have") crates with unsafe, all with a row in $1"
}

scan
case "${1:-}" in
--print)
	rows "$ledger" | comm -23 "$tmp/have" - | sed 's/.*/| `&` | TODO: what its unsafe does | TODO: why accepted |/'
	;;
--self-test)
	first=$(head -n 1 "$tmp/have")
	if [ -n "$first" ]; then
		grep -v "^| \`$first\` |" "$ledger" >"$tmp/removed.md"
		if check "$tmp/removed.md" >/dev/null 2>&1; then
			echo "self-test: removing the row of $first did not fail" >&2; exit 1
		fi
	fi
	{ cat "$ledger"; echo '| `no-such-crate` | - | - |'; } >"$tmp/stale.md"
	if check "$tmp/stale.md" >/dev/null 2>&1; then
		echo "self-test: a stale row did not fail" >&2; exit 1
	fi
	echo "self-test: missing and stale rows fail"
	check "$ledger"
	;;
"") check "$ledger" ;;
*) echo "usage: $0 [--print | --self-test]" >&2; exit 2 ;;
esac
