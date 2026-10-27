use corral_core::work::{self, Counter, HotScope, WorkSnapshot};
use corral_core::{ApprovedScope, PlanDeliveryTarget, Status, Store};
use corral_ops::{Ctx, PlanProgress};
use serde_json::{Value, json};

fn fixture() -> (tempfile::TempDir, Ctx, String, Vec<String>) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    let mut tracker = store.create("Plan", None, Status::Backlog, "").unwrap();
    tracker.created = "2026-10-27T00:00:00Z".parse().unwrap();
    tracker.updated = tracker.created;
    tracker.tracker = true;
    tracker.approved_scope = Some(ApprovedScope {
        goal: "Deliver members".into(),
        acceptance: vec![],
        delivery_target: PlanDeliveryTarget::Main,
        approved_design: None,
    });
    let mut ids = Vec::new();
    for status in [
        Status::Done,
        Status::Running,
        Status::Review,
        Status::NeedsMe,
        Status::Dropped,
    ] {
        let mut child = store.create("Member", None, status, "").unwrap();
        child.created = tracker.created;
        child.updated = tracker.updated;
        child.parent = Some(tracker.id.clone());
        ids.push(child.id.clone());
        store.fixture_put_task(&child).unwrap();
    }
    tracker.children = ids.clone();
    store.fixture_put_task(&tracker).unwrap();
    (
        root,
        corral_ops::resources::FixedResourceProbe::ctx(store),
        tracker.id,
        ids,
    )
}

// The pre-head, full-task algorithm for this fixture's dependency-free members.
fn oracle(ctx: &Ctx, id: &str) -> Value {
    let tracker = ctx.store.get(id).unwrap();
    let mut counts = [0; 5];
    let work: Vec<_> = tracker
        .children
        .iter()
        .map(|id| {
            let child = ctx.store.get(id).unwrap();
            assert_eq!(
                ctx.store.execution_plan(&child).unwrap().unwrap().id,
                tracker.id
            );
            let (bucket, state, reason) = match child.status {
                Status::Done => (0, "accepted", None),
                Status::Running => (1, "running", None),
                Status::Review => (2, "review", None),
                Status::NeedsMe => (3, "waiting", Some("owner input required")),
                Status::Dropped => (
                    4,
                    "missing",
                    Some("required work was dropped; approved scope is unchanged"),
                ),
                _ => panic!("fixture has no queued members"),
            };
            counts[bucket] += 1;
            json!({
                "task": child.id, "title": child.title, "status": child.status,
                "state": state, "run": child.current_run().map(|run| &run.id),
                "reason": reason, "prerequisites": []
            })
        })
        .collect();
    json!({
        "task": tracker.id, "status": tracker.status, "approved_scope": tracker.approved_scope,
        "scope_revision": tracker.approved_scope_revision,
        "counts": {"accepted": counts[0], "running": counts[1], "review": counts[2],
                   "waiting": counts[3], "missing": counts[4]},
        "work": work, "coverage": [], "unmapped_criteria": []
    })
}

fn measure(ctx: &Ctx, id: &str) -> (PlanProgress, WorkSnapshot) {
    let before = work::snapshot();
    let progress = {
        let _hot = HotScope::enter();
        corral_ops::plan_progress(ctx, id).unwrap()
    };
    let report = work::snapshot().since(&before);
    assert_eq!(report.unattributed_hydrations(), 0);
    report.check_identity().unwrap();
    (progress, report)
}

#[test]
fn progress_uses_supplied_snapshot_after_member_writer() {
    for fake in [false, true] {
        let mut reports = Vec::new();
        for factor in [1, 10] {
            // Given an approved Plan frozen before a writer changes its Done member.
            let (_root, mut ctx, id, members) = fixture();
            if fake {
                let tasks = ctx.store.list().unwrap();
                ctx.store = ctx
                    .store
                    .with_backend(corral_core::backend::conformance::fake_fixture().backend);
                for task in tasks {
                    ctx.store.fixture_put_task(&task).unwrap();
                }
            }
            for _ in 0..factor * 10 {
                ctx.store.create("Old", None, Status::Done, "").unwrap();
                ctx.store
                    .log_event(Some(&members[0]), "history", json!({}))
                    .unwrap();
            }
            let expected = oracle(&ctx, &id);
            let frozen = Ctx {
                store: ctx.store.read_only_snapshot().unwrap(),
                ..ctx.clone()
            };
            let writer = if fake {
                ctx.store.clone()
            } else {
                Store::open(ctx.store.root()).unwrap()
            };
            writer
                .update(&members[0], |task| {
                    task.status = Status::Dropped;
                    Ok(())
                })
                .unwrap();
            // Populate the process cache with a newer state before reading the older view.
            let current = Ctx {
                store: ctx.store.read_only_snapshot().unwrap(),
                ..ctx.clone()
            };
            let (new, _) = measure(&current, &id);
            assert_eq!(new.counts.missing, 2);

            // When progress reads the supplied frame view.
            let (old, cells) = measure(&frozen, &id);

            // Then the entire output equals the full-task pre-writer result on both backends.
            assert_eq!(serde_json::to_value(old).unwrap(), expected);
            assert_eq!(cells.get(Counter::ViewsOpened), 0);
            assert_eq!(frozen.store.head(&members[0]).unwrap().status, Status::Done);
            reports.push(cells);
        }
        for counter in Counter::ALL {
            assert_eq!(
                reports[0].get(counter),
                reports[1].get(counter),
                "{fake} {counter:?}"
            );
        }
    }
}

#[test]
fn unrelated_records_do_not_recompute_tracker_projection() {
    let mut reports = Vec::new();
    for factor in [1, 10] {
        // Given a warm tracker with tenfold unrelated terminal/history growth.
        let (_root, ctx, id, members) = fixture();
        for _ in 0..factor * 10 {
            ctx.store
                .create("Terminal", None, Status::Done, "")
                .unwrap();
            ctx.store
                .log_event(Some(&members[0]), "old", json!({}))
                .unwrap();
        }
        let expected = oracle(&ctx, &id);
        measure(&ctx, &id);
        ctx.store
            .save_record("resource-observations", "latest", &json!({"load": 1}))
            .unwrap();

        // When an unrelated record is the only change since the derivation.
        let (progress, cells) = measure(&ctx, &id);

        // Then the task computation is reused, not merely rehydrated more cheaply.
        assert_eq!(serde_json::to_value(progress).unwrap(), expected);
        assert_eq!(cells.get(Counter::TaskHeadDecodes), 0);
        assert_eq!(cells.get(Counter::ViewsOpened), 1);
        reports.push(cells);
        ctx.store
            .update(&members[0], |task| {
                task.status = Status::Dropped;
                Ok(())
            })
            .unwrap();
        let (changed, _) = measure(&ctx, &id);
        assert_eq!(changed.counts.missing, 2);
    }
    for counter in Counter::ALL {
        assert_eq!(
            reports[0].get(counter),
            reports[1].get(counter),
            "{counter:?}"
        );
    }
}

#[test]
fn unrelated_live_config_changes_do_not_recompute_task_projection() {
    let mut reports = Vec::new();
    for factor in [1, 10] {
        // Given an unchanged task projection and a saved UI-only configuration change.
        let (_root, ctx, id, _) = fixture();
        for _ in 0..10 * factor {
            ctx.store.create("Old", None, Status::Done, "").unwrap();
        }
        let expected = oracle(&ctx, &id);
        measure(&ctx, &id);
        let mut writer = ctx.store.reopen().unwrap();
        writer.config.ui.done_max = 17;
        writer.save_config().unwrap();

        // When a live caller reloads config without any task-scope change.
        let (progress, cells) = measure(&ctx, &id);

        // Then the task-only derivation survives the fresh configuration read.
        assert_eq!(serde_json::to_value(progress).unwrap(), expected);
        assert_eq!(cells.get(Counter::TaskHeadDecodes), 0);
        assert_eq!(cells.get(Counter::ViewsOpened), 1);
        reports.push(cells);
    }
    for counter in Counter::ALL {
        assert_eq!(
            reports[0].get(counter),
            reports[1].get(counter),
            "{counter:?}"
        );
    }
}

#[test]
fn snapshot_progress_retains_dispatch_lease_refusal() {
    // Given a snapshot whose board dispatch lease is held by a writer.
    let (_root, ctx, id, _) = fixture();
    let frozen = Ctx {
        store: ctx.store.read_only_snapshot().unwrap(),
        ..ctx.clone()
    };
    let _lease = ctx.store.try_lock("dispatch").unwrap().unwrap();

    // When progress tries to read that snapshot.
    let error = corral_ops::plan_progress(&frozen, &id).unwrap_err();

    // Then it retains the existing externally visible refusal.
    assert_eq!(
        error.to_string(),
        "board is being updated; retry Plan progress"
    );
}

#[test]
fn progress_recovers_from_a_pruned_feed_using_full_head_computation() {
    let mut reports = Vec::new();
    for factor in [1, 10] {
        // Given a derived tracker whose member mutation is pruned out of the feed.
        let (_root, ctx, id, members) = fixture();
        for _ in 0..10 * factor {
            ctx.store.create("Old", None, Status::Done, "").unwrap();
        }
        measure(&ctx, &id);
        let mut changed = ctx.store.get(&members[0]).unwrap();
        changed.status = Status::Dropped;
        ctx.store.fixture_put_task(&changed).unwrap();
        let backend = corral_core::sqlite::SqliteBackend::open(
            ctx.store.dir(),
            corral_core::sqlite::SqliteOptions::default(),
        )
        .unwrap();
        let seq = ctx.store.view().unwrap().seq();
        backend.prune_changes_through(seq).unwrap();
        let expected = oracle(&ctx, &id);

        // When progress encounters the gap rather than the member's change entry.
        let (progress, cells) = measure(&ctx, &id);

        // Then it rebuilds from this view and agrees with the old full-task output.
        assert_eq!(serde_json::to_value(progress).unwrap(), expected);
        assert!(cells.get(Counter::TaskHeadDecodes) > 0);
        assert_eq!(cells.get(Counter::ViewsOpened), 1);
        reports.push(cells);
    }
    for counter in Counter::ALL {
        assert_eq!(
            reports[0].get(counter),
            reports[1].get(counter),
            "{counter:?}"
        );
    }
}
