use super::*;

fn pending(key: &str) -> PendingCause {
    PendingCause {
        envelope_key: key.into(),
        cause: Cause {
            class: CostClass::Item,
            project: Some("project".into()),
            task: Some("T-001".into()),
            kinds: (0..6).map(|n| format!("kind-{n}")).collect(),
            ..Cause::default()
        },
    }
}

#[test]
fn bounds_evict_oldest_pending_and_bound_records_and_count_drops() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    // Only the oldest child is read/deleted by eviction; unrelated child bodies are immaterial.
    let head = Index {
        pending: (0..PENDING_MAX)
            .map(|n| pending_key(&format!("p{n}")))
            .collect(),
        bound: (0..ATTRIBUTIONS_MAX as u64).map(|turn| (1, turn)).collect(),
        dropped: 0,
    };
    store.save_record(AREA, "current", &head).unwrap();
    store
        .save_record(PENDING, &pending_key("p0"), &pending("p0"))
        .unwrap();
    store
        .save_record(BOUND, &bound_key(1, 0), &CostAttribution::default())
        .unwrap();
    prepare(&store, pending("new")).unwrap();
    let origin = TurnOrigin {
        kind: TurnOriginKind::Bridge,
        idempotency_key: Some("new".into()),
    };
    let bound = bind(&store, 2, 1, &origin, None).unwrap();
    let head = index(&store).unwrap();
    assert_eq!(head.pending.len(), PENDING_MAX - 1);
    assert_eq!(head.bound.len(), ATTRIBUTIONS_MAX);
    assert_eq!(head.dropped, 2);
    assert!(
        store
            .record::<PendingCause>(PENDING, &pending_key("p0"))
            .unwrap()
            .is_none()
    );
    assert!(attribution(&store, 1, 0).unwrap().is_none());
    assert_eq!(bound.kinds.len(), 4);
}

#[test]
fn binding_replay_is_idempotent_and_only_linked_continuations_inherit() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    prepare(&store, pending("pm:env1")).unwrap();
    prepare(&store, pending("pm:env1")).unwrap();
    assert_eq!(load(&store).unwrap().pending.len(), 1);
    let bridge = TurnOrigin {
        kind: TurnOriginKind::Bridge,
        idempotency_key: Some("pm:env1".into()),
    };
    let first = bind(&store, 1, 1, &bridge, None).unwrap();
    assert_eq!(bind(&store, 1, 1, &bridge, None).unwrap(), first);
    let continuation = TurnOrigin {
        kind: TurnOriginKind::Continuation,
        idempotency_key: Some("continue:P1:1".into()),
    };
    let inherited = bind(&store, 1, 2, &continuation, Some((1, 1))).unwrap();
    assert_eq!(inherited.task, first.task);
    assert_eq!(inherited.envelope_key, first.envelope_key);
    assert_eq!(
        bind(&store, 2, 1, &continuation, None).unwrap().class,
        CostClass::Unallocated
    );
    let records = load(&store).unwrap();
    assert!(records.pending.is_empty());
    assert_eq!(records.attributions.len(), 3);
    assert_eq!(records.dropped, 0);
}
