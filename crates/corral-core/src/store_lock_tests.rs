use super::Store;

#[test]
fn completed_store_lock_does_not_leave_peer_changing() {
    // Given a duplicated open-file description, as retained across a concurrent fork.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let lock = store.lock("T-002").unwrap();
    let inherited = lock.0.try_clone().unwrap();
    assert!(store.try_lock("T-002").unwrap().is_none());

    // When the store operation finishes while the duplicate descriptor remains open.
    drop(lock);

    // Then a landing check sees the peer as settled, without waiting for the duplicate.
    let next = store.try_lock("T-002").unwrap();
    assert!(
        next.is_some(),
        "completed peer still appears to be changing"
    );
    drop(inherited);
}

#[test]
fn active_store_lock_still_blocks_peer_landing() {
    // Given an active store operation and a duplicate descriptor.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let lock = store.lock("T-002").unwrap();
    let inherited = lock.0.try_clone().unwrap();

    // When only the duplicate closes, while the operation still owns its lock.
    drop(inherited);

    // Then landing remains excluded until the actual operation finishes.
    assert!(store.try_lock("T-002").unwrap().is_none());
    drop(lock);
}
#[test]
fn dropping_owner_unlocks_even_when_child_keeps_an_inherited_descriptor() {
    // Given a descriptor duplicated as it would be across fork before exec.
    let root = tempfile::tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    let guard = store.try_lock("launch-test").unwrap().unwrap();
    let inherited = guard.0.try_clone().unwrap();
    assert!(store.try_lock("launch-test").unwrap().is_none());

    // When the parent finishes normally, the child may still hold its descriptor.
    drop(guard);

    // Then the next owner does not depend on the child's scheduling/exec timing.
    let next = store.try_lock("launch-test").unwrap();
    assert!(next.is_some());
    drop(inherited);
}
