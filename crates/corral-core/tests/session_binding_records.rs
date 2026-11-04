use std::sync::Arc;
use std::time::Duration;

use corral_core::backend::{
    BackendConfig, ManualClock, SESSION_AREA, SESSION_KEY, fake::FakeBackend,
};
use corral_core::{HerdrSessionBinding, Run, Status, Store};
use serde_json::json;

#[test]
fn binding_record_preserves_original_evidence_across_reopens() {
    // Given a durable explicit binding.
    let temp = tempfile::tempdir().unwrap();
    let store = Store::init(temp.path()).unwrap();
    let socket = temp.path().join("owner.sock");
    store
        .bind_herdr_session(&socket, "original evidence")
        .unwrap();
    let before = store
        .record_row(SESSION_AREA, SESSION_KEY)
        .unwrap()
        .unwrap();

    // When another opener repeats the binding with different evidence.
    let reopened = Store::open_unobserved(temp.path()).unwrap();
    reopened
        .bind_herdr_session(&socket, "later evidence")
        .unwrap();

    // Then the typed record and its version are unchanged, with no legacy file.
    assert_eq!(
        reopened.record_row(SESSION_AREA, SESSION_KEY).unwrap(),
        Some(before.clone())
    );
    let binding: HerdrSessionBinding =
        serde_json::from_value(before.doc["binding"].clone()).unwrap();
    assert_eq!(binding.socket, socket);
    assert_eq!(binding.launch_evidence, "original evidence");
    assert!(!store.dir().join("herdr-session.json").exists());
}

#[test]
fn snapshot_ownership_reads_binding_and_journal_from_its_original_commit() {
    // Given a snapshot captured before either ownership record exists.
    let temp = tempfile::tempdir().unwrap();
    let store = Store::init(temp.path()).unwrap();
    let snapshot = store.read_only_snapshot().unwrap();
    let socket = temp.path().join("owner.sock");

    // When a second opener writes both records before the snapshot first evaluates ownership.
    let other = Store::open_unobserved(temp.path()).unwrap();
    other
        .save_record("review-launch", "current", &json!({"socket": socket}))
        .unwrap();
    other.bind_herdr_session(&socket, "record").unwrap();

    // Then the old snapshot sees neither record; a fresh view sees both together.
    let old = snapshot.session_view().unwrap();
    assert!(old.binding.is_none());
    assert!(old.pending.is_none());
    let fresh = store.read_session_view().unwrap();
    assert_eq!(fresh.binding.as_ref().unwrap().socket, socket);
    assert_eq!(fresh.pending, Some(Some(socket)));
}

#[test]
fn pending_record_refuses_a_conflicting_binding_and_preserves_legacy_unknown() {
    for saved in [true, false] {
        // Given a saved or legacy auxiliary launch in backend records.
        let temp = tempfile::tempdir().unwrap();
        let store = Store::init(temp.path()).unwrap();
        let owner = temp.path().join("owner.sock");
        let foreign = temp.path().join("foreign.sock");
        let socket = saved.then_some(&owner);
        store
            .save_record("review-launch", "current", &json!({"socket": socket}))
            .unwrap();

        // When ownership is evaluated or a conflicting explicit binding is attempted.
        let view = store.read_session_view().unwrap();
        assert_eq!(view.owns(&owner).unwrap(), saved);
        let result = store.bind_herdr_session(&foreign, "attestation");

        // Then saved evidence prevents transfer, while legacy ownership needs attestation.
        assert_eq!(result.is_err(), saved);
        if saved {
            assert!(store.herdr_session_binding().unwrap().is_none());
        } else {
            assert!(store.read_session_view().unwrap().owns(&foreign).unwrap());
        }
    }
}

#[test]
fn cached_pass_ownership_expires_with_its_read_view() {
    // Given a pass on an injectable backend clock, with its verdict already cached.
    let temp = tempfile::tempdir().unwrap();
    let clock = Arc::new(ManualClock::new("2026-01-01T00:00:00Z".parse().unwrap()));
    let backend = FakeBackend::new(BackendConfig::default(), clock.clone(), Arc::default());
    let store = Store::init(temp.path())
        .unwrap()
        .with_backend(Box::new(backend));
    store.begin_session_view().unwrap();
    let view = store.session_view().unwrap();
    assert!(view.owns(&temp.path().join("owner.sock")).unwrap());

    // When that pass outlives its configured view age.
    clock.advance(Duration::from_secs(121));

    // Then neither a cached view nor its cached verdict can bypass expiry.
    assert!(store.session_view().is_err());
    assert!(view.owns(&temp.path().join("owner.sock")).is_err());
}

#[test]
fn inventory_refresh_does_not_install_a_pass_on_a_reusable_store() {
    let temp = tempfile::tempdir().unwrap();
    let clock = Arc::new(ManualClock::new("2026-01-01T00:00:00Z".parse().unwrap()));
    let backend = FakeBackend::new(BackendConfig::default(), clock.clone(), Arc::default());
    let mut store = Store::init(temp.path())
        .unwrap()
        .with_backend(Box::new(backend));
    store.config.storage.view_max_age = 5; // Explicit expiry seam, not the global default.
    let socket = temp.path().join("owner.sock");

    // Inventory may temporarily lease a reusable command context; it is not a sync pass.
    let lease = store.try_inventory_session_pass().unwrap().unwrap();
    let sampled = store.session_view().unwrap();
    drop(lease);
    clock.advance(Duration::from_secs(6));
    assert!(sampled.owns(&socket).is_err());
    assert!(store.session_view().unwrap().owns(&socket).unwrap());

    // A deliberately begun pass still stays pinned and fails closed after its own expiry.
    store.begin_session_view().unwrap();
    let lease = store.try_inventory_session_pass().unwrap().unwrap();
    drop(lease);
    clock.advance(Duration::from_secs(6));
    assert!(store.session_view().is_err());
}

#[test]
fn refreshable_ownership_guards_reuse_fresh_views_and_recheck_after_expiry() {
    let temp = tempfile::tempdir().unwrap();
    let clock = Arc::new(ManualClock::new("2026-01-01T00:00:00Z".parse().unwrap()));
    let backend = FakeBackend::new(BackendConfig::default(), clock.clone(), Arc::default());
    let mut store = Store::init(temp.path())
        .unwrap()
        .with_backend(Box::new(backend));
    store.config.storage.view_max_age = 5; // Keep the original short-view regression.
    let owner = temp.path().join("owner.sock");
    store.begin_refreshable_session_view().unwrap();
    let before = store.session_view().unwrap();
    assert!(before.owns(&owner).unwrap());
    assert!(Arc::ptr_eq(&before, &store.session_view().unwrap()));

    // A long external step outlives the five-second view; ownership may change meanwhile.
    clock.advance(Duration::from_secs(6));
    let foreign = temp.path().join("foreign.sock");
    store
        .bind_herdr_session(&foreign, "new launch evidence")
        .unwrap();
    let after = store.session_view().unwrap();
    assert!(!Arc::ptr_eq(&before, &after));
    assert!(!after.owns(&owner).unwrap());
    assert!(before.owns(&owner).is_err(), "old snapshots never revive");
    assert!(Arc::ptr_eq(&after, &store.session_view().unwrap()));
}

#[test]
fn session_binding_record_process_b() {
    let (Some(root), Some(socket)) = (
        std::env::var_os("T323_BINDING_ROOT"),
        std::env::var_os("T323_BINDING_SOCKET"),
    ) else {
        return;
    };
    let store = Store::open_unobserved(std::path::Path::new(&root)).unwrap();
    if let Err(error) = store.bind_herdr_session(std::path::Path::new(&socket), "process B") {
        eprintln!("{error:#}");
        std::process::exit(3);
    }
}

#[test]
fn record_binding_obeys_two_process_leases_for_saved_and_legacy_references() {
    for saved in [true, false] {
        // Given two-process ownership with either saved or legacy pane references.
        let temp = tempfile::tempdir().unwrap();
        let store = Store::init(temp.path()).unwrap();
        let socket = temp.path().join("owner.sock");
        let mut task = store.create("worker", None, Status::Running, "").unwrap();
        let run: Run = serde_json::from_value(json!({
            "id":"R1", "agent":"worker", "worktree":temp.path(),
            "branch":"main", "base":"main", "base_sha":"fixture",
            "started":"2026-01-01T00:00:00Z",
            "herdr":{"workspace_id":"w1","pane_id":"w1:p1",
                "socket":saved.then_some(&socket)}
        }))
        .unwrap();
        task.runs.push(run);
        store.fixture_put_task(&task).unwrap();
        let pass = store.clone();
        let lease = pass.try_begin_session_pass().unwrap().unwrap();
        let before = pass.session_view().unwrap();
        let bind = || {
            std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "session_binding_record_process_b", "--nocapture"])
                .env("T323_BINDING_ROOT", temp.path())
                .env("T323_BINDING_SOCKET", &socket)
                .stdin(std::process::Stdio::null())
                .output()
                .unwrap()
        };

        // When B attempts to bind during the leased pass, then after its lease is released.
        let held = bind();
        drop(lease);
        let released = bind();

        // Then B is excluded during inventory; the old pass stays pinned and new reads see B.
        assert_eq!(held.status.code(), Some(3), "{held:?}");
        assert!(released.status.success(), "{released:?}");
        assert!(before.binding.is_none());
        assert!(pass.session_view().unwrap().binding.is_none());
        assert_eq!(before.owns(&socket).unwrap(), saved);
        let next = store.read_session_view().unwrap();
        assert_eq!(next.binding.as_ref().unwrap().socket, socket);
        assert!(next.owns(&socket).unwrap());
    }
}
