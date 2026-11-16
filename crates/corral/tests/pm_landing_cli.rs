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

/// A completed candidate in Review; `review` is its run's review record, if any.
fn candidate(
    accept: Option<Accept>,
    review: Option<serde_json::Value>,
) -> (tempfile::TempDir, Store) {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let task = store.create("Reviewed", None, Status::Review, "").unwrap();
    let mut run = json!({
        "id":"R1", "agent":"generic", "harness":"generic",
        "worktree":tmp.path(), "branch":"corral/T-001", "base":"main", "base_sha":"fixture",
        "started":"2026-10-29T01:00:00Z", "ended":"2026-10-29T01:10:00Z", "outcome":"done"
    });
    if let Some(review) = review {
        run["review"] = review;
    }
    let run: Run = serde_json::from_value(run).unwrap();
    store
        .update(&task.id, |task| {
            task.runs.push(run);
            task.review = accept;
            Ok(())
        })
        .unwrap();
    (tmp, store)
}

/// The PM's shell is stopped by the landing fence; the owner's is not, on the same candidate.
fn only_the_pm_is_fenced(tmp: &Path, store: &Store) {
    let pm = approve(tmp, true);
    let owner = approve(tmp, false);
    let refused = String::from_utf8_lossy(&pm.stderr);
    assert!(!pm.status.success(), "{refused}");
    assert!(refused.contains("pm_landing_refused"), "{refused}");
    assert!(
        !String::from_utf8_lossy(&owner.stderr).contains("pm_landing_refused"),
        "the owner keeps the override path"
    );
    assert_eq!(store.get("T-001").unwrap().status, Status::Review);
}

#[test]
fn pm_shell_cannot_land_where_acceptance_is_explicitly_the_owners() {
    let (tmp, store) = candidate(Some(Accept::Owner), None);
    only_the_pm_is_fenced(tmp.path(), &store);
}

#[test]
fn pm_shell_cannot_land_over_a_changes_verdict_policy_did_not_require() {
    // Given no configured reviewer, but a review that was run anyway and asked for changes.
    let (tmp, store) = candidate(
        None,
        Some(json!({
            "reviewer":"fixture", "verdict":"changes", "sha":"fixture",
            "at":"2026-10-29T01:20:00Z"
        })),
    );
    only_the_pm_is_fenced(tmp.path(), &store);
}
