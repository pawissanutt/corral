//! Real executor entries, not a table of synthetic submit callbacks.
#![cfg(unix)]

use std::sync::{Arc, Mutex};

use anyhow::Result;
use corral_core::{
    AuthorizedSubmission, AutoStage, AutopilotState, OperatingActionKind, OperatingOrigin, Status,
    SubmissionDisposition,
    backend::{ChangeSeq, OpKind, PageReq, StorageQuery, StorageRow},
};

pub(crate) mod fixture {
    use crate as corral_ops;
    include!("../tests/operating_launch/fixture.rs");
}
#[path = "../tests/support/action_budget.rs"]
mod action_budget;
#[path = "../tests/support/action_budget_checks.rs"]
mod action_budget_checks;
#[path = "../tests/support/action_budget_fixture.rs"]
mod action_budget_fixture;
#[path = "../tests/support/action_budget_growth.rs"]
mod action_budget_growth;
#[path = "../tests/support/action_budget_hot.rs"]
mod action_budget_hot;
#[path = "../tests/support/action_budget_prompts.rs"]
mod action_budget_prompts;
#[path = "../tests/support/action_budget_trace.rs"]
mod action_budget_trace;
#[path = "../tests/support/action_heads.rs"]
mod action_heads;
#[path = "../tests/support/submission_aggregate.rs"]
mod aggregate;
#[path = "../tests/support/submission_path_crashes.rs"]
mod crashes;
#[path = "../tests/support/launch_resume.rs"]
mod launch_resume;
#[path = "../tests/support/submission_refusals.rs"]
mod refusals;
#[path = "../tests/support/reviewer_guard.rs"]
mod reviewer_guard;

use fixture::Fx;

const PLAN: &str = "---\ngoal: Bounded regression\nnon_goals: []\ndecisions: []\naffected_paths: [candidate.txt]\ncontracts: []\ndependencies: []\nacceptance: [{scenario: bounded, expected: verified}]\nverification: [check]\nowner_decisions: []\n---\n";

#[derive(Clone, Copy, Debug)]
enum Path {
    Worker,
    Planner,
    RepairLegacy,
    RepairInbox,
    PlanReviewPane,
    PlanReviewPm,
    AcceptPlan,
    ReviewerPane,
    ReviewerPm,
    Gate,
}

impl Path {
    fn kind(self) -> OperatingActionKind {
        match self {
            Self::Worker => OperatingActionKind::Dispatch,
            Self::Planner => OperatingActionKind::Plan,
            Self::RepairLegacy | Self::RepairInbox => OperatingActionKind::Repair,
            Self::PlanReviewPane | Self::PlanReviewPm => OperatingActionKind::PlanReview,
            Self::AcceptPlan => OperatingActionKind::AcceptPlan,
            Self::ReviewerPane | Self::ReviewerPm => OperatingActionKind::Review,
            Self::Gate => OperatingActionKind::Gate,
        }
    }

    fn effects(self) -> (usize, usize, usize) {
        match self {
            Self::Worker
            | Self::Planner
            | Self::PlanReviewPane
            | Self::ReviewerPane
            | Self::Gate => (1, 0, 0),
            Self::RepairLegacy | Self::PlanReviewPm | Self::ReviewerPm => (0, 1, 0),
            Self::RepairInbox => (0, 0, 1),
            Self::AcceptPlan => (0, 0, 0),
        }
    }

    fn setup(self) -> (Fx, String) {
        let fx = Fx::new();
        let task = fx.task("submission matrix", Status::Ready);
        if matches!(self, Self::Planner) {
            fx.ctx
                .store
                .update(&task.id, |t| {
                    t.design = true;
                    Ok(())
                })
                .unwrap();
        }
        if matches!(
            self,
            Self::PlanReviewPane | Self::PlanReviewPm | Self::AcceptPlan
        ) {
            fx.ctx
                .store
                .update(&task.id, |t| {
                    t.design = true;
                    Ok(())
                })
                .unwrap();
            fx.start(&task.id);
            let source = fx.temp.path().join("plan.md");
            std::fs::write(&source, PLAN).unwrap();
            crate::plan_submit(&fx.ctx, &task.id, &source).unwrap();
            if matches!(self, Self::AcceptPlan) {
                let review = crate::plan_review(&fx.ctx, &task.id, Some("rev")).unwrap();
                crate::plan_verdict(
                    &fx.ctx,
                    &task.id,
                    &crate::PlanVerdict {
                        plan: "P1",
                        attempt: Some(&review.id),
                        verdict: corral_core::Verdict::Approve,
                        notes: "independently verified",
                    },
                )
                .unwrap();
                // The independent verdict is collected before the grant exists.
                // Reconcile its owner-attention lifecycle before testing accept_auto itself.
                fx.ctx
                    .store
                    .update(&task.id, |task| {
                        task.status = Status::Review;
                        Ok(())
                    })
                    .unwrap();
            }
        }
        if matches!(
            self,
            Self::RepairLegacy
                | Self::RepairInbox
                | Self::ReviewerPane
                | Self::ReviewerPm
                | Self::Gate
        ) {
            fx.start(&task.id);
            fx.done(&task.id, "candidate.txt");
            if matches!(self, Self::RepairLegacy | Self::RepairInbox) {
                fx.ctx
                    .store
                    .update(&task.id, |t| {
                        t.current_run_mut().unwrap().delivery = if matches!(self, Self::RepairInbox)
                        {
                            corral_core::inbox::DeliveryMode::Inbox
                        } else {
                            corral_core::inbox::DeliveryMode::Legacy
                        };
                        Ok(())
                    })
                    .unwrap();
            }
            if matches!(self, Self::ReviewerPane | Self::ReviewerPm) {
                crate::gate_start(&fx.ctx, &task.id).unwrap();
                assert!(crate::gate_collect(&fx.ctx, &task.id).unwrap().is_some());
            }
        }
        if matches!(self, Self::PlanReviewPm | Self::ReviewerPm) {
            // Supply an existing shared PM, so bootstrap is not counted as the action.
            let task = fx.ctx.store.get(&task.id).unwrap();
            let herdr = task
                .plan_runs
                .last()
                .and_then(|r| r.herdr.as_ref())
                .or_else(|| task.current_run().and_then(|r| r.herdr.as_ref()))
                .unwrap();
            corral_core::records::RecordArea::new("pm")
                .save(
                    &fx.ctx.store,
                    "state",
                    &serde_json::json!({"pane_id":herdr.pane_id}),
                )
                .unwrap();
        }
        (fx, task.id)
    }

    fn enter(self, ctx: &crate::Ctx, id: &str, automatic: bool) -> Result<()> {
        match self {
            Self::Worker | Self::Planner => {
                if automatic {
                    crate::dispatch::dispatch_automatic(
                        ctx,
                        id,
                        serde_json::json!({"source":"repair"}),
                        fixture::now(),
                    )?;
                } else {
                    crate::dispatch(ctx, id, &Default::default())?;
                }
            }
            Self::RepairLegacy | Self::RepairInbox => {
                let task = ctx.store.get(id)?;
                let run = task.current_run().unwrap();
                let key = crate::integration::RepairKey::new(&task, &run_head(run), "regression")?;
                crate::dispatch::auto_request_changes(
                    ctx,
                    id,
                    "repair regression",
                    1,
                    AutopilotState {
                        stage: AutoStage::Gating,
                        reason: None,
                        sha: None,
                        at: fixture::now(),
                    },
                    &key,
                )?;
            }
            Self::PlanReviewPane | Self::PlanReviewPm => {
                if automatic {
                    crate::plan_review::review_auto(ctx, id)?;
                } else {
                    crate::plan_review(
                        ctx,
                        id,
                        Some(if matches!(self, Self::PlanReviewPm) {
                            "pm"
                        } else {
                            "rev"
                        }),
                    )?;
                }
            }
            Self::AcceptPlan => crate::plan_review::accept_auto(ctx, id, "P1")?,
            Self::ReviewerPane | Self::ReviewerPm => {
                let reviewer = Some(if matches!(self, Self::ReviewerPm) {
                    "pm"
                } else {
                    "rev"
                });
                if automatic {
                    crate::reviewer::review_run_automatic(ctx, id, reviewer)?;
                } else {
                    crate::review_run(ctx, id, reviewer)?;
                }
            }
            Self::Gate => {
                if automatic {
                    crate::gate::gate_start_automatic(ctx, id)?;
                } else {
                    crate::gate_start(ctx, id)?;
                }
            }
        }
        Ok(())
    }
}

fn authorizations(
    ctx: &crate::Ctx,
    id: &str,
    kind: OperatingActionKind,
) -> Vec<AuthorizedSubmission> {
    ctx.store
        .view()
        .unwrap()
        .storage_rows(
            &StorageQuery::Authorizations {
                task: Some(id.into()),
            },
            PageReq::first(128),
        )
        .unwrap()
        .items
        .into_iter()
        .filter_map(|row| {
            let StorageRow::Authorization(row) = row else {
                return None;
            };
            let record: AuthorizedSubmission = serde_json::from_value(row.doc).unwrap();
            (record.authorization.action.kind == kind).then_some(record)
        })
        .collect()
}

fn counts(fx: &Fx) -> (usize, usize, usize) {
    (
        fx.records("submissions.jsonl").len(),
        fx.records("deliveries.jsonl").len(),
        fx.fresh()
            .store
            .events()
            .unwrap()
            .iter()
            .filter(|e| e.kind == "message_posted")
            .count(),
    )
}

fn assert_authorization_commit(
    store: &corral_core::Store,
    point: &str,
    cursor: &Mutex<Option<ChangeSeq>>,
) {
    if point == "prepare_returned" {
        *cursor.lock().unwrap() = Some(store.view().unwrap().seq());
    } else if point == "authorization_committed" {
        let before = cursor.lock().unwrap().unwrap();
        let view = store.view().unwrap();
        assert_eq!(
            view.seq().0,
            before.0 + 1,
            "submission split its durable writes"
        );
        let changes = view.changes(before, 8).unwrap();
        assert_eq!(changes.entries.len(), 1);
        assert_eq!(changes.entries[0].op, OpKind::Authorization);
    }
}

fn assert_lock_order(held: &[String], task_id: &str, inbox: bool) {
    let board = held.iter().rposition(|lease| lease == "dispatch").unwrap();
    let task = held.iter().rposition(|lease| lease == task_id).unwrap();
    assert!(board < task, "{held:?}");
    if inbox {
        let posting = held
            .iter()
            .rposition(|lease| lease.starts_with("inbox-"))
            .unwrap();
        assert!(posting < board, "{held:?}");
    }
}

#[test]
fn submission_paths_real_entries_commit_before_exact_effect_and_refuse_replay() {
    for path in [
        Path::Worker,
        Path::Planner,
        Path::RepairLegacy,
        Path::RepairInbox,
        Path::PlanReviewPane,
        Path::PlanReviewPm,
        Path::AcceptPlan,
        Path::ReviewerPane,
        Path::ReviewerPm,
        Path::Gate,
    ] {
        for automatic in [false, true] {
            if !automatic
                && matches!(
                    path,
                    Path::RepairLegacy | Path::RepairInbox | Path::AcceptPlan
                )
            {
                continue; // These call sites have no owner-origin submit variant.
            }
            let (mut fx, id) = path.setup();
            if automatic {
                if matches!(path, Path::PlanReviewPm) {
                    fx.ctx.store.config.review.reviewer = Some("pm".into());
                    fx.ctx.store.save_config().unwrap();
                }
                fx.grant(&[&id]);
            }
            let before = counts(&fx);
            let stages = Arc::new(Mutex::new(Vec::new()));
            let captured = stages.clone();
            let locks = Arc::new(Mutex::new(Vec::new()));
            let acquired = locks.clone();
            let store = fx.ctx.store.clone();
            let bin = fx.bin.clone();
            let task_id = id.clone();
            let kind = path.kind();
            let cursor = Mutex::new(None);
            fx.ctx.store = fx.ctx.store.with_submission_barrier(Arc::new(move |point| {
                if let Some(lease) = point.strip_prefix("lock:") {
                    acquired.lock().unwrap().push(lease.to_owned());
                    return;
                }
                // `review_child_observed` is the reviewer launch's guard observation, before
                // its submission is entered.
                if matches!(
                    point,
                    "submission_entered" | "refused_committed" | "review_child_observed"
                ) {
                    return;
                }
                let ctx = crate::Ctx::from_store(store.clone());
                assert_authorization_commit(&store, point, &cursor);
                let records = authorizations(&ctx, &task_id, kind);
                let expected = match point {
                    "inbox_reserved" | "prepare_returned" => SubmissionDisposition::Prepared,
                    "authorization_committed"
                    | "send_returned"
                    | "effect_entered"
                    | "effect_returned" => SubmissionDisposition::Submitting,
                    "submitted_committed" => SubmissionDisposition::Submitted,
                    other => panic!("{path:?}: unexpected barrier {other}"),
                };
                assert!(
                    records.iter().any(|record| record.disposition == expected),
                    "{path:?} {point}: {records:?}"
                );
                if point == "inbox_reserved" {
                    assert!(matches!(path, Path::RepairInbox));
                }
                if matches!(point, "inbox_reserved" | "prepare_returned") {
                    let held = acquired.lock().unwrap();
                    // The last dispatch/task pair is the submission, not earlier preparation.
                    assert_lock_order(&held, &task_id, matches!(path, Path::RepairInbox));
                }
                if matches!(
                    point,
                    "inbox_reserved"
                        | "authorization_committed"
                        | "send_returned"
                        | "effect_entered"
                        | "effect_returned"
                ) {
                    let pane_runs = std::fs::read_to_string(bin.join("submissions.jsonl"))
                        .unwrap_or_default()
                        .lines()
                        .count();
                    let prompts = std::fs::read_to_string(bin.join("deliveries.jsonl"))
                        .unwrap_or_default()
                        .lines()
                        .count();
                    let messages = ctx
                        .store
                        .events()
                        .unwrap()
                        .iter()
                        .filter(|event| event.kind == "message_posted")
                        .count();
                    let expected = if matches!(point, "send_returned" | "effect_returned") {
                        path.effects()
                    } else {
                        (0, 0, 0)
                    };
                    assert_eq!(
                        (
                            pane_runs - before.0,
                            prompts - before.1,
                            messages - before.2
                        ),
                        expected,
                        "{path:?} {point}: effects inside submit"
                    );
                }
                if !point.starts_with("effect_") {
                    captured.lock().unwrap().push(point.to_owned());
                }
            }));
            path.enter(&fx.ctx, &id, automatic)
                .unwrap_or_else(|e| panic!("{path:?} automatic={automatic}: {e:#}"));
            let mut expected_stages = vec![
                "prepare_returned",
                "authorization_committed",
                "send_returned",
                "submitted_committed",
            ];
            if matches!(path, Path::RepairInbox) {
                expected_stages.insert(0, "inbox_reserved");
            }
            assert_eq!(
                *stages.lock().unwrap(),
                expected_stages,
                "{path:?} automatic={automatic}"
            );
            let after = counts(&fx);
            let expected = path.effects();
            // An inbox delivery kick is outside submit and intentionally tested separately below.
            let deliveries = if matches!(path, Path::RepairInbox) {
                0
            } else {
                after.1 - before.1
            };
            assert_eq!(
                (after.0 - before.0, deliveries, after.2 - before.2),
                expected,
                "{path:?} automatic={automatic}"
            );
            let record = authorizations(&fx.ctx, &id, path.kind())
                .into_iter()
                .find(|record| record.disposition == SubmissionDisposition::Submitted)
                .unwrap();
            assert_eq!(
                matches!(
                    record.authorization.origin,
                    OperatingOrigin::Automatic { .. }
                ),
                automatic
            );
            let result = fx.ctx.store.submit_operating_action(
                &record.authorization,
                &record.authorization.action,
                |_, _| panic!("replay ran prepare"),
                || -> Result<()> { panic!("replay sent") },
            );
            assert!(
                result.unwrap_err().to_string().contains("refusing replay"),
                "{path:?}"
            );
            assert_eq!(counts(&fx), after, "{path:?}: replay changed effects");
        }
    }
}

#[test]
fn submission_paths_paused_policy_refuses_every_automatic_entry_without_effects() {
    for path in [
        Path::Worker,
        Path::Planner,
        Path::RepairLegacy,
        Path::RepairInbox,
        Path::PlanReviewPane,
        Path::PlanReviewPm,
        Path::AcceptPlan,
        Path::ReviewerPane,
        Path::ReviewerPm,
        Path::Gate,
    ] {
        let (mut fx, id) = path.setup();
        if matches!(path, Path::PlanReviewPm) {
            fx.ctx.store.config.review.reviewer = Some("pm".into());
            fx.ctx.store.save_config().unwrap();
        }
        fx.grant(&[&id]);
        fx.ctx.store.pause_operating_policy(true).unwrap();
        let before = counts(&fx);
        let stages = Arc::new(Mutex::new(Vec::new()));
        let seen = stages.clone();
        fx.ctx.store = fx.ctx.store.with_submission_barrier(Arc::new(move |point| {
            if !point.starts_with("lock:")
                && !matches!(point, "submission_entered" | "refused_committed")
            {
                seen.lock().unwrap().push(point.to_owned());
            }
        }));
        assert!(path.enter(&fx.ctx, &id, true).is_err(), "{path:?}");
        assert!(
            stages.lock().unwrap().is_empty(),
            "{path:?}: refused action ran callback"
        );
        assert_eq!(
            counts(&fx),
            before,
            "{path:?}: refused action produced an effect"
        );
    }
}

fn recovered_setup(path: Path, automatic: bool) -> (Fx, String) {
    let (mut fx, id) = path.setup();
    if automatic {
        fx.grant(&[&id]);
    }
    refuse_prepared(&fx, &id, path, automatic);
    (fx, id)
}

/// A known never-sent attempt of `path` on task `id`: prepared, bound and refused once.
fn refuse_prepared(fx: &Fx, id: &str, path: Path, automatic: bool) {
    use crate::operating::Origin;
    use crate::review_budget::{Kind, ReviewLaunch};
    use corral_core::ReviewRecord;

    let id = id.to_owned();
    let kind = if matches!(path, Path::Gate) {
        Kind::Gate
    } else {
        Kind::Reviewer
    };
    let task = fx.ctx.store.get(&id).unwrap();
    let run = task.current_run().unwrap();
    let candidate = if kind == Kind::Reviewer {
        let gate = run.gate.as_ref().unwrap().identity.as_ref().unwrap();
        crate::verification::identity(&fx.ctx, &task, &run_head(run), gate).unwrap()
    } else {
        crate::verification::capture_identity(&fx.ctx, &task, &run_head(run), "candidate").unwrap()
    };
    let pane = run.herdr.as_ref().unwrap().clone();
    {
        let mut launch = ReviewLaunch::reserve_origin(
            &fx.ctx,
            &id,
            kind,
            false,
            if automatic {
                Origin::Automatic
            } else {
                Origin::Owner
            },
        )
        .unwrap();
        let child_id = (kind == Kind::Reviewer).then_some("R1-review1");
        launch
            .bind(&candidate, child_id.unwrap_or("candidate"), child_id)
            .unwrap();
        launch
            .attach(&pane.pane_id, !matches!(path, Path::ReviewerPm))
            .unwrap();
        fx.ctx
            .store
            .update(&id, |task| {
                let run = task.current_run_mut().unwrap();
                if kind == Kind::Gate {
                    run.gate = Some(serde_json::from_value(serde_json::json!({
                        "result":"running", "sha":candidate.head, "at":fixture::now(),
                        "pane":pane.pane_id, "socket":pane.socket, "identity":candidate
                    }))?);
                } else {
                    let review = ReviewRecord {
                        route: None,
                        identity: Some(candidate.clone()),
                        evidence_digest: Some(crate::verification::evidence_digest(
                            &corral_core::RunHead::of(run)?,
                        )?),
                        verdict: None,
                        process_lost: false,
                        reviewer: if matches!(path, Path::ReviewerPm) {
                            "pm"
                        } else {
                            "rev"
                        }
                        .into(),
                        notes: None,
                        sha: candidate.head.clone(),
                        at: fixture::now(),
                        pane: Some(pane.pane_id.clone()),
                    };
                    let mut child = run.clone();
                    child.id = "R1-review1".into();
                    child.agent = "rev".into();
                    child.review = Some(review.clone());
                    child.review_runs.clear();
                    child.pending_reply = Some("prepared review request".into());
                    child.herdr = Some(pane.clone());
                    run.review = Some(review);
                    run.review_runs.push(child);
                }
                Ok(())
            })
            .unwrap();
        let prepared_file = if kind == Kind::Gate {
            crate::gate::gate_paths(&fx.ctx, &id, &run.id).script
        } else {
            crate::reviewer::review_dir(&fx.ctx, &id, &run.id, "R1-review1")
                .unwrap()
                .join("review.md")
        };
        std::fs::create_dir_all(prepared_file.parent().unwrap()).unwrap();
        std::fs::write(prepared_file, "prepared regression fixture").unwrap();
        // Set up a known never-sent attempt; the assertion below uses the real entry
        // and ReviewLaunch::resume, not another synthetic submission.
        let error = launch
            .submit(
                |_, _| anyhow::bail!("fixture precondition refused"),
                || -> Result<()> { panic!("refused fixture sent") },
            )
            .unwrap_err();
        assert!(crate::operating::refused(&error));
    }
}

#[test]
fn submission_paths_recovered_auxiliary_entries_reuse_refused_attempts_once() {
    for path in [Path::Gate, Path::ReviewerPane, Path::ReviewerPm] {
        for automatic in [false, true] {
            let (mut fx, id) = recovered_setup(path, automatic);
            let before = counts(&fx);
            let authorization = authorizations(&fx.ctx, &id, path.kind())
                .into_iter()
                .find(|r| {
                    matches!(
                        r.disposition,
                        SubmissionDisposition::AuthorizationRefused { .. }
                    )
                })
                .unwrap()
                .authorization;
            let store = fx.ctx.store.clone();
            let cursor = Mutex::new(None);
            let locks = Mutex::new(Vec::new());
            let task_id = id.clone();
            fx.ctx.store = fx.ctx.store.with_submission_barrier(Arc::new(move |point| {
                if let Some(lease) = point.strip_prefix("lock:") {
                    locks.lock().unwrap().push(lease.to_owned());
                    return;
                }
                assert_authorization_commit(&store, point, &cursor);
                if point == "prepare_returned" {
                    assert_lock_order(&locks.lock().unwrap(), &task_id, false);
                }
            }));
            path.enter(&fx.ctx, &id, automatic)
                .unwrap_or_else(|error| panic!("{path:?}: {error:#}"));
            let after = counts(&fx);
            assert_eq!(
                (after.0 - before.0, after.1 - before.1, after.2 - before.2),
                path.effects(),
                "{path:?}"
            );
            let record = fx
                .ctx
                .store
                .authorized_submission(&authorization.id)
                .unwrap();
            assert_eq!(
                record.authorization.action, authorization.action,
                "{path:?}"
            );
            assert_eq!(
                record.disposition,
                SubmissionDisposition::Submitted,
                "{path:?}"
            );
            assert!(
                fx.ctx
                    .store
                    .submit_operating_action(
                        &record.authorization,
                        &record.authorization.action,
                        |_, _| panic!("recovery replay prepared"),
                        || -> Result<()> { panic!("recovery replay sent") }
                    )
                    .is_err()
            );
            assert_eq!(counts(&fx), after);
        }
    }
}

/// The run projection a converted helper takes.
fn run_head(run: &corral_core::Run) -> corral_core::RunHead {
    corral_core::RunHead::of(run).unwrap()
}
