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
        replies.push(changes(&shared, query.clone(), app::SeenCursor::default()).unwrap());
    }
    assert!(changes(&shared, query, app::SeenCursor::default()).is_err());
    assert!(
        changes(
            &shared,
            app::ChangesQuery {
                before: None,
                after: None,
                limit: 201
            },
            app::SeenCursor::default()
        )
        .is_err()
    );
}

#[test]
fn topics_publish_during_writes_and_ignore_lease_only_churn() -> Result<()> {
    let fixture = crate::tests::Fixture::new();
    let ctx = &fixture.ctx;
    let original = ctx.store.create_assistant("pi", None, None, None)?;
    let shared = Shared::new();
    let _fixture = install(&shared);
    let watch = watch(&shared, Key::new(Topic::Assistant, None)?)?;
    let service = service(&shared)?;
    let mut revision = 0;
    let mut reads = 0;
    let mut added = None;

    refresh_with(ctx, &service, 0, &mut revision, |snapshot, topic| {
        reads += 1;
        let rows = rows::read(snapshot, topic)?;
        // An independent writer can commit while a coherent topic is being read.
        added = Some(ctx.store.create_assistant("pi", None, None, None)?);
        Ok(rows)
    })?;
    let first = watch
        .snapshot()?
        .expect("a committed snapshot must not starve behind concurrent writes");
    let Rows::Assistant(rows) = &first.rows else {
        panic!("expected assistant snapshot");
    };
    assert_eq!(
        rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
        vec![original.id.as_str()]
    );

    refresh_with(ctx, &service, 0, &mut revision, |snapshot, topic| {
        reads += 1;
        rows::read(snapshot, topic)
    })?;
    let second = watch.snapshot()?.unwrap();
    let Rows::Assistant(rows) = &second.rows else {
        panic!("expected assistant snapshot");
    };
    let actual: std::collections::BTreeSet<_> = rows.iter().map(|row| &row.id).collect();
    assert_eq!(
        actual,
        std::collections::BTreeSet::from([&original.id, &added.as_ref().unwrap().id])
    );
    assert!(second.rev > first.rev);

    ctx.store
        .save_record("leases", "pm", &serde_json::json!({"held": true}))?;
    refresh_with(ctx, &service, 0, &mut revision, |snapshot, topic| {
        reads += 1;
        rows::read(snapshot, topic)
    })?;
    assert_eq!(reads, 2, "lease bookkeeping must not repeat topic scans");
    assert_eq!(watch.snapshot()?.unwrap().rev, second.rev);

    ctx.store.save_record(
        "test-other",
        "change",
        &serde_json::json!({"updated": true}),
    )?;
    let mut retired = Some(watch);
    let mut renewed = None;
    refresh_with(ctx, &service, 0, &mut revision, |snapshot, topic| {
        let rows = rows::read(snapshot, topic)?;
        drop(retired.take());
        renewed = Some(super::watch(&shared, Key::new(Topic::Assistant, None)?)?);
        Ok(rows)
    })?;
    refresh(ctx, &service, 0, &mut revision)?;
    let renewed = renewed
        .unwrap()
        .snapshot()?
        .expect("a renewed watch must not inherit a retired cache stamp without its data");
    assert!(renewed.rows == second.rows);
    Ok(())
}
