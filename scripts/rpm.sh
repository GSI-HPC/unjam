#!/bin/sh
# SPDX-License-Identifier: LGPL-3.0-or-later
# SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany
#
# RPM packages and the dnf repository (.github/workflows/rpm.yml and pages.yml). Run each command
# in a container of the target distribution, as root, from the repository root:
#
#   scripts/rpm.sh build VERSION BINARY OUTDIR   unjam for this distribution and architecture
#   scripts/rpm.sh release VERSION OUTDIR        unjam-release (noarch) for this distribution
#   scripts/rpm.sh sign RPM...                   sign in place and verify the signatures
#   scripts/rpm.sh repo SITEDIR RPM...           repository tree, signed metadata, release links
#
# Packages: rpm-build (build, release), rpm-sign gnupg2 (sign), createrepo_c gnupg2 (repo).
# sign and repo read the armored secret key from $RPM_SIGNING_KEY. The public key is
# $RPM_PUBLIC_KEY (default pkg/rpm/RPM-GPG-KEY-unjam); sign stops if the two do not match.
set -eu

PUBKEY=${RPM_PUBLIC_KEY:-pkg/rpm/RPM-GPG-KEY-unjam}
ARCHES="x86_64 aarch64"

die() { echo "rpm.sh: $*" >&2; exit 1; }

# An RPM Version cannot hold '-': 0.3.0-rc.1 becomes 0.3.0~rc.1, which sorts before 0.3.0.
rpmver() { printf '%s\n' "$1" | sed 's/-/~/g'; }

# rpmbuild SPEC VERSION OUTDIR: build from $SOURCES in a private topdir, copy the packages to OUTDIR.
rpmbuild_to() {
  top=$(mktemp -d)
  mkdir -p "$3"
  # The specs have no %changelog: take no build time from it.
  rpmbuild -bb --quiet --define "_topdir $top" --define "_sourcedir $SOURCES" \
    --define "source_date_epoch_from_changelog 0" \
    --define "ver $(rpmver "$2")" "$1"
  find "$top/RPMS" -name '*.rpm' -exec cp {} "$3/" \;
  rm -rf "$top"
}

# Import $RPM_SIGNING_KEY into a private keyring (GNUPGHOME); set fpr to its fingerprint.
import_key() {
  [ -n "${RPM_SIGNING_KEY:-}" ] || die "RPM_SIGNING_KEY is not set"
  GNUPGHOME=$(mktemp -d)
  export GNUPGHOME
  printf '%s\n' "$RPM_SIGNING_KEY" | gpg --batch --quiet --import
  want=$(gpg --batch --with-colons --show-keys "$PUBKEY" | awk -F: '$1 == "fpr" { print $10; exit }')
  have=$(gpg --batch --with-colons --list-secret-keys | awk -F: '$1 == "fpr" { print $10; exit }')
  [ -n "$want" ] && [ "$want" = "$have" ] ||
    die "the secret key ($have) is not the key in $PUBKEY ($want)"
  fpr=$want
}

cmd=${1:-}
[ $# -gt 0 ] && shift
case $cmd in
  build)
    [ $# -eq 3 ] || die "usage: build VERSION BINARY OUTDIR"
    SOURCES=$(mktemp -d)
    cp "$2" "$SOURCES/unjam"
    cp LICENSES/LGPL-3.0-or-later.txt README.md CHANGELOG.md "$SOURCES/"
    rpmbuild_to pkg/rpm/unjam.spec "$1" "$3"
    ;;
  release)
    [ $# -eq 2 ] || die "usage: release VERSION OUTDIR"
    SOURCES=$(mktemp -d)
    cp "$PUBKEY" "$SOURCES/RPM-GPG-KEY-unjam"
    rpmbuild_to pkg/rpm/unjam-release.spec "$1" "$2"
    ;;
  sign)
    [ $# -gt 0 ] || die "usage: sign RPM..."
    import_key
    rpmsign --addsign --define "_gpg_name $fpr" "$@" > /dev/null
    # Verify against the public key only, in a scratch RPM database.
    db=$(mktemp -d)
    rpmkeys --dbpath "$db" --import "$PUBKEY"
    for f in "$@"; do
      out=$(rpmkeys --dbpath "$db" --checksig "$f")
      echo "$out"
      case $out in *": digests signatures OK") ;; *) die "bad signature: $f" ;; esac
    done
    ;;
  repo)
    [ $# -gt 1 ] || die "usage: repo SITEDIR RPM..."
    site=$1
    shift
    import_key
    # name-version-release.arch.rpm; the dist tag in the release names the directory.
    for f in "$@"; do
      base=${f##*/}
      arch=${base%.rpm}
      arch=${arch##*.}
      case $base in
        *.el[0-9]*.rpm) dist=el/$(printf '%s\n' "$base" | sed -n 's/.*\.el\([0-9][0-9]*\)\..*/\1/p') ;;
        *.fc[0-9]*.rpm) dist=fedora/$(printf '%s\n' "$base" | sed -n 's/.*\.fc\([0-9][0-9]*\)\..*/\1/p') ;;
        *) die "no el or fc dist tag: $base" ;;
      esac
      case $arch in
        noarch) dirs=$ARCHES ;;
        x86_64 | aarch64) dirs=$arch ;;
        *) die "unknown architecture: $base" ;;
      esac
      for a in $dirs; do
        mkdir -p "$site/$dist/$a"
        cp "$f" "$site/$dist/$a/"
      done
    done
    for d in "$site"/el/*/* "$site"/fedora/*/*; do
      [ -d "$d" ] || continue
      # gz metadata and no sqlite: every dnf on EL 9, EL 10 and Fedora reads it.
      createrepo_c --quiet --general-compress-type=gz --no-database "$d"
      gpg --batch --yes --armor --detach-sign "$d/repodata/repomd.xml"
    done
    cp "$PUBKEY" "$site/RPM-GPG-KEY-unjam"
    # Stable links for `dnf install <url>`: the newest unjam-release of each distribution.
    for d in el/9:el9 el/10:el10 fedora/*:fedora; do
      src=$(ls -d "$site"/${d%%:*}/x86_64 2>/dev/null | sort -V | tail -n 1)
      [ -n "$src" ] || continue
      new=$(ls "$src"/unjam-release-*.noarch.rpm 2>/dev/null | sort -V | tail -n 1)
      [ -n "$new" ] && cp "$new" "$site/unjam-release-${d#*:}.noarch.rpm"
    done
    ;;
  *)
    die "usage: rpm.sh build|release|sign|repo ... (see the header)"
    ;;
esac
