use super::*;
use crate::{DispatchOpts, resources::FixedResourceProbe};
use corral_core::{ProfileChoice, Store};
use std::path::Path;

fn fixture() -> (tempfile::TempDir, Ctx) {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("app/.git")).unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let mut ctx = FixedResourceProbe::ctx(store);
    // Any accidental engine call must fail locally, never reach a live session.
    ctx.herdr = corral_herdr::Herdr::with_bin(
        tmp.path().join("nonexistent-herdr"),
        Some("corral-test".into()),
    );
    (tmp, ctx)
}

fn tracker(ctx: &Ctx, status: Status, children: Vec<String>) -> Task {
    let task = ctx
        .store
        .create("Tracker", Some("app"), status, "## Acceptance\nOwner body")
        .unwrap();
    ctx.store
        .update(&task.id, |task| {
            task.tracker = true;
            task.children = children;
            Ok(())
        })
        .unwrap()
}

fn refuses<T: std::fmt::Debug>(result: Result<T>) {
    let error = result.unwrap_err().to_string();
    assert!(error.contains("tracker"), "{error}");
    assert!(error.contains("cannot"), "{error}");
}

#[test]
fn tracker_guards_refuse_before_engine_config_or_plan_file_access() {
    let (_tmp, ctx) = fixture();
    let task = tracker(&ctx, Status::Ready, vec![]);
    let before = ctx.store.get(&task.id).unwrap();
    let events = ctx.store.events().unwrap().len();
    for plan in [false, true] {
        let opts = DispatchOpts {
            plan,
            ..Default::default()
        };
        refuses(crate::dispatch(&ctx, &task.id, &opts));
        refuses(crate::enqueue(&ctx, &task.id, &opts));
        refuses(crate::routing::route_with_plan(
            &ctx,
            &task.id,
            &ProfileChoice::default(),
            plan,
        ));
        refuses(crate::routing::route_snapshot(
            &ctx,
            &task,
            &ProfileChoice::default(),
            plan,
        ));
        refuses(crate::plan_dispatch::dispatch(
            &ctx,
            &task,
            &opts,
            &crate::dispatch::Admission::Manual,
        ));
    }
    refuses(crate::plan_submit(
        &ctx,
        &task.id,
        Path::new("nonexistent-plan.md"),
    ));
    refuses(crate::approved_plan(&ctx, &task));
    assert_eq!(ctx.store.get(&task.id).unwrap(), before);
    assert_eq!(ctx.store.events().unwrap().len(), events);
}

#[test]
fn tracker_ready_and_backlog_admission_skip_without_mutation_or_failure_events() {
    let (_tmp, mut ctx) = fixture();
    ctx.store.config.dispatch.promote_ideas = corral_core::PromoteIdeas::Auto;
    ctx.store.save_config().unwrap();
    let ready = tracker(&ctx, Status::Ready, vec![]);
    let backlog = tracker(&ctx, Status::Backlog, vec![]);
    let _unqueued = tracker(&ctx, Status::Backlog, vec![]);
    let idea = tracker(&ctx, Status::Idea, vec![]);
    ctx.store
        .update(&backlog.id, |task| {
            task.queued = true;
            Ok(())
        })
        .unwrap();
    assert!(!crate::queue::authorized_ready(&ctx.store, &ready));
    let mut ordinary = ready.clone();
    ordinary.tracker = false;
    assert!(!crate::queue::authorized_ready(&ctx.store, &ordinary));
    assert!(
        crate::promotion::promotion_shortlist(&ctx)
            .unwrap()
            .iter()
            .any(|candidate| candidate.id == idea.id)
    );
    let before = ctx.store.list().unwrap();
    let events = ctx.store.events().unwrap().len();
    assert!(crate::queue::migrate_backlog(&ctx).unwrap().is_empty());
    assert!(
        crate::dispatch_policy::advance(&ctx, "2026-10-18T00:00:00Z".parse().unwrap())
            .unwrap()
            .is_empty()
    );
    assert_eq!(ctx.store.list().unwrap(), before);
    assert_eq!(ctx.store.events().unwrap().len(), events);
}

#[test]
fn tracker_closes_only_with_nonempty_existing_all_done_children() {
    for child_status in [
        Status::Idea,
        Status::Backlog,
        Status::Ready,
        Status::Running,
        Status::NeedsMe,
        Status::Review,
        Status::Dropped,
        Status::Done,
    ] {
        let (_tmp, ctx) = fixture();
        let done = ctx.store.create("Done", None, Status::Done, "").unwrap();
        let child = ctx.store.create("Child", None, child_status, "").unwrap();
        let task = tracker(&ctx, Status::Backlog, vec![done.id, child.id]);
        let before = ctx.store.get(&task.id).unwrap();
        let change = reconcile_one(&ctx, &task.id).unwrap();
        let saved = ctx.store.get(&task.id).unwrap();
        if child_status == Status::Done {
            let change = change.unwrap();
            assert_eq!(change.task, task.id);
            assert_eq!(change.from, Status::Backlog);
            assert_eq!(change.to, Status::Done);
            assert_eq!(change.stage, None);
            assert_eq!(saved.status, Status::Done);
            let events = ctx.store.events().unwrap();
            let completed = events
                .iter()
                .find(|e| e.kind == "tracker_completed")
                .unwrap();
            assert_eq!(completed.task.as_deref(), Some(task.id.as_str()));
            assert_eq!(
                completed.detail["children"],
                serde_json::json!(task.children)
            );
            assert!(reconcile_one(&ctx, &task.id).unwrap().is_none());
            assert_eq!(ctx.store.get(&task.id).unwrap(), saved);
            assert_eq!(ctx.store.events().unwrap().len(), events.len());
        } else {
            assert!(change.is_none());
            assert_eq!(saved, before);
        }
    }
    let (_tmp, ctx) = fixture();
    for children in [vec![], vec!["T-999".into()]] {
        let task = tracker(&ctx, Status::Ready, children);
        assert!(reconcile_one(&ctx, &task.id).unwrap().is_none());
        assert_eq!(ctx.store.get(&task.id).unwrap(), task);
    }
}

#[test]
fn terminal_and_ordinary_tasks_are_never_rewritten() {
    let (_tmp, ctx) = fixture();
    let child = ctx.store.create("Done", None, Status::Done, "").unwrap();
    for status in [Status::Done, Status::Dropped] {
        let task = tracker(&ctx, status, vec![child.id.clone()]);
        assert!(reconcile_one(&ctx, &task.id).unwrap().is_none());
        assert_eq!(ctx.store.get(&task.id).unwrap(), task);
    }
    let ordinary = ctx
        .store
        .create("Ordinary", None, Status::Ready, "")
        .unwrap();
    ctx.store
        .update(&ordinary.id, |task| {
            task.children = vec![child.id];
            Ok(())
        })
        .unwrap();
    let before = ctx.store.get(&ordinary.id).unwrap();
    assert!(reconcile_one(&ctx, &ordinary.id).unwrap().is_none());
    assert_eq!(ctx.store.get(&ordinary.id).unwrap(), before);
}

#[test]
fn closure_uses_fresh_children_and_preserves_owner_edits() {
    let (_tmp, ctx) = fixture();
    let child = ctx.store.create("Child", None, Status::Done, "").unwrap();
    let task = tracker(&ctx, Status::Ready, vec![child.id.clone()]);
    // Warm the reader cache, then let another store edit the child and owner text.
    assert!(ctx.store.list().unwrap().iter().any(|t| t.id == child.id));
    let owner = Store::open(ctx.store.root()).unwrap();
    owner
        .update(&child.id, |task| {
            task.status = Status::Dropped;
            Ok(())
        })
        .unwrap();
    owner
        .update(&task.id, |task| {
            task.title = "Owner renamed".into();
            task.body = "Concurrent owner notes".into();
            task.source_digest = Some("owner-digest".into());
            Ok(())
        })
        .unwrap();
    let edited = owner.get(&task.id).unwrap();
    assert!(reconcile_one(&ctx, &task.id).unwrap().is_none());
    assert_eq!(ctx.store.get(&task.id).unwrap(), edited);
    owner
        .update(&child.id, |task| {
            task.status = Status::Done;
            Ok(())
        })
        .unwrap();
    assert!(reconcile_one(&ctx, &task.id).unwrap().is_some());
    let saved = ctx.store.get(&task.id).unwrap();
    assert_eq!(saved.title, edited.title);
    assert_eq!(saved.body, edited.body);
    assert_eq!(saved.source_digest, edited.source_digest);
    assert_eq!(saved.children, edited.children);
}

#[test]
fn sync_completes_tracker_without_a_herdr_server() {
    let (_tmp, ctx) = fixture();
    let child = ctx.store.create("Child", None, Status::Done, "").unwrap();
    let task = tracker(&ctx, Status::Ready, vec![child.id]);
    let changes = crate::sync_at(&ctx, "2026-10-18T00:00:00Z".parse().unwrap()).unwrap();
    assert!(
        changes
            .iter()
            .any(|change| change.task == task.id && change.to == Status::Done)
    );
    assert_eq!(ctx.store.get(&task.id).unwrap().status, Status::Done);
}
