<!--
SPDX-License-Identifier: LGPL-3.0-or-later
SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany
-->

# Contributing

## Code Quality

All code must pass the following checks without warnings:

- [`cargo fmt`](https://github.com/rust-lang/rustfmt)
- [`cargo clippy --all-targets`](https://github.com/rust-lang/rust-clippy)
- `cargo build`
- `cargo test`

These checks are enforced in CI.

## Licensing

All files must include [SPDX](https://spdx.dev/) license headers. See the [SPDX specification](https://spdx.github.io/spdx-spec/) for details. Compliance is checked with [`reuse lint`](https://reuse.software/) in CI.

## Commit Messages

This project follows the [Conventional Commits](https://www.conventionalcommits.org/) specification.

## Releases

Releases are managed with [`cargo-release`](https://github.com/crates-io/cargo-release). Install it with:

```sh
cargo install cargo-release
```

### Setup

`cargo-release` is configured to push to a git remote named `upstream`. Before releasing, add it:

```sh
git remote add upstream git@github.com:GSI-HPC/unjam.git
```

### Creating a Release

```sh
cargo release patch  # 0.1.0 → 0.1.1
cargo release minor  # 0.1.0 → 0.2.0
cargo release major  # 0.1.0 → 1.0.0
```

This bumps the version in `Cargo.toml`, commits, tags, and pushes to `upstream`. CI then runs checks and publishes to [crates.io](https://crates.io/) automatically.
