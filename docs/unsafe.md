<!--
SPDX-License-Identifier: LGPL-3.0-or-later
SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany
-->

# unsafe ledger

Production code has no `unsafe` (`unsafe_code = "deny"`). Policy and decision tree:
`.claude/skills/unsafe-policy/SKILL.md` and design §12.

# Our code

One `## <n>. <file>:<item>` entry per `#[allow(unsafe_code, reason = "…")]` site in `src/`:
location, invariants, rejected safe alternative, and the test that exercises it.
`tests/budget.rs` checks that the number of entries equals the number of sites.

No entries yet.

# Dependencies

One row per crate in the resolved normal dependency tree whose source contains `unsafe`.
`scripts/unsafe-deps.sh` checks this table against the tree (no missing and no stale rows);
`scripts/unsafe-deps.sh --print` prints stub rows for missing crates.

| Crate | What its unsafe does | Why accepted |
|---|---|---|
