use std::path::Path;

use corral_core::work::{Work, count};
use corral_core::{Run, Status, Store};
use serde_json::json;

#[test]
fn pass_reads_binding_once_blocks_binds_under_its_lease_and_ends_with_its_handle() {
    // Given two independent opens of one unbound store.
    let temp = tempfile::tempdir().unwrap();
    let owner = Store::init(temp.path()).unwrap();
    let other = Store::open_unobserved(temp.path()).unwrap();
    let socket = owner.root().join("owner-herdr.sock");
    let reads = count(Work::SessionBindingRead);

    // When a per-pass handle begins its pass under the shared ownership lease.
    let pass = owner.clone();
    let lease = pass.try_begin_session_pass().unwrap().unwrap();
    let first = pass.session_view().unwrap();
    let again = pass.session_view().unwrap();

    // Then the pass read the binding once, and no bind commits under the lease.
    assert!(std::sync::Arc::ptr_eq(&first, &again));
    assert!(first.binding.is_none());
    assert_eq!(count(Work::SessionBindingRead), reads + 1);
    assert!(other.bind_herdr_session(&socket, "other record").is_err());
    drop(lease);
    other.bind_herdr_session(&socket, "other record").unwrap();
    // The rest of the pass keeps its read; other handles and check_runtime read afresh.
    assert!(pass.session_view().unwrap().binding.is_none());
    let outside = owner.session_view().unwrap();
    assert_eq!(outside.binding.as_ref().unwrap().socket, socket);
    pass.check_runtime().unwrap();
    let refreshed = pass.session_view().unwrap();
    assert_eq!(refreshed.binding.as_ref().unwrap().socket, socket);
    // A later pass's handle never inherits the earlier view.
    drop(pass);
    let next = owner.clone();
    assert!(!std::sync::Arc::ptr_eq(
        &next.session_view().unwrap(),
        &first
    ));
}

#[test]
fn snapshot_pass_reads_binding_once_and_next_snapshot_rereads() {
    // Given a read-only snapshot pass that observed no binding.
    let temp = tempfile::tempdir().unwrap();
    let store = Store::init(temp.path()).unwrap();
    let snapshot = store.read_only_snapshot().unwrap();
    let view = snapshot.session_view().unwrap();
    assert!(view.binding.is_none());

    // When another open binds the store during that pass.
    let socket = store.root().join("owner-herdr.sock");
    Store::open_unobserved(temp.path())
        .unwrap()
        .bind_herdr_session(&socket, "record")
        .unwrap();

    // Then that pass keeps its one read, while the next pass and launch reads are fresh.
    let reads = count(Work::SessionBindingRead);
    let same = snapshot.clone().session_view().unwrap();
    assert!(std::sync::Arc::ptr_eq(&view, &same));
    assert_eq!(count(Work::SessionBindingRead), reads);
    let next = store.read_only_snapshot().unwrap().session_view().unwrap();
    assert_eq!(next.binding.as_ref().unwrap().socket, socket);
    let launch = store.read_session_view().unwrap();
    assert_eq!(launch.binding.as_ref().unwrap().socket, socket);
}

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
