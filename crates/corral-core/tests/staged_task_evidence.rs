//! Evidence references and their reservation join the actual task operation.
use corral_core::backend::BlobState;
use corral_core::blob_ref::BlobHash;
use corral_core::{Status, Store};
use serde_json::json;

#[test]
fn evidence_staging_requires_a_unit_and_callback_refusal_publishes_nothing() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    let task = store.create("Before", None, Status::Ready, "").unwrap();
    let bytes = b"staged evidence";
    let hash = BlobHash::of(bytes);
    assert!(
        store
            .stage_task_evidence("proof", "one", bytes, |blob| json!({"blob":blob}))
            .is_err()
    );
    let result = store.update(&task.id, |task| {
        store.stage_task_evidence("proof", "one", bytes, |blob| json!({"blob":blob}))?;
        task.title = "Never published".into();
        anyhow::bail!("refused after staging")
    });
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("refused after staging")
    );
    assert_eq!(store.get(&task.id).unwrap().title, "Before");
    assert!(store.record_row("proof", "one").unwrap().is_none());
    let staged = store.blob_row(&hash).unwrap().unwrap();
    assert_eq!(staged.refs, 0);
    assert_ne!(staged.state, BlobState::Live);
}

#[test]
fn full_and_head_units_publish_the_task_record_and_live_blob_together() {
    for headed in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let store = Store::init(root.path()).unwrap();
        let task = store.create("Before", None, Status::Ready, "").unwrap();
        let bytes = b"committed evidence";
        let stage =
            || store.stage_task_evidence("proof", "one", bytes, |blob| json!({"blob":blob}));
        if headed {
            store
                .update_head(&task.id, |head| {
                    stage()?;
                    head.title = "After".into();
                    Ok(())
                })
                .unwrap();
        } else {
            store
                .update(&task.id, |task| {
                    stage()?;
                    task.title = "After".into();
                    Ok(())
                })
                .unwrap();
        }
        assert_eq!(store.get(&task.id).unwrap().title, "After");
        assert!(store.record_row("proof", "one").unwrap().is_some());
        let row = store.blob_row(&BlobHash::of(bytes)).unwrap().unwrap();
        assert_eq!((row.state, row.refs), (BlobState::Live, 1));
    }
}
