//! A pinned store clock must govern initial immutable headers, not only later updates.
use corral_core::{Status, Store};

#[test]
fn creation_and_batch_drafts_use_the_store_clock_for_their_immutable_created_stamp() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let at = "2099-01-01T00:00:00Z".parse().unwrap();
    store.fixture_pin_now(at).unwrap();
    let mut tasks = vec![store.create("Single", None, Status::Ready, "").unwrap()];
    tasks.extend(
        store
            .create_batch(2, false, |_, tasks| {
                for (n, task) in tasks.iter_mut().enumerate() {
                    task.title = format!("Batch {n}");
                }
                Ok(Vec::new())
            })
            .unwrap(),
    );
    for task in tasks {
        assert_eq!(task.created, at);
        assert_eq!(task.updated, at);
        assert_eq!(task.status_since, Some(at));
        let head = store.head(&task.id).unwrap();
        let stored = store.view().unwrap().head(head.num).unwrap().unwrap();
        assert_eq!(stored.header.created, at);
        assert_eq!(head.created, at);
    }
    let preview = store.create_batch(1, true, |_, _| Ok(Vec::new())).unwrap();
    assert_eq!(preview[0].created, at);
}
