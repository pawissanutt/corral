//! `corral approve` is the owner's override path; a PM shell lands only on the executor's bar.
use std::path::Path;
use std::process::{Command, Output, Stdio};

use corral_core::{Accept, Run, Status, Store};
use serde_json::json;

fn approve(root: &Path, pm: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_corral"));
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if name.starts_with("CORRAL_") || name.starts_with("HERDR_") {
            command.env_remove(key);
        }
    }
    if pm {
        command.env("CORRAL_PM", "1");
    }
    command
        .env("HERDR_BIN_PATH", "/nonexistent/herdr")
        .args(["--root", root.to_str().unwrap(), "approve", "T-001"])
        .current_dir(root)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

#[test]
fn pm_shell_cannot_land_where_acceptance_is_explicitly_the_owners() {
    // Given a completed candidate whose task sets accept = "owner".
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let task = store.create("Reviewed", None, Status::Review, "").unwrap();
    let run: Run = serde_json::from_value(json!({
        "id":"R1", "agent":"generic", "harness":"generic",
        "worktree":tmp.path(), "branch":"corral/T-001", "base":"main", "base_sha":"fixture",
        "started":"2026-10-29T01:00:00Z", "ended":"2026-10-29T01:10:00Z", "outcome":"done"
    }))
    .unwrap();
    store
        .update(&task.id, |task| {
            task.runs.push(run);
            task.review = Some(Accept::Owner);
            Ok(())
        })
        .unwrap();
    // When the PM's shell and then the owner's run the landing command.
    let pm = approve(tmp.path(), true);
    let owner = approve(tmp.path(), false);
    // Then only the PM is stopped by the landing fence, before anything is carried or landed.
    let refused = String::from_utf8_lossy(&pm.stderr);
    assert!(!pm.status.success(), "{refused}");
    assert!(refused.contains("pm_landing_refused"), "{refused}");
    assert!(
        !String::from_utf8_lossy(&owner.stderr).contains("pm_landing_refused"),
        "the owner keeps the override path"
    );
    assert_eq!(store.get("T-001").unwrap().status, Status::Review);
}
