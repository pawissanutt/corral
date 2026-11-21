use super::*;
use serde_json::json;
use std::fs;
use windows::TestState;

fn patch(key: &str, value: Value) -> BTreeMap<String, Option<Value>> {
    BTreeMap::from([(key.into(), Some(value))])
}

#[test]
fn windows_stale_revision_does_not_overwrite_another_instance() {
    let TestState(root) = &TestState::new();
    {
        let _held = lock(root).unwrap();
        let contender = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(root.join("corral-app").join("durable.lock"))
            .unwrap();
        assert!(matches!(
            contender.try_lock(),
            Err(fs::TryLockError::WouldBlock)
        ));
    }

    let first = read_in(root).unwrap();
    let second = read_in(root).unwrap();
    assert_eq!(first, (0, BTreeMap::new()));
    assert!(compare_exchange_in(root, first.0, patch("message-a", json!("first"))).unwrap());
    assert!(!compare_exchange_in(root, second.0, patch("message-b", json!("second"))).unwrap());

    let (revision, saved) = read_in(root).unwrap();
    assert_eq!(revision, 1);
    assert_eq!(
        saved,
        BTreeMap::from([("message-a".into(), json!("first"))])
    );
    assert!(compare_exchange_in(root, revision, patch("message-b", json!("second"))).unwrap());
    assert_eq!(
        read_in(root).unwrap(),
        (
            2,
            BTreeMap::from([
                ("message-a".into(), json!("first")),
                ("message-b".into(), json!("second")),
            ]),
        )
    );
}

#[test]
fn windows_oversized_durable_file_is_not_treated_as_empty() {
    let TestState(root) = &TestState::new();
    let (revision, _) = read_in(root).unwrap();
    let path = root.join("corral-app").join("durable.json");
    fs::File::create(&path)
        .unwrap()
        .set_len(MAX_BYTES as u64 + 1)
        .unwrap();
    assert!(read_in(root).is_err());
    assert!(compare_exchange_in(root, revision, patch("message-a", json!("first"))).is_err());
    assert_eq!(fs::metadata(path).unwrap().len(), MAX_BYTES as u64 + 1);
}
