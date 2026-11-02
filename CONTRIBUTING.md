<!--
SPDX-License-Identifier: LGPL-3.0-or-later
SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany
-->

# Contributing

## Setup

The Rust toolchain comes from `rust-toolchain.toml` through [rustup](https://rustup.rs/). All
other tools are pinned in `mise.toml`; install them with [mise](https://mise.jdx.dev/):

```sh
mise install
```

The integration tests also need [sind](https://github.com/GSI-HPC/sind) v0.15.0 (see its
installation guide) and a rootful Docker daemon on cgroup v2. sind is not a mise tool because
mise resolves its releases through the GitHub API; `mise use -g github:GSI-HPC/sind@0.15.0`
installs it where that API is reachable.

## Code Quality

All code must pass, without warnings:

```sh
mise run check   # fmt, clippy, tests (incl. budget gates), reuse, cargo-deny, size, unsafe ledger
```

Single gates: `mise run fmt`, `lint`, `test`, `reuse`, `deny`, `size`, `unsafe-deps`, `msrv`
(`rust-version` 1.88). `mise tasks` lists them all. CI runs the same tasks.

The budgets are part of the code: `docs/budget.txt` (lines per module, binary size, crate
count), `docs/deps.md` (one row per dependency) and `docs/unsafe.md` (one entry per `unsafe`
use, one row per dependency with `unsafe`). The skills in `.claude/skills/` describe the rules.

## Integration Tests

```sh
mise run it                       # create the sind cluster, test, delete it
SLURM=25.11 mise run it           # Slurm release line (default 26.05)
EXTRA=1 JOBIDPATHS=1 mise run it  # EnableExtraControllers=io,pids, CgroupJobIdPaths=yes
```

`mise run it:up`, `it:test` and `it:down` run the steps one at a time. The nodes run the static
binary from `mise run build:musl`, since the node images may have an older glibc than the host.

## Licensing

All files must include [SPDX](https://spdx.dev/) license headers. See the [SPDX specification](https://spdx.github.io/spdx-spec/) for details. Files that cannot carry a header (`.claude/**`, `tests/fixtures/**`) are covered by `REUSE.toml`. Compliance is checked with [`reuse lint`](https://reuse.software/) (`mise run reuse`) in CI.

## Commit Messages

This project follows the [Conventional Commits](https://www.conventionalcommits.org/) specification.

## Releases

Releases are managed with [`cargo-release`](https://github.com/crates-io/cargo-release), pinned in `mise.toml` and installed by `mise install`.

### Setup

`cargo-release` is configured to push to a git remote named `upstream`. Before releasing, add it:

```sh
git remote add upstream git@github.com:GSI-HPC/unjam.git
```

### Creating a Release

```sh
cargo release patch  # 0.1.0 → 0.1.1 (dry run; add --execute to release)
cargo release minor  # 0.1.0 → 0.2.0
cargo release major  # 0.1.0 → 1.0.0
```

This bumps the version in `Cargo.toml`, commits, tags, and pushes to `upstream`. Add a `## [X.Y.Z] - DATE` section to `CHANGELOG.md` in the same commit: it becomes the release notes, and the tag job fails without it.

CI then runs the following steps for the tag, which must be on `main`:

1. Run the checks and integration tests.
2. Build static x86_64 and aarch64 binaries with build provenance attestations and checksums.
3. Package the binaries as RPMs for Rocky Linux 9, Rocky Linux 10 and the latest Fedora (`rpm.yml`), sign them with the key of the `rpm-signing` environment and attest them.
4. Create a **draft** GitHub release with the binaries, the RPMs and the CHANGELOG section as notes.
5. Wait for a reviewer of the `crates-io` environment to approve, then publish to [crates.io](https://crates.io/) with Trusted Publishing.

Publish the draft release by hand after the crate is on crates.io. Publishing starts `pages.yml`: it builds the dnf repository at <https://gsi-hpc.github.io/unjam/rpm/> again from the RPMs of all published releases, deploys it and installs unjam from it on each distribution. `mise-install.yml` also starts: it installs the release with `mise use github:GSI-HPC/unjam` on both architectures and runs `unjam --version`. mise checks the attestation itself, so a failure there means the release assets or their provenance are wrong. To add RPMs to a release that has none, run `rpm.yml` by hand with its tag.

### RPM Signing Key

One RSA key signs all packages and the repository metadata. Its public half is `pkg/rpm/RPM-GPG-KEY-unjam`; `scripts/rpm.sh` stops if the secret key does not match it. To make the key (once):

```sh
export GNUPGHOME=$(mktemp -d)
gpg --batch --passphrase '' --quick-gen-key 'unjam packages (GSI-HPC/unjam)' rsa4096 sign never
gpg --armor --export > pkg/rpm/RPM-GPG-KEY-unjam          # commit this file
gpg --armor --export-secret-keys > unjam-rpm-signing.asc  # keep offline, then delete here
gh api -X PUT repos/GSI-HPC/unjam/environments/rpm-signing \
  -F 'deployment_branch_policy[protected_branches]=false' -F 'deployment_branch_policy[custom_branch_policies]=true'
gh api repos/GSI-HPC/unjam/environments/rpm-signing/deployment-branch-policies -f name=main -f type=branch
gh api repos/GSI-HPC/unjam/environments/rpm-signing/deployment-branch-policies -f name='v*' -f type=tag
gh secret set RPM_SIGNING_KEY --env rpm-signing -R GSI-HPC/unjam < unjam-rpm-signing.asc
rm -rf "$GNUPGHOME"
```

The environment gives the key only to runs on `main` and `v*` tags, never to a pull request. GitHub Pages must deploy from GitHub Actions (Settings → Pages → Source).
