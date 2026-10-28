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

This bumps the version in `Cargo.toml`, commits, tags, and pushes to `upstream`. CI then runs the checks and integration tests, publishes to [crates.io](https://crates.io/), and attaches static x86_64 and aarch64 binaries with build provenance attestations and checksums to the GitHub release.
