use super::*;

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct State {
    pane: String,
    generation: u32,
}

const AREA: RecordArea<State> = RecordArea::new("ops-fixture");

#[test]
fn typed_records_round_trip_across_restart_and_keep_view_identity() {
    // Given a typed document and a pinned view.
    let root = tempfile::tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    let first = State {
        pane: "first".into(),
        generation: 1,
    };
    AREA.save(&store, "state", &first).unwrap();
    let snapshot = store.read_only_snapshot().unwrap();
    // When another handle replaces the document.
    let second = State {
        pane: "second".into(),
        generation: 2,
    };
    AREA.save(&Store::open(root.path()).unwrap(), "state", &second)
        .unwrap();
    // Then restart reads the replacement, while the view keeps its committed value.
    assert_eq!(
        AREA.load(&Store::open(root.path()).unwrap(), "state")
            .unwrap(),
        Some(second)
    );
    assert_eq!(AREA.load(&snapshot, "state").unwrap(), Some(first));
    assert_eq!(AREA.list(&snapshot).unwrap().len(), 1);
    assert!(!store.dir().join("state").join("ops-fixture.json").exists());
}

#[test]
fn failed_task_update_discards_typed_record_writes() {
    // Given a task with no application record.
    let root = tempfile::tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    let task = store
        .create("fixture", None, crate::Status::Backlog, "")
        .unwrap();
    // When a callback stages a record and then refuses its task change.
    let result = store.update(&task.id, |_| {
        AREA.save(
            &store,
            "state",
            &State {
                pane: "refused".into(),
                generation: 1,
            },
        )?;
        anyhow::bail!("refused");
    });
    // Then neither this handle nor a restarted process sees that record.
    assert!(result.is_err());
    assert!(
        AREA.load(&Store::open(root.path()).unwrap(), "state")
            .unwrap()
            .is_none()
    );
}

#[test]
fn unknown_document_versions_are_refused_without_rewriting() {
    // Given a future document.
    let root = tempfile::tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    let doc = serde_json::json!({"doc_version": 2, "value": {"pane": "future", "generation": 9}});
    store.save_record("ops-fixture", "state", &doc).unwrap();
    // When a version-one reader tries to consume it.
    let result = AREA.load(&store, "state");
    // Then it refuses and leaves authority intact.
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("unsupported ops-fixture document version 2")
    );
    assert_eq!(
        store
            .record_row("ops-fixture", "state")
            .unwrap()
            .unwrap()
            .doc,
        doc
    );
}

#[test]
fn committed_intent_survives_a_failed_external_publication_callback() {
    // Given an intent journal protecting independently published children.
    let root = tempfile::tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    let task = store
        .create("publication", None, crate::Status::Backlog, "")
        .unwrap();
    let intent = State {
        pane: "reserved-child".into(),
        generation: 1,
    };
    // When publication stops after the intent has committed.
    let result = store.update(&task.id, |_| {
        AREA.save_intent(&store, &task.id, &intent)?;
        anyhow::bail!("child publication interrupted");
    });
    // Then restart can reconcile the retained intent and the parent is unchanged.
    assert!(result.is_err());
    let reopened = Store::open(root.path()).unwrap();
    assert_eq!(AREA.load(&reopened, &task.id).unwrap(), Some(intent));
    assert_eq!(
        reopened.get(&task.id).unwrap().status,
        crate::Status::Backlog
    );
}
