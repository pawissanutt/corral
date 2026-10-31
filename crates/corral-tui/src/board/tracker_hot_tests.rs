use super::tests::budget_seeded as seeded;
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

#[derive(Debug)]
struct AcceptedProbe {
    pinned: corral_ops::resources::PinnedReadingsProbe,
    volumes: std::collections::BTreeSet<std::path::PathBuf>,
}

impl corral_ops::resources::ResourceProbe for AcceptedProbe {
    fn sample(&self, store: &Store) -> anyhow::Result<corral_ops::resources::ResourceSnapshot> {
        let snapshot = corral_ops::resources::ResourceProbe::sample(&self.pinned, store)?;
        assert_eq!(
            snapshot
                .disks
                .iter()
                .map(|disk| disk.path.clone())
                .collect::<std::collections::BTreeSet<_>>(),
            self.volumes
        );
        Ok(snapshot)
    }
}

fn accepted_frame_budget(code: bool) {
    let mut reports = Vec::new();
    let worktrees = tempfile::tempdir().unwrap();
    for factor in [1, 10] {
        // Given a Done accepted member, not the Running early-return branch.
        let mut fx = fixture(factor, true);
        // A shared /tmp/wt appearing in another test must not change this pass's volume set.
        for mut task in fx.ctx.store.list().unwrap() {
            for run in &mut task.runs {
                run.worktree = worktrees.path().join(&task.id).join(&run.id);
            }
            fx.ctx.store.fixture_put_task(&task).unwrap();
        }
        let mut member = fx.ctx.store.get(MEMBER).unwrap();
        member.status = Status::Done;
        let run = member.runs.last_mut().unwrap();
        run.started = at(0);
        run.ended = Some(at(1));
        run.outcome = Some(corral_core::RunOutcome::Done);
        let sha = if code {
            "1".repeat(40)
        } else {
            run.base_sha.clone()
        };
        fx.ctx.store.fixture_put_task(&member).unwrap();
        fx.ctx
            .store
            .log_event(
                Some(MEMBER),
                "approved",
                json!({
                    "run":"R1", "sha":sha, "strategy":"rebase", "base":"main"
                }),
            )
            .unwrap();
        let source = fx.ctx.store.root().join("accepted.txt");
        for n in 0..4 * factor {
            std::fs::write(&source, format!("accepted artifact {n:04}\n")).unwrap();
            corral_ops::add_artifact(
                &fx.ctx,
                MEMBER,
                &source,
                Some(corral_core::ArtifactKind::Evidence),
                None,
            )
            .unwrap();
        }
        let mut member = fx.ctx.store.get(MEMBER).unwrap();
        member.updated = at(1);
        for artifact in &mut member.artifacts {
            artifact.added = at(0);
        }
        fx.ctx.store.fixture_put_task(&member).unwrap();
        assert_eq!(
            serde_json::to_value(&fx.ctx.store.config.resources).unwrap(),
            serde_json::to_value(corral_core::ResourcesCfg::default()).unwrap()
        );
        let mut volumes = std::collections::BTreeSet::from([fx.ctx.store.dir().to_path_buf()]);
        for name in fx.ctx.store.config.projects.keys() {
            let repo = fx.ctx.store.project_path(name).unwrap();
            volumes.extend([repo.join(".worktrees"), repo.join("target")]);
        }
        for task in fx.ctx.store.list().unwrap() {
            for run in task.runs {
                if run.worktree.exists() {
                    volumes.extend([run.worktree.join("target"), run.worktree]);
                }
            }
        }
        if let Some(target) = std::env::var_os("CARGO_TARGET_DIR").filter(|s| !s.is_empty()) {
            let target = std::path::PathBuf::from(target);
            volumes.insert(if target.is_absolute() {
                target
            } else {
                std::env::current_dir().unwrap().join(target)
            });
        }
        let probe = std::sync::Arc::new(AcceptedProbe {
            pinned: corral_ops::resources::PinnedReadingsProbe::admitting(),
            volumes,
        });
        fx.ctx.resource_probe = probe.clone();
        let tracker = fx.ctx.store.get(PLAN).unwrap();
        let acceptance = tracker.approved_scope.as_ref().unwrap().acceptance.clone();
        let error = corral_git::rev_sha(fx.ctx.store.root(), "refs/heads/main").unwrap_err();
        // The full-task oracle reaches the same Git error after checking valid snapshots.
        let expected = json!({
            "task":tracker.id, "status":tracker.status, "approved_scope":tracker.approved_scope,
            "scope_revision":tracker.approved_scope_revision,
            "counts":{"accepted":1, "running":0, "review":0, "waiting":0, "missing":0},
            "work":[{
                "task":member.id, "title":member.title, "status":member.status, "state":"accepted",
                "run":member.current_run().map(|r| &r.id), "reason":null, "prerequisites":[]
            }],
            "coverage":acceptance.iter().map(|criterion| json!({
                "acceptance":criterion, "tasks":[]
            })).collect::<Vec<_>>(),
            "unmapped_criteria":acceptance,
            "aggregate":{
                "ready":false, "blockers":[format!("{error:#}")], "attempt":null,
                "accepted":false, "acceptance":null
            }
        });
        BoardSnapshot::load(&fx.ctx).unwrap();
        let before = work::snapshot();
        let samples = probe.pinned.samples();

        // When the production frame refreshes its warmed accepted-member path.
        let frame = BoardSnapshot::load(&fx.ctx).unwrap();
        let report = work::snapshot().since(&before);

        // Then its complete projection and hot budget agree at 1x and 10x history.
        assert_eq!(probe.pinned.samples() - samples, 1);
        assert_eq!(report.followup_calls("FU-3", "sample"), 1);
        assert!(frame.resources.reason.is_none());
        let mut board = fx.board();
        board.apply_snapshot(frame);
        assert_eq!(
            serde_json::to_value(board.plan_progress[PLAN].as_ref().unwrap()).unwrap(),
            expected
        );
        assert_eq!(report.unattributed_hydrations(), 0);
        report.check_identity().unwrap();
        reports.push(report);
    }
    for counter in Counter::ALL {
        assert_eq!(
            reports[0].get(counter),
            reports[1].get(counter),
            "{counter:?}"
        );
    }
}

fn frame_membership_edge(parent: bool) {
    let mut refused_reports = Vec::new();
    let mut restored_reports = Vec::new();
    for factor in [1, 10] {
        // Given a warmed frame with one accepted member and tenfold terminal/member history.
        let mut fx = fixture(factor, false);
        let probe = std::sync::Arc::new(corral_ops::resources::PinnedReadingsProbe::admitting());
        fx.ctx.resource_probe = probe.clone();
        let mut member = fx.ctx.store.get(MEMBER).unwrap();
        member.status = Status::Done;
        // Lesson preparation refreshes a changed project's newest 256 sources, each holding a
        // task's newest 32 runs and 32 eligible artifacts: the db316644 signal windows. Fill
        // them at both sizes so the comparison isolates growth beyond them.
        for i in 0..32 {
            let id = format!("pad{i:04}");
            let mut run = super::tests::run(&id, "worker");
            run.started = at(0);
            run.ended = Some(at(1));
            run.outcome = Some(corral_core::RunOutcome::Done);
            let current = member.runs.len() - 1;
            member.runs.insert(current, run);
            member.artifacts.insert(
                0,
                serde_json::from_value(json!({
                    "run": id, "kind": "log", "title": "Pad log",
                    "path": format!("artifacts/{}/{id}/log.txt", member.id),
                    "source": "/tmp/log.txt", "sha256": "0".repeat(64), "bytes": 1,
                    "added": at(2)
                }))
                .unwrap(),
            );
        }
        fx.ctx.store.fixture_put_task(&member).unwrap();
        for _ in 0..260 {
            fx.ctx.store.create("Old", None, Status::Done, "").unwrap();
        }
        // Grow terminal trackers as well as ordinary terminal tasks and member history:
        // scanning all trackers must not hide behind an unchanged open-board population.
        for mut old in fx
            .ctx
            .store
            .list()
            .unwrap()
            .into_iter()
            .filter(|task| task.title == "Old")
        {
            old.tracker = true;
            old.project = Some("corral".into());
            old.created = at(-100);
            // Whole seconds: wall-clock stamps vary in printed width between runs.
            old.updated = at(-100);
            fx.ctx.store.fixture_put_task(&old).unwrap();
        }
        // The constructor performs exactly one complete production board load.
        let mut board = fx.board();
        assert_eq!(probe.samples(), 1);
        let accepted = board.plan_progress[PLAN].as_ref().unwrap();
        assert_eq!(accepted.counts.accepted, 1);
        let accepted = serde_json::to_value(accepted).unwrap();
        let new = fx
            .ctx
            .store
            .create("New", Some("corral"), Status::Done, "")
            .unwrap();
        let mut new = fx
            .ctx
            .store
            .update(&new.id, |task| {
                if parent {
                    task.parent = Some(PLAN.into());
                } else {
                    task.tracker = true;
                    task.children = vec![MEMBER.into()];
                }
                Ok(())
            })
            .unwrap();
        new.created = at(0);
        new.updated = at(0);
        fx.ctx.store.fixture_put_task(&new).unwrap();
        let expected = format!("{:#}", fx.ctx.store.execution_plan(&member).unwrap_err());
        let samples = probe.samples();
        let before = work::snapshot();

        // When the warm second frame sees a new incoming membership edge, measure the
        // WHOLE production call, including preparation outside its internal HotScope.
        let refused = {
            let _whole_call = HotScope::enter();
            BoardSnapshot::load(&fx.ctx).unwrap()
        };

        // Then the real frame refuses exactly like the uncached full-task validator.
        let report = work::snapshot().since(&before);
        board.apply_snapshot(refused);
        assert_eq!(board.plan_progress[PLAN].as_ref().unwrap_err(), &expected);
        assert_eq!(probe.samples() - samples, 1);
        refused_reports.push(report);
        let mut new = fx
            .ctx
            .store
            .update(&new.id, |task| {
                task.parent = None;
                task.children.clear();
                Ok(())
            })
            .unwrap();
        new.updated = at(0);
        fx.ctx.store.fixture_put_task(&new).unwrap();
        assert_eq!(
            fx.ctx.store.execution_plan(&member).unwrap().unwrap().id,
            PLAN
        );
        let samples = probe.samples();
        let before = work::snapshot();
        let restored = {
            let _whole_call = HotScope::enter();
            BoardSnapshot::load(&fx.ctx).unwrap()
        };
        let report = work::snapshot().since(&before);
        board.apply_snapshot(restored);
        assert_eq!(
            serde_json::to_value(board.plan_progress[PLAN].as_ref().unwrap()).unwrap(),
            accepted
        );
        assert_eq!(probe.samples() - samples, 1);
        restored_reports.push(report);
    }
    for reports in [&refused_reports, &restored_reports] {
        for report in reports {
            assert_eq!(report.unattributed_hydrations(), 0);
            report.check_identity().unwrap();
        }
        for counter in Counter::ALL {
            assert_eq!(
                reports[0].get(counter),
                reports[1].get(counter),
                "{counter:?}; all cells: {:?}",
                Counter::ALL
                    .iter()
                    .map(|&cell| (cell, reports[0].get(cell), reports[1].get(cell)))
                    .collect::<Vec<_>>()
            );
        }
    }
}

#[test]
fn accepted_code_member_frame_keeps_same_run_artifact_work_constant() {
    accepted_frame_budget(true);
}

#[test]
fn accepted_no_code_member_frame_keeps_same_run_artifact_work_constant() {
    accepted_frame_budget(false);
}

#[test]
fn warm_second_frame_membership_edge_budget_includes_preparation() {
    frame_membership_edge(false);
}

#[test]
fn frame_tracker_refuses_new_unlisted_child_then_recovers() {
    frame_membership_edge(true);
}
