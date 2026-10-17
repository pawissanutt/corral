use std::path::Path;

use corral_core::{Run, Status, Store};
use serde_json::json;

#[test]
fn ownership_binding_is_durable_idempotent_and_cannot_transfer() {
    // Given an explicit original launch record in a private store.
    let temp = tempfile::tempdir().unwrap();
    let store = Store::init(temp.path()).unwrap();
    let socket = store.root().join("original-herdr.sock");
    store
        .bind_herdr_session(&socket, "original plugin registration")
        .unwrap();
    let saved = std::fs::read(store.dir().join("herdr-session.json")).unwrap();

    // When reopening, repeating a binding, or attempting to change its server.
    let store = Store::open_unobserved(temp.path()).unwrap();
    store.bind_herdr_session(&socket, "a later record").unwrap();
    assert!(
        store
            .bind_herdr_session(
                &store.root().join("foreign-herdr.sock"),
                "foreign registration"
            )
            .is_err()
    );

    // Then the immutable evidence survives unchanged, outside rebuildable state.
    assert_eq!(
        std::fs::read(store.dir().join("herdr-session.json")).unwrap(),
        saved
    );
    let binding = store.herdr_session_binding().unwrap().unwrap();
    assert_eq!(binding.socket, socket);
    assert_eq!(binding.launch_evidence, "original plugin registration");
}

#[test]
fn binding_refuses_missing_evidence_or_relative_socket() {
    // Given a store without ownership evidence.
    let temp = tempfile::tempdir().unwrap();
    let store = Store::init(temp.path()).unwrap();

    // When either required migration input is absent.
    assert!(
        store
            .bind_herdr_session(Path::new("relative.sock"), "record")
            .is_err()
    );
    assert!(
        store
            .bind_herdr_session(&store.root().join("herdr.sock"), " \n")
            .is_err()
    );

    // Then no binding is installed.
    assert!(store.herdr_session_binding().unwrap().is_none());
}

#[test]
fn binding_cannot_override_saved_worker_or_nested_reviewer_socket() {
    for child in [false, true] {
        // Given saved launch ownership on a worker or reviewer child.
        let temp = tempfile::tempdir().unwrap();
        let store = Store::init(temp.path()).unwrap();
        let task = store.create("owned", None, Status::Running, "").unwrap();
        let mut run: Run = serde_json::from_value(json!({
            "id":"R1", "agent":"generic", "worktree":temp.path(),
            "branch":"main", "base":"main", "base_sha":"fixture",
            "started":"2000-01-01T00:00:00Z",
            "herdr":{"workspace_id":"w1","pane_id":"w1:p1","socket":store.root().join("original-herdr.sock")}
        }))
        .unwrap();
        if child {
            let reviewer = run.clone();
            run.herdr = None;
            run.review_runs.push(reviewer);
        }
        store
            .update(&task.id, |task| {
                task.runs.push(run);
                Ok(())
            })
            .unwrap();

        // When migration claims a conflicting server.
        let result =
            store.bind_herdr_session(&store.root().join("foreign-herdr.sock"), "foreign record");

        // Then existing launch evidence wins and no binding is written.
        assert!(result.is_err());
        assert!(store.herdr_session_binding().unwrap().is_none());
    }
}

#[test]
fn binding_cannot_override_gate_only_or_pending_auxiliary_socket() {
    for pending in [false, true] {
        // Given auxiliary launch evidence without any worker reference.
        let temp = tempfile::tempdir().unwrap();
        let store = Store::init(temp.path()).unwrap();
        let socket = store.root().join("original-herdr.sock");
        if pending {
            std::fs::write(
                store.state_dir().join("review-launch.json"),
                serde_json::to_vec(&json!({"socket":socket})).unwrap(),
            )
            .unwrap();
        } else {
            let task = store.create("gate only", None, Status::Review, "").unwrap();
            let run: Run = serde_json::from_value(json!({
                "id":"R1", "agent":"generic", "worktree":temp.path(),
                "branch":"main", "base":"main", "base_sha":"fixture",
                "started":"2000-01-01T00:00:00Z",
                "gate":{"result":"running","sha":"fixture","pane":"w1:p1",
                    "socket":socket,"at":"2000-01-01T00:00:00Z"}
            }))
            .unwrap();
            store
                .update(&task.id, |task| {
                    task.runs.push(run);
                    Ok(())
                })
                .unwrap();
        }

        // When migration claims a different server.
        let result =
            store.bind_herdr_session(&store.root().join("foreign-herdr.sock"), "foreign record");

        // Then the reservation or gate socket prevents installing a conflicting binding.
        assert!(result.is_err());
        assert!(store.herdr_session_binding().unwrap().is_none());
        store
            .bind_herdr_session(&socket, "original record")
            .unwrap();
    }
}
