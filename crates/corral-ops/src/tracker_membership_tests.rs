use super::*;
use corral_core::work::{self, Counter, HotScope, WorkSnapshot};

const AT: jiff::Timestamp = jiff::Timestamp::constant(1_767_225_600, 0);

fn fixture(factor: usize) -> (tempfile::TempDir, Ctx, Task, Task) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let ctx = Ctx::from_store(store);
    let mut plan = ctx
        .store
        .create("Plan", Some("app"), Status::Ready, "")
        .unwrap();
    let mut member = ctx
        .store
        .create("Member", Some("app"), Status::Done, "")
        .unwrap();
    member.parent = Some(plan.id.clone());
    member.created = AT;
    member.updated = AT;
    for _ in 0..factor {
        member.comments.push(
            serde_json::from_value(json!({"author": "owner", "at": AT, "text": "Old comment"}))
                .unwrap(),
        );
    }
    ctx.store.fixture_put_task(&member).unwrap();
    plan.tracker = true;
    plan.children = vec![member.id.clone()];
    plan.created = AT;
    plan.updated = AT;
    plan.approved_scope = Some(ApprovedScope {
        goal: "Deliver member".into(),
        acceptance: vec![],
        delivery_target: PlanDeliveryTarget::Main,
        approved_design: None,
    });
    ctx.store.fixture_put_task(&plan).unwrap();
    for _ in 0..10 * factor {
        let mut old = ctx
            .store
            .create("Old", Some("app"), Status::Done, "")
            .unwrap();
        old.created = AT;
        old.updated = AT;
        ctx.store.fixture_put_task(&old).unwrap();
    }
    (dir, ctx, plan, member)
}

/// The db316644 full-task algorithm for this single Done member with no criteria.
fn oracle(ctx: &Ctx, plan: &Task, member: &Task) -> Result<serde_json::Value> {
    let plan = ctx.store.get(&plan.id)?;
    let member = ctx.store.get(&member.id)?;
    ensure!(
        ctx.store.execution_plan(&member)?.as_ref().map(|p| &p.id) == Some(&plan.id),
        "execution Plan membership changed"
    );
    Ok(json!({
        "task": plan.id, "status": plan.status, "approved_scope": plan.approved_scope,
        "scope_revision": plan.approved_scope_revision,
        "counts": {"accepted": 1, "running": 0, "review": 0, "waiting": 0, "missing": 0},
        "work": [{
            "task": member.id, "title": member.title, "status": member.status,
            "state": "accepted", "run": null, "reason": null, "prerequisites": []
        }],
        "coverage": [], "unmapped_criteria": []
    }))
}

fn compare_budgets(reports: &[WorkSnapshot]) {
    for report in reports {
        assert_eq!(report.unattributed_hydrations(), 0);
        report.check_identity().unwrap();
    }
    for counter in Counter::ALL {
        assert_eq!(
            reports[0].get(counter),
            reports[1].get(counter),
            "{counter:?}"
        );
    }
}

fn introduce(ctx: &Ctx, parent: Option<&str>, child: Option<&str>) -> Task {
    let new = ctx
        .store
        .create("New", Some("app"), Status::Done, "")
        .unwrap();
    // Use the ordinary update API that exposed the reviewer's conflicting owner.
    let mut new = ctx
        .store
        .update(&new.id, |task| {
            task.parent = parent.map(str::to_owned);
            task.tracker = child.is_some();
            task.children = child.into_iter().map(str::to_owned).collect();
            Ok(())
        })
        .unwrap();
    // Keep counted document widths independent of wall-clock fractional precision.
    new.created = AT;
    new.updated = AT;
    ctx.store.fixture_put_task(&new).unwrap();
    new
}

fn refuses_new_edge(parent: bool, owns_plan: bool) {
    let mut reports = Vec::new();
    for factor in [1, 10] {
        // Given a cached, valid approved Plan and tenfold unrelated history.
        let (_dir, ctx, plan, member) = fixture(factor);
        assert_eq!(
            serde_json::to_value(plan_progress(&ctx, &plan.id).unwrap()).unwrap(),
            oracle(&ctx, &plan, &member).unwrap()
        );
        let _new = introduce(
            &ctx,
            parent.then_some(plan.id.as_str()),
            (!parent).then_some(if owns_plan { &plan.id } else { &member.id }),
        );
        let expected = format!("{:#}", oracle(&ctx, &plan, &member).unwrap_err());
        let oracle_ctx = Ctx {
            store: ctx.store.read_only_snapshot().unwrap(),
            ..ctx.clone()
        };
        // A fresh, uncached projection refuses exactly the same membership.
        assert_eq!(
            format!("{:#}", compute(&oracle_ctx, &plan.id).err().unwrap()),
            expected
        );
        // Do not prime the measured evaluation's membership memo with the oracle.
        let snapshot = Ctx {
            store: ctx.store.read_only_snapshot().unwrap(),
            ..ctx.clone()
        };
        let before = work::snapshot();

        // When the public entry point follows changes from a previously unknown task.
        let actual = {
            let _hot = HotScope::enter();
            plan_progress(&snapshot, &plan.id)
        };

        // Then it refuses just like the full-task and uncached computations.
        reports.push(work::snapshot().since(&before));
        assert_eq!(format!("{:#}", actual.unwrap_err()), expected);
    }
    compare_budgets(&reports);
}

#[test]
fn cached_progress_refuses_new_competing_member_owner() {
    refuses_new_edge(false, false);
}

#[test]
fn cached_progress_refuses_new_unlisted_reciprocal_child() {
    refuses_new_edge(true, false);
}

#[test]
fn cached_progress_refuses_new_nonreciprocal_plan_owner() {
    refuses_new_edge(false, true);
}

#[test]
fn cached_progress_recovers_after_conflicting_membership_edge_removal() {
    for parent in [false, true] {
        let mut reports = Vec::new();
        for factor in [1, 10] {
            // Given an older successful cache and a newly introduced invalid edge.
            let (_dir, ctx, plan, member) = fixture(factor);
            let expected = oracle(&ctx, &plan, &member).unwrap();
            plan_progress(&ctx, &plan.id).unwrap();
            let new = introduce(
                &ctx,
                parent.then_some(plan.id.as_str()),
                (!parent).then_some(member.id.as_str()),
            );
            assert_eq!(
                format!("{:#}", plan_progress(&ctx, &plan.id).unwrap_err()),
                format!("{:#}", oracle(&ctx, &plan, &member).unwrap_err())
            );
            let mut new = ctx
                .store
                .update(&new.id, |task| {
                    task.children.clear();
                    task.parent = None;
                    Ok(())
                })
                .unwrap();
            new.updated = AT;
            ctx.store.fixture_put_task(&new).unwrap();
            let oracle_ctx = Ctx {
                store: ctx.store.read_only_snapshot().unwrap(),
                ..ctx.clone()
            };
            compute(&oracle_ctx, &plan.id).unwrap();
            let snapshot = Ctx {
                store: ctx.store.read_only_snapshot().unwrap(),
                ..ctx.clone()
            };
            let before = work::snapshot();

            // When only the conflicting edge is removed, without touching known members.
            let actual = {
                let _hot = HotScope::enter();
                plan_progress(&snapshot, &plan.id).unwrap()
            };

            // Then the old valid result is restored, exactly matching fresh computation.
            assert_eq!(serde_json::to_value(actual).unwrap(), expected);
            assert_eq!(oracle(&ctx, &plan, &member).unwrap(), expected);
            reports.push(work::snapshot().since(&before));
        }
        compare_budgets(&reports);
    }
}

#[test]
fn cached_progress_reuses_projection_after_unrelated_record_write() {
    let mut reports = Vec::new();
    for factor in [1, 10] {
        // Given a warmed approved Plan at both history sizes.
        let (_dir, ctx, plan, member) = fixture(factor);
        let expected = oracle(&ctx, &plan, &member).unwrap();
        plan_progress(&ctx, &plan.id).unwrap();
        ctx.store
            .save_record("resource-observations", "latest", &json!({"load": 1}))
            .unwrap();
        let snapshot = Ctx {
            store: ctx.store.read_only_snapshot().unwrap(),
            ..ctx.clone()
        };
        let before = work::snapshot();

        // When a genuinely unrelated record is the only feed change.
        let actual = {
            let _hot = HotScope::enter();
            plan_progress(&snapshot, &plan.id).unwrap()
        };

        // Then it retains exact output without recomputing or opening a new view.
        assert_eq!(serde_json::to_value(actual).unwrap(), expected);
        let report = work::snapshot().since(&before);
        assert_eq!(report.get(Counter::TaskHeadDecodes), 0);
        assert_eq!(report.get(Counter::ViewsOpened), 0);
        reports.push(report);
    }
    compare_budgets(&reports);
}
