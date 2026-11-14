use super::*;

pub(crate) struct Fixture {
    _receiver: mpsc::Receiver<Job>,
}

pub(crate) fn install(shared: &Shared) -> Fixture {
    let (sender, receiver) = mpsc::sync_channel(QUEUE);
    *shared.views.lock().unwrap() = Some(Arc::new(Service {
        sender,
        entries: Mutex::new(BTreeMap::new()),
    }));
    Fixture {
        _receiver: receiver,
    }
}

pub(crate) fn publish(shared: &Shared, key: &Key, rows: Rows, rev: u64) {
    service(shared)
        .unwrap()
        .entries
        .lock()
        .unwrap()
        .get_mut(key)
        .unwrap()
        .published = Some(Ok(Arc::new(Snapshot { rev, rows })));
}

#[test]
fn scopes_have_one_canonical_subscription_identity() {
    assert_eq!(
        Key::new(Topic::Inbox, Some("ignored".into())).unwrap(),
        Key::new(Topic::Inbox, None).unwrap()
    );
    assert!(Key::new(Topic::Thread, None).is_err());
    for invalid in ["", "A-0", "A-01", "A-+1", "A--1", "A-1/../../", "quick "] {
        assert!(Key::new(Topic::Thread, Some(invalid.into())).is_err());
    }
    assert!(Key::new(Topic::Thread, Some("A-12".into())).is_ok());
    assert!(Key::new(Topic::Thread, Some("quick".into())).is_ok());
}

#[test]
fn watcher_drop_releases_only_its_own_subscription() {
    let shared = Shared::new();
    let _fixture = install(&shared);
    let key = Key::new(Topic::Inbox, None).unwrap();
    let first = watch(&shared, key.clone()).unwrap();
    let second = watch(&shared, key.clone()).unwrap();
    let service = service(&shared).unwrap();
    assert_eq!(service.entries.lock().unwrap()[&key].users, 2);
    drop(first);
    assert_eq!(service.entries.lock().unwrap()[&key].users, 1);
    drop(second);
    assert!(service.entries.lock().unwrap().is_empty());
}

#[test]
fn read_admission_is_bounded_without_waiting_for_the_worker() {
    let shared = Shared::new();
    let _fixture = install(&shared);
    let query = app::ChangesQuery {
        before: None,
        after: None,
        limit: 1,
    };
    let mut replies = Vec::new();
    for _ in 0..QUEUE {
        replies.push(changes(&shared, query.clone()).unwrap());
    }
    assert!(changes(&shared, query).is_err());
    assert!(
        changes(
            &shared,
            app::ChangesQuery {
                before: None,
                after: None,
                limit: 201
            }
        )
        .is_err()
    );
}
