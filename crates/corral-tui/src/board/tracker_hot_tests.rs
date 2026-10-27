use super::tests::seeded;
use super::*;
use corral_core::work::{self, Counter, HotScope};
use corral_core::{ApprovedScope, PlanAcceptance, PlanDeliveryTarget, Store};
use serde_json::json;

const PLAN: &str = "T-002";
const MEMBER: &str = "T-004";

fn at(seconds: i64) -> jiff::Timestamp {
    jiff::Timestamp::constant(1_767_225_600, 0)
        .checked_add(jiff::SignedDuration::from_secs(seconds))
        .unwrap()
}

fn grow(member: &mut Task, factor: usize) {
    let mut current = std::mem::take(&mut member.runs);
    for i in 0..2 * factor {
        let id = format!("old{i:04}");
        let mut run = super::tests::run(&id, "worker");
        run.started = at(0);
        run.ended = Some(at(1));
        run.outcome = Some(corral_core::RunOutcome::Done);
        run.review = Some(
            serde_json::from_value(json!({
                "reviewer": "rev", "verdict": "approve", "sha": "abc", "at": at(1)
            }))
            .unwrap(),
        );
        if let Some(active) = current.last_mut() {
            active.review_runs.push(run.clone());
        }
        member.runs.push(run);
        member.comments.push(
            serde_json::from_value(json!({
                "author": "owner", "at": at(0), "text": format!("old comment {i:04}")
            }))
            .unwrap(),
        );
        member.artifacts.push(
            serde_json::from_value(json!({
                "run": id, "kind": "log", "title": "Old log",
                "path": format!("artifacts/{}/{id}/log.txt", member.id),
                "source": "/tmp/log.txt", "sha256": "0".repeat(64), "bytes": 1, "added": at(0)
            }))
            .unwrap(),
        );
        member.plan_runs.push(
            serde_json::from_value(json!({
                "id": id, "profile": "planner", "harness": "fixture", "spec_revision": "spec",
                "started": at(0), "ended": at(1), "output_dir": "/tmp/plans",
                "rounds": 0, "state": "planning"
            }))
            .unwrap(),
        );
        let reviews = (0..2 * factor)
            .map(|r| {
                json!({
                    "id": format!("{id}-PR{r:04}"), "reviewer": "rev", "policy_digest": "policy",
                    "verdict": "approve", "at": at(1)
                })
            })
            .collect::<Vec<_>>();
        member.plans.push(serde_json::from_value(json!({
            "id": id, "spec_revision": "spec",
            "artifact": {
                "run": id, "kind": "plan", "title": "Old plan",
                "path": format!("artifacts/{}/{id}/plan.md", member.id), "source": "/tmp/plan.md",
                "sha256": "0".repeat(64), "bytes": 1, "added": at(0)
            },
            "document": {
                "goal": "Old goal", "non_goals": [], "decisions": [], "affected_paths": [],
                "contracts": [], "dependencies": [], "acceptance": [], "verification": [],
                "owner_decisions": []
            },
            "reviews": reviews
        })).unwrap());
        member.aggregate_attempts.push(serde_json::from_value(json!({
            "id": id, "target": {"kind": "main"}, "code": false, "members": [], "started": at(0),
            "identity": {
                "version": 2, "task": member.id, "run": "aggregate", "attempt": id,
                "head": "1".repeat(40), "tree": "2".repeat(40),
                "integration_base": "0".repeat(40), "dispatch_base": "0".repeat(40),
                "spec_digest": "s", "policy_digest": "p", "inputs_digest": "i"
            }
        })).unwrap());
        member.approved_scope_history.push(
            serde_json::from_value(json!({
                "revision": i, "approved_scope": null, "superseded_at": at(0)
            }))
            .unwrap(),
        );
    }
    member.runs.extend(current);
}

fn fixture(factor: usize, aggregate: bool) -> super::tests::Fixture {
    let mut fx = seeded();
    fx.ctx.store.config.ui.done_max = 0;
    // The fixture needs no checkout or live herdr server. Aggregate capture reaches
    // its member reads, then reports the same missing Git target as the full oracle.
    fx.ctx.store.config.projects.insert(
        "corral".into(),
        serde_json::from_value(json!({"path": fx.ctx.store.root()})).unwrap(),
    );
    let mut member = fx.ctx.store.get(MEMBER).unwrap();
    grow(&mut member, factor);
    member.parent = Some(PLAN.into());
    member.project = Some("corral".into());
    member.depends_on.clear();
    fx.ctx.store.fixture_put_task(&member).unwrap();
    let mut tracker = fx.ctx.store.get(PLAN).unwrap();
    tracker.tracker = true;
    tracker.project = Some("corral".into());
    tracker.children = vec![MEMBER.into()];
    tracker.approved_scope = Some(ApprovedScope {
        goal: "Deliver member".into(),
        acceptance: if aggregate {
            vec![PlanAcceptance {
                criterion: Some("C1".into()),
                scenario: "Combine".into(),
                expected: "Works".into(),
            }]
        } else {
            vec![]
        },
        delivery_target: PlanDeliveryTarget::Main,
        approved_design: None,
    });
    fx.ctx.store.fixture_put_task(&tracker).unwrap();
    for _ in 0..10 * factor {
        let mut old = fx.ctx.store.create("Old", None, Status::Done, "").unwrap();
        old.updated = at(-100);
        fx.ctx.store.fixture_put_task(&old).unwrap();
    }
    fx
}

#[test]
fn approved_plan_frame_budget_is_independent_of_terminal_and_member_history() {
    let mut reports = Vec::new();
    let mut projections = Vec::new();
    for factor in [1, 10] {
        // Given a cold approved Plan with tenfold terminal and member history growth.
        let fx = fixture(factor, true);
        let full = fx.ctx.store.get(MEMBER).unwrap();
        let tracker = fx.ctx.store.get(PLAN).unwrap();
        let acceptance = tracker.approved_scope.as_ref().unwrap().acceptance.clone();
        let error = corral_git::rev_sha(fx.ctx.store.root(), "refs/heads/main").unwrap_err();
        // db316644's full-task algorithm for this dependency-free Running member.
        let expected = json!({
            "task": tracker.id,
            "status": tracker.status,
            "approved_scope": tracker.approved_scope,
            "scope_revision": tracker.approved_scope_revision,
            "counts": {"accepted": 0, "running": 1, "review": 0, "waiting": 0, "missing": 0},
            "work": [{
                "task": full.id, "title": full.title, "status": full.status,
                "state": "running", "run": full.current_run().map(|run| &run.id),
                "reason": null, "prerequisites": []
            }],
            "coverage": acceptance.iter().map(|criterion| json!({
                "acceptance": criterion, "tasks": []
            })).collect::<Vec<_>>(),
            "unmapped_criteria": acceptance,
            "aggregate": {
                "ready": false, "blockers": [format!("{error:#}")],
                "attempt": null, "accepted": false, "acceptance": null
            }
        });
        let before = work::snapshot();

        // When the production board frame loads without any test-only preparation.
        let frame = BoardSnapshot::load(&fx.ctx).unwrap();
        let report = work::snapshot().since(&before);
        let mut board = fx.board();
        board.apply_snapshot(frame);
        let progress = board.plan_progress[PLAN].as_ref().unwrap();

        // Then the old full-task progress buckets and current run agree exactly.
        assert_eq!(
            progress.counts.running,
            usize::from(full.status == Status::Running)
        );
        assert_eq!(progress.work[0].status, full.status);
        assert_eq!(
            progress.work[0].run.as_deref(),
            full.current_run().map(|run| run.id.as_str())
        );
        assert!(progress.aggregate.is_some());
        assert_eq!(serde_json::to_value(progress).unwrap(), expected);
        assert_eq!(report.unattributed_hydrations(), 0);
        assert_eq!(report.get(Counter::ViewsOpened), 1);
        report.check_identity().unwrap();
        reports.push(report);
        projections.push(serde_json::to_value(progress).unwrap());
    }
    // Paths differ across fixtures only in error context; the task projection is identical.
    for projection in &mut projections {
        projection.as_object_mut().unwrap().remove("aggregate");
    }
    assert_eq!(projections[0], projections[1]);
    for counter in Counter::ALL {
        assert_eq!(
            reports[0].get(counter),
            reports[1].get(counter),
            "{counter:?}"
        );
    }
}

#[test]
fn frame_tracker_load_keeps_member_from_supplied_view_after_writer() {
    // Given a frame view that has read its open heads before a writer changes a member.
    let fx = fixture(1, false);
    fx.ctx
        .store
        .update(MEMBER, |task| {
            task.status = Status::Done;
            Ok(())
        })
        .unwrap();
    let expected = corral_ops::plan_progress(&fx.ctx, PLAN).unwrap();
    let before = work::snapshot();
    let (ctx, sources) = {
        let _hot = HotScope::enter();
        let ctx = Ctx {
            store: fx.ctx.store.read_only_snapshot().unwrap(),
            ..fx.ctx.clone()
        };
        let sources = sources::FrameSources::load(&ctx).unwrap();
        (ctx, sources)
    };
    let writer = Store::open_unobserved(fx.ctx.store.root()).unwrap();
    writer
        .update(MEMBER, |task| {
            task.status = Status::Dropped;
            Ok(())
        })
        .unwrap();
    let progress = {
        let _hot = HotScope::enter();
        // When the same component used by BoardSnapshot loads tracker progress.
        let task = sources.tasks.iter().find(|task| task.id == PLAN).unwrap();
        crate::plan::tracker::load(&ctx, task).unwrap().unwrap()
    };
    // Then the complete frame projection remains at its original committed state.
    assert_eq!(
        serde_json::to_value(progress).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
    let report = work::snapshot().since(&before);
    assert_eq!(report.unattributed_hydrations(), 0);
    assert_eq!(report.get(Counter::ViewsOpened), 1);
    assert_eq!(fx.ctx.store.head(MEMBER).unwrap().status, Status::Dropped);
    report.check_identity().unwrap();
}

#[test]
fn frame_tracker_reuses_projection_after_resource_observation() {
    // Given the production frame has already derived this approved Plan.
    let fx = fixture(1, false);
    BoardSnapshot::load(&fx.ctx).unwrap();
    fx.ctx
        .store
        .save_record("resource-observations", "latest", &json!({"load": 1}))
        .unwrap();
    let ctx = Ctx {
        store: fx.ctx.store.read_only_snapshot().unwrap(),
        ..fx.ctx.clone()
    };
    let tracker = ctx.store.head(PLAN).unwrap().to_shell_task().unwrap();
    let before = work::snapshot();

    // When the frame's tracker component sees only an unrelated resource write.
    {
        let _hot = HotScope::enter();
        crate::plan::tracker::load(&ctx, &tracker).unwrap().unwrap();
    }

    // Then it does not recompute any task inputs or open another view.
    let report = work::snapshot().since(&before);
    assert_eq!(report.get(Counter::TaskHeadDecodes), 0);
    assert_eq!(report.get(Counter::ViewsOpened), 0);
    assert_eq!(report.unattributed_hydrations(), 0);
    report.check_identity().unwrap();
}
