// SPDX-License-Identifier: LGPL-3.0-or-later
// SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany

//! `unjam record` as a process: what the user sees when recording cannot start.

use std::process::Command;

#[test]
fn record_warns_when_it_runs_the_app_unrecorded() {
    let missing = std::env::temp_dir().join(format!("unjam-no-such-dir-{}", std::process::id()));
    let out = Command::new(env!("CARGO_BIN_EXE_unjam"))
        .arg("record")
        .arg("--root")
        .arg(&missing)
        .arg("-o")
        .arg(missing.join("x.arrows"))
        .args(["--", "sh", "-c", "exit 3"])
        .env_remove("SLURM_LOCALID")
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "the app ran and its code passed through: {err}");
    assert!(err.starts_with("unjam: record: ") && err.contains("; running unrecorded"), "{err}");
    assert!(!missing.exists());
}
