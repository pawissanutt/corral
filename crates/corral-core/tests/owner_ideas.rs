use corral_core::{Origin, Status, Store, parse_task};

fn fixture() -> (tempfile::TempDir, Store, String) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let task = store
        .create("Owner idea", None, Status::Idea, "Full scope\n")
        .unwrap();
    (dir, store, task.id)
}

#[test]
fn legacy_idea_retains_owner_origin_after_scheduling() {
    // Given a pre-origin task file.
    let (dir, store, id) = fixture();
    let task = store.get(&id).unwrap();
    let legacy = std::fs::read_to_string(&task.path)
        .unwrap()
        .replace("  origin: owner\n", "");
    std::fs::write(&task.path, legacy).unwrap();
    // When the inferred owner idea is scheduled.
    store
        .update(&id, |task| {
            task.status = Status::Ready;
            Ok(())
        })
        .unwrap();
    // Then a fresh reader retains ownership, and dropping still requires consent.
    let reopened = Store::open(dir.path()).unwrap();
    assert_eq!(reopened.get(&id).unwrap().origin, Some(Origin::Owner));
    assert!(
        reopened
            .update(&id, |task| {
                task.status = Status::Dropped;
                Ok(())
            })
            .is_err()
    );
}

#[test]
fn drop_is_refused_without_owner_decision() {
    // Given an owner idea.
    let (_dir, store, id) = fixture();
    // When a caller drops it without explicit consent.
    let result = store.update(&id, |task| {
        task.status = Status::Dropped;
        Ok(())
    });
    // Then the durable status is untouched.
    assert!(result.is_err());
    assert_eq!(store.get(&id).unwrap().status, Status::Idea);
}

#[test]
fn discard_cannot_bypass_protection_by_clearing_task_path() {
    // Given an owner idea copied out of the store.
    let (_dir, store, id) = fixture();
    let mut task = store.get(&id).unwrap();
    task.path.clear();
    task.status = Status::Dropped;
    // When the low-level save API is used.
    let result = store.save(&task);
    // Then it still checks the existing task identity.
    assert!(result.is_err());
    assert_eq!(store.get(&id).unwrap().status, Status::Idea);
}

#[test]
fn approved_drop_records_the_owner_reason_and_consumes_consent() {
    // Given explicit consent for this idea revision.
    let (_dir, store, id) = fixture();
    store
        .owner_decision(&id, "drop", "Replaced by a better approach", "flag")
        .unwrap();
    // When the idea is dropped.
    store
        .update(&id, |task| {
            task.status = Status::Dropped;
            Ok(())
        })
        .unwrap();
    // Then the audit carries the decision and consent cannot drop a reopened idea again.
    let events = store.events().unwrap();
    let audit = events.iter().find(|e| e.kind == "idea_decided").unwrap();
    assert_eq!(audit.detail["action"], "drop");
    assert_eq!(audit.detail["owner_ok"], true);
    assert_eq!(audit.detail["reason"], "Replaced by a better approach");
    store
        .update(&id, |task| {
            task.status = Status::Idea;
            Ok(())
        })
        .unwrap();
    assert!(
        store
            .update(&id, |task| {
                task.status = Status::Dropped;
                Ok(())
            })
            .is_err()
    );
}

#[test]
fn changed_spec_invalidates_old_approval() {
    // Given drop and edit approvals for the original specification.
    let (_dir, store, id) = fixture();
    store
        .owner_decision(&id, "drop", "Original scope no longer needed", "comment")
        .unwrap();
    store
        .owner_decision(&id, "narrow", "Revise the scope", "flag")
        .unwrap();
    store
        .update(&id, |task| {
            task.body = "Different scope\n".into();
            Ok(())
        })
        .unwrap();
    // When the old drop approval is used for the changed specification.
    let result = store.update(&id, |task| {
        task.status = Status::Dropped;
        Ok(())
    });
    // Then it cannot authorize that different idea.
    assert!(result.is_err());
    assert_eq!(store.get(&id).unwrap().status, Status::Idea);
}

#[test]
fn narrow_requires_its_own_consent_even_with_drop_permission() {
    // Given consent only to drop.
    let (_dir, store, id) = fixture();
    store
        .owner_decision(&id, "drop", "Not needed", "flag")
        .unwrap();
    // When a caller also replaces the specification.
    let result = store.update(&id, |task| {
        task.status = Status::Dropped;
        task.body = "Only half the request\n".into();
        Ok(())
    });
    // Then consent to one operation cannot authorize the other.
    assert!(result.is_err());
    assert_eq!(store.get(&id).unwrap().body, "Full scope\n");
}

#[test]
fn owner_origin_cannot_be_laundered() {
    // Given an owner idea.
    let (_dir, store, id) = fixture();
    // When a caller changes attribution before dropping it.
    let result = store.update(&id, |task| {
        task.origin = Some(Origin::Agent);
        task.status = Status::Dropped;
        Ok(())
    });
    // Then the origin and status remain protected.
    assert!(result.is_err());
    assert_eq!(store.get(&id).unwrap().effective_origin(), Origin::Owner);
}

#[test]
fn non_owner_work_can_be_dropped_without_owner_consent() {
    // Given explicitly agent-filed work.
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let task = store
        .create_with_origin("Agent task", None, Status::Idea, "", Origin::Agent)
        .unwrap();
    // When it is dropped.
    let result = store
        .update(&task.id, |task| {
            task.status = Status::Dropped;
            Ok(())
        })
        .unwrap();
    // Then no owner decision is needed.
    assert_eq!(result.status, Status::Dropped);
}

#[test]
fn legacy_non_idea_is_not_reclassified_as_owner() {
    // Given a historical backlog record without origin.
    let text = "---\nid: T-001\ntitle: Legacy\nstatus: backlog\ncreated: 2026-10-01T00:00:00Z\nupdated: 2026-10-01T00:00:00Z\n---\n";
    // When parsed.
    let task = parse_task(text, std::path::Path::new("legacy.md")).unwrap();
    // Then only historical Idea records receive owner inference.
    assert_eq!(task.effective_origin(), Origin::Agent);
}
