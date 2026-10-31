//! Real per-row publication refusal, not a filesystem shadow or a reader mode.
use corral_core::{Store, backend::RecordWrite};
use serde_json::json;
const AREA: &str = "quoted'fixture";
const KEY: &str = "proof'key";

#[test]
fn independent_record_writer() {
    let Some(root) = std::env::var_os("CORRAL_FIXTURE_RECORD_FAULT_ROOT") else {
        return;
    };
    let store = Store::open_unobserved(std::path::Path::new(&root)).unwrap();
    assert_eq!(
        store.record::<serde_json::Value>(AREA, KEY).unwrap(),
        Some(json!({"original":true}))
    );
    assert!(
        store
            .save_record(AREA, KEY, &json!({"changed":true}))
            .is_err()
    );
}

#[test]
fn exact_record_fault_is_visible_after_reopen_and_rolls_back_the_whole_batch() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    store
        .save_record(AREA, KEY, &json!({"original":true}))
        .unwrap();
    let original = store.record_rows(AREA).unwrap();
    let events = store.events().unwrap();
    store.fixture_block_record(AREA, KEY, true).unwrap();
    let result = store.put_records(vec![
        RecordWrite {
            area: "unrelated".into(),
            key: "new".into(),
            expected: None,
            doc: Some(json!(1)),
            updated: store.now(),
        },
        RecordWrite {
            area: AREA.into(),
            key: KEY.into(),
            expected: Some(original[0].version),
            doc: Some(json!({"changed":true})),
            updated: store.now(),
        },
    ]);
    assert!(result.is_err());
    assert_eq!(store.record_rows(AREA).unwrap(), original);
    assert!(
        store
            .record::<serde_json::Value>("unrelated", "new")
            .unwrap()
            .is_none()
    );
    assert_eq!(store.events().unwrap(), events);
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "independent_record_writer", "--nocapture"])
        .env("CORRAL_FIXTURE_RECORD_FAULT_ROOT", root.path())
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    store.fixture_block_record(AREA, KEY, false).unwrap();
    store
        .save_record(AREA, KEY, &json!({"changed":true}))
        .unwrap();
    assert_eq!(
        store.record::<serde_json::Value>(AREA, KEY).unwrap(),
        Some(json!({"changed":true}))
    );
}
