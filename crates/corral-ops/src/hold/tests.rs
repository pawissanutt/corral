use super::*;
use corral_core::{Status, Store};

#[test]
fn holds_preserve_every_open_status_and_audit_idempotently() {
    let root = tempfile::tempdir().unwrap();
    let ctx = Ctx::from_store(Store::init(root.path()).unwrap());
    for status in [
        Status::Idea,
        Status::Backlog,
        Status::Ready,
        Status::Running,
        Status::NeedsMe,
        Status::Review,
    ] {
        let task = ctx.store.create("held", Some("app"), status, "").unwrap();
        set(&ctx, &task.id, "urgent work", None).unwrap();
        set(&ctx, &task.id, "urgent work", None).unwrap();
        let head = ctx.store.head(&task.id).unwrap();
        assert_eq!(head.status, status);
        assert_eq!(head.hold.as_ref().unwrap().reason, "urgent work");
        assert_eq!(ctx.store.get(&task.id).unwrap().hold, head.hold);
        assert!(corral_core::TaskHold::require_clear(&head).is_err());
        release(&ctx, &task.id).unwrap();
        release(&ctx, &task.id).unwrap();
        let events = ctx
            .store
            .events_page(Some(&task.id), &["held", "released"], None, 0, 10)
            .unwrap();
        assert_eq!(
            events.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(),
            ["held", "released"]
        );
        assert_eq!(ctx.store.head(&task.id).unwrap().status, status);
    }
    for status in [Status::Done, Status::Dropped] {
        let task = ctx.store.create("terminal", None, status, "").unwrap();
        assert!(set(&ctx, &task.id, "no", None).is_err());
    }
}

#[test]
fn focus_reconciles_new_tasks_projects_clear_and_terminal_release() {
    let root = tempfile::tempdir().unwrap();
    let ctx = Ctx::from_store(Store::init(root.path()).unwrap());
    let urgent = ctx
        .store
        .create("urgent", Some("app"), Status::Running, "")
        .unwrap();
    ctx.store
        .update(&urgent.id, |t| {
            t.priority = "P0".into();
            Ok(())
        })
        .unwrap();
    let peer = ctx
        .store
        .create("peer", Some("app"), Status::Review, "")
        .unwrap();
    let manual = ctx
        .store
        .create("manual", Some("app"), Status::Backlog, "")
        .unwrap();
    set(&ctx, &manual.id, "manual", None).unwrap();
    let foreign = ctx
        .store
        .create("foreign", Some("other"), Status::Review, "")
        .unwrap();
    ctx.store
        .save_record(
            "task-focus",
            "current",
            &Focus {
                task: urgent.id.clone(),
                all: false,
                held: Vec::new(),
            },
        )
        .unwrap();
    reconcile_focus(&ctx, &crate::sync::SyncScope::All).unwrap();
    assert_eq!(
        ctx.store
            .head(&peer.id)
            .unwrap()
            .hold
            .as_ref()
            .unwrap()
            .focus
            .as_deref(),
        Some(urgent.id.as_str())
    );
    assert!(ctx.store.head(&foreign.id).unwrap().hold.is_none());
    let new = ctx
        .store
        .create("new", Some("app"), Status::Ready, "")
        .unwrap();
    assert!(
        crate::operating::require_origin(
            &ctx.store,
            &new,
            corral_core::OperatingActionKind::Gate,
            crate::operating::Origin::Owner
        )
        .is_err(),
        "new lower-priority work is fenced before the hold latch"
    );
    reconcile_focus(&ctx, &crate::sync::SyncScope::All).unwrap();
    assert!(ctx.store.head(&new.id).unwrap().hold.is_some());
    ctx.store
        .update(&urgent.id, |t| {
            t.status = Status::Done;
            Ok(())
        })
        .unwrap();
    reconcile_focus(&ctx, &crate::sync::SyncScope::All).unwrap();
    assert!(ctx.store.head(&peer.id).unwrap().hold.is_none());
    assert_eq!(
        ctx.store
            .head(&manual.id)
            .unwrap()
            .hold
            .as_ref()
            .unwrap()
            .reason,
        "manual"
    );
    let urgent2 = ctx
        .store
        .create("urgent2", Some("app"), Status::Ready, "")
        .unwrap();
    ctx.store
        .update(&urgent2.id, |t| {
            t.priority = "P0".into();
            Ok(())
        })
        .unwrap();
    ctx.store
        .save_record(
            "task-focus",
            "current",
            &Focus {
                task: urgent2.id.clone(),
                all: true,
                held: Vec::new(),
            },
        )
        .unwrap();
    reconcile_focus(&ctx, &crate::sync::SyncScope::All).unwrap();
    assert!(ctx.store.head(&foreign.id).unwrap().hold.is_some());
    clear_focus(&ctx).unwrap();
    assert!(ctx.store.head(&foreign.id).unwrap().hold.is_none());
    assert!(ctx.store.head(&manual.id).unwrap().hold.is_some());
}
