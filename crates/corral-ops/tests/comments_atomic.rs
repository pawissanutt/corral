use corral_core::{CommentAuthor, Status, Store};
use corral_ops::{Ctx, comment};

#[test]
fn approval_comment_and_audits_commit_as_one_operation() {
    // Given a task and a client that cannot accidentally contact the owner's server.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let task = store.create("Comment", None, Status::Backlog, "").unwrap();
    let mut ctx = Ctx::from_store(store);
    ctx.herdr = corral_herdr::Herdr::with_bin(tmp.path().join("unexpected-herdr"), None);
    let before = ctx.store.view().unwrap().seq();

    // When the owner records a comment carrying explicit approval.
    let saved = comment(
        &ctx,
        &task.id,
        "owner-ok: defer Wait for the dependency.",
        CommentAuthor::Owner,
        false,
    )
    .unwrap();

    // Then the comment and both audit events belong to the same committed operation.
    let store = Store::open(tmp.path()).unwrap();
    let changes = store.changes_since(before, 10).unwrap();
    assert_eq!(changes.entries.len(), 1);
    let scopes = &changes.entries[0].scopes;
    assert!(
        scopes
            .iter()
            .any(|scope| matches!(scope, corral_core::backend::ChangeScope::Task { .. }))
    );
    for expected in ["commented", "owner_decision"] {
        assert!(scopes.iter().any(|scope| matches!(
            scope,
            corral_core::backend::ChangeScope::Events { task: id, kind }
                if id.as_deref() == Some(task.id.as_str()) && kind == expected
        )));
    }
    assert_eq!(store.get(&task.id).unwrap().comments, saved.comments);
    let approval = store
        .events_for(&task.id)
        .unwrap()
        .into_iter()
        .find(|event| event.kind == "owner_decision")
        .unwrap();
    assert_eq!(approval.detail["action"], "defer");
    assert_eq!(
        approval.detail["revision"],
        corral_core::owner_revision(&saved).unwrap()
    );
}
