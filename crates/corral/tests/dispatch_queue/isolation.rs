use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::queue_support::Fixture;

fn snapshot(root: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    let mut files = std::collections::BTreeMap::new();
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(snapshot(&path));
        } else {
            files.insert(path.clone(), std::fs::read(&path).unwrap());
        }
    }
    files
}

#[test]
fn inherited_workspace_and_gate_identity_cannot_change_sentinel_store() {
    let sentinel = tempfile::tempdir().unwrap();
    let store = corral_core::Store::init(sentinel.path()).unwrap();
    store
        .log_event(None, "sentinel", serde_json::json!({"untouched":true}))
        .unwrap();
    let gates = store.state_dir().join("gates");
    std::fs::create_dir(&gates).unwrap();
    std::fs::write(gates.join("sentinel.json"), "sentinel result\n").unwrap();
    std::fs::write(gates.join("sentinel.log"), "sentinel log\n").unwrap();
    let before = snapshot(store.dir());
    // Run the real fixture test with a hostile parent environment, from the project worktree.
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "capacity::finished_reviewer_frees_slot_for_waiting_gate_before_ready_dispatch",
        ])
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .env("CORRAL_ROOT", sentinel.path())
        .env("CORRAL_GATE_ATTEMPT", "sentinel")
        .env("CORRAL_CANDIDATE_HEAD", "deadbeef")
        .env("CORRAL_INTEGRATION_BASE", "deadbeef")
        .env("HERDR_PANE_ID", "sentinel:p1")
        .env("HERDR_WORKSPACE_ID", "sentinel")
        .env("HERDR_TAB_ID", "sentinel:t1")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(snapshot(store.dir()), before, "sentinel store was modified");
    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
}

#[test]
fn queue_children_pin_their_store_and_remove_parent_gate_and_pane_identity() {
    let fixture = Fixture::new();
    let command = fixture.command(&["sync"]);
    let environment: std::collections::BTreeMap<_, _> = command.get_envs().collect();
    assert_eq!(
        environment.get(OsStr::new("CORRAL_ROOT")),
        Some(&Some(fixture.temp.path().as_os_str())),
        "the child must not discover or inherit the parent workspace"
    );
    for name in [
        "CORRAL_GATE_ATTEMPT",
        "CORRAL_CANDIDATE_HEAD",
        "CORRAL_INTEGRATION_BASE",
        "HERDR_PANE_ID",
        "HERDR_WORKSPACE_ID",
        "HERDR_TAB_ID",
    ] {
        assert_eq!(environment.get(OsStr::new(name)), Some(&None), "{name}");
    }
}
