//! Session readbacks are disposable renderings, never a second authority.
#[cfg(unix)]
#[test]
fn assistant_readbacks_are_read_only_and_tampering_cannot_change_records() {
    use corral_core::Store;
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let store = Store::init(temp.path()).unwrap();
    let first = store.create_assistant("pi", None, None, None).unwrap();
    let second = store.create_assistant("pi", None, None, None).unwrap();
    store
        .update_assistant(&first.id, |session| {
            session.summary = "Canonical summary".into();
            session.body = "Canonical body".into();
            Ok(())
        })
        .unwrap();
    let session = store.get_assistant(&first.id).unwrap();
    let container = store.get_assistant_container(&first.id).unwrap();
    let markdown = store.assistant_path(&first.id).unwrap();
    let json = store.assistant_container_path(&first.id).unwrap();
    for path in [&markdown, &json] {
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o222,
            0
        );
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::write(path, "untrusted rendering").unwrap();
    }
    assert_eq!(store.get_assistant(&first.id).unwrap(), session);
    assert_eq!(store.get_assistant_container(&first.id).unwrap(), container);
    assert!(
        std::fs::read_to_string(store.assistant_path(&first.id).unwrap())
            .unwrap()
            .contains("Canonical body")
    );
    assert_eq!(
        serde_json::from_slice::<corral_core::assistant::AssistantContainer>(
            &std::fs::read(store.assistant_container_path(&first.id).unwrap()).unwrap()
        )
        .unwrap(),
        container
    );
    assert_eq!(
        store.active_assistant_ids().unwrap(),
        vec![first.id.clone(), second.id.clone()]
    );
    store.end_assistant(&first.id).unwrap();
    assert_eq!(store.active_assistant_ids().unwrap(), vec![second.id]);
    let reopened = Store::open(temp.path()).unwrap();
    assert_eq!(
        reopened.get_assistant(&first.id).unwrap().body,
        session.body
    );
    assert!(
        reopened.list().unwrap().is_empty(),
        "sessions never create task history"
    );
}
