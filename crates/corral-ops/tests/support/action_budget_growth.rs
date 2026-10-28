//! History growth for the action budget's stores (plan T-253 section 4): every history family
//! of the acted task, its relations and terminal tasks, and the emitted rows its prompts show.
use std::path::Path as FsPath;

use corral_core::{CommentAuthor, Run, Status, Store, TaskComment};
use serde_json::json;

use super::action_budget_fixture::Kind;
use super::fixture;

pub(super) fn bare_historic(index: usize, worktree: &FsPath) -> Run {
    serde_json::from_value(json!({
        "id": format!("H{index:03}"), "agent": "historic", "harness": "historic",
        "worktree": worktree.join(format!("history-{index:03}")), "branch": "corral/history",
        "base": "main", "base_sha": "0".repeat(40), "started": fixture::now(),
        "ended": fixture::now(), "outcome": "superseded",
        "review": {"reviewer": "historic", "sha": "0".repeat(40), "verdict": "approve",
                   "at": fixture::now()},
    }))
    .unwrap()
}

/// A finished receipt of run `run` for obligation `id`.
pub(super) fn receipt(
    run: &str,
    id: &str,
    outcome: corral_core::ReceiptOutcome,
) -> corral_core::ResumeReceipt {
    corral_core::ResumeReceipt {
        version: 1,
        id: format!("{id}-receipt"),
        run: run.into(),
        outcome,
        delivery: corral_core::ResumeDelivery::Acknowledged,
        observed_at: fixture::now(),
        answer: (outcome == corral_core::ReceiptOutcome::Answered).then(|| "main".into()),
        data: json!({}),
    }
}

/// A checkpoint record of run `run` at `revision`, freezing `frozen`.
pub(super) fn checkpoint_record(
    run: &str,
    revision: u64,
    frozen: &corral_core::RunObligations,
) -> corral_core::CheckpointRecord {
    serde_json::from_value(json!({
        "artifact": format!("artifacts/history/{run}-checkpoint-{revision}.json"),
        "checkpoint": {
            "version": 1, "revision": revision, "progress": null, "completed_steps": [],
            "current_step": null, "captured": fixture::now(), "run_obligations": frozen,
            "input": {
                "task": "T-000", "run": run, "spec_revision": "0".repeat(64),
                "expected_revision": 0, "progress_revision": 0, "head": "0".repeat(40),
                "base": "0".repeat(40), "tree": "0".repeat(64), "criterion_ids": [],
                "evidence_refs": [], "next_action": "Earlier work", "decisions": [],
                "waits": [], "resources": [],
            },
        },
    }))
    .unwrap()
}

/// A resolved wait and an answered decision of run `run`: final obligations.
pub(super) fn final_obligations(
    run: &str,
    i: usize,
) -> (corral_core::WaitRecord, corral_core::DecisionRecord) {
    use corral_core::{ObligationSource, ReceiptOutcome, WaitObservation, WaitState};
    let observation = WaitObservation {
        at: fixture::now(),
        state: WaitState::Ready,
        detail: "resolved earlier".into(),
    };
    let wait = format!("{run}-build-{i:03}");
    let decision = format!("{run}-branch-{i:03}");
    (
        corral_core::WaitRecord {
            request: corral_core::WaitRequest {
                version: 1,
                id: wait.clone(),
                run: run.into(),
                condition: "An earlier build completes".into(),
                owner: "fixture".into(),
                source: ObligationSource::Worker,
                deadline: None,
                handle: corral_core::WaitHandle::Channel {
                    channel_id: wait.clone(),
                    capability: None,
                },
            },
            registered_at: fixture::now(),
            observation: observation.clone(),
            receipt: Some(receipt(run, &wait, ReceiptOutcome::Ready)),
        },
        corral_core::DecisionRecord {
            request: corral_core::DecisionRequest {
                version: 1,
                id: decision.clone(),
                run: run.into(),
                question: "Which earlier branch?".into(),
                requested_action: "Choose a branch".into(),
                owner: "owner".into(),
                source: ObligationSource::Worker,
                deadline: None,
            },
            registered_at: fixture::now(),
            observation,
            receipt: Some(receipt(run, &decision, ReceiptOutcome::Answered)),
        },
    )
}

/// An earlier coding run with nested history of its own: a review child, a switch, an older
/// checkpoint, a resolved wait, an answered decision, applied inbox effects and an advanced
/// observe cursor. Nothing an action takes reads any of it.
pub(super) fn historic_run(index: usize, worktree: &FsPath) -> Run {
    let mut run = bare_historic(index, worktree);
    let mut child = bare_historic(index, worktree);
    child.id = format!("{}-review-1", run.id);
    child.outcome = Some(corral_core::RunOutcome::Done);
    run.review_runs.push(child);
    run.switches.push(corral_core::ProfileSwitch {
        from: "historic-a".into(),
        to: "historic-b".into(),
        reason: "earlier switch".into(),
    });
    run.checkpoints
        .push(checkpoint_record(&run.id, 1, &Default::default()));
    let (wait, decision) = final_obligations(&run.id, 0);
    run.obligations.waits.push(wait);
    run.obligations.decisions.push(decision);
    run.inbox.effects_applied.insert(1);
    run.inbox.next_seq = 2;
    run.inbox.observe_cursor.offset = 64;
    run.inbox.observe_cursor.last_complete_line = 1;
    run
}

/// `scale` earlier rows in every history family of `id` that no current input reads, and
/// nested growth before each current container's current child.
/// A Done relation whose only coding runs are grown history landed its last one: that run is Done and its
/// acceptance receipt names the project's `main`, which every dispatch base contains.
pub(super) fn land_grown(store: &Store, id: &str) {
    let task = store.get(id).unwrap();
    // A relation with runs of its own keeps its real acceptance receipt.
    let grown_only = task.runs.iter().all(|run| run.id.starts_with('H'));
    let Some(run) = task
        .runs
        .last()
        .filter(|_| task.status == Status::Done && grown_only)
    else {
        return;
    };
    let run = run.id.clone();
    store
        .update(id, |t| {
            t.runs.last_mut().unwrap().outcome = Some(corral_core::RunOutcome::Done);
            Ok(())
        })
        .unwrap();
    let repo = store.project_path("app").unwrap();
    let sha = corral_git::rev_sha(&repo, "main").unwrap();
    store
        .log_event(
            Some(id),
            "approved",
            json!({"run": run, "sha": sha, "strategy": "fast_forward", "base": "main"}),
        )
        .unwrap();
}

/// The growth [`grow`] claims is really there: every history family of `id` reaches `scale`.
pub(super) fn grown_by(store: &Store, id: &str, scale: usize, acted: bool) {
    let task = store.get(id).unwrap();
    let historic: Vec<&Run> = task.runs.iter().filter(|r| r.id.starts_with('H')).collect();
    let nested = |f: fn(&Run) -> usize| historic.iter().map(|r| f(r)).sum::<usize>();
    assert!(
        task.approved_scope_history.len() >= scale,
        "{id}: scope revisions"
    );
    if !acted || task.runs.len() > historic.len() {
        assert!(historic.len() >= scale, "{id}: earlier coding runs");
        for (family, count) in [
            ("review children", nested(|r| r.review_runs.len())),
            ("switches", nested(|r| r.switches.len())),
            ("older checkpoints", nested(|r| r.checkpoints.len())),
            ("final waits", nested(|r| r.obligations.waits.len())),
            (
                "answered decisions",
                nested(|r| r.obligations.decisions.len()),
            ),
            ("applied effects", nested(|r| r.inbox.effects_applied.len())),
        ] {
            assert!(count >= scale, "{id}: {family} {count} < {scale}");
        }
    }
    if let Some(run) = task.current_run().filter(|run| run.outcome.is_none()) {
        let entries = run.usage.as_ref().map_or(0, |u| u.entries.len());
        assert!(entries >= scale, "{id}: current usage entries");
    }
}

/// `scale` of every history family on task `id`. The acted task gets earlier coding runs only
/// beside its own (its actions decide on whether it has run); every other task gets them always.
pub(super) fn grow(store: &Store, root: &FsPath, id: &str, scale: usize, acted: bool) {
    store
        .update(id, |t| {
            if !acted || !t.runs.is_empty() {
                let earlier: Vec<Run> = (0..scale).map(|i| historic_run(i, root)).collect();
                t.runs.splice(0..0, earlier);
            }
            if let Some(current) = t.plan_runs.last().cloned() {
                let earlier = (0..scale).map(|i| {
                    let mut run = current.clone();
                    run.id = format!("HD{i:03}");
                    run.state = corral_core::PlanRunState::Cancelled;
                    run.ended = Some(fixture::now());
                    run.herdr = None;
                    run.inbox = Default::default();
                    run
                });
                let at = t.plan_runs.len() - 1;
                t.plan_runs.splice(at..at, earlier.collect::<Vec<_>>());
            }
            if let Some(current) = t.plans.last().cloned() {
                let earlier = (0..scale).map(|i| {
                    let mut plan = current.clone();
                    plan.id = format!("HP{i:03}");
                    plan.reviews.clear();
                    plan.approval = None;
                    plan
                });
                let at = t.plans.len() - 1;
                t.plans.splice(at..at, earlier.collect::<Vec<_>>());
                let plan = t.plans.last_mut().unwrap();
                if let Some(template) = plan.reviews.last().cloned() {
                    let reviews = (0..scale).map(|i| {
                        let mut review = template.clone();
                        review.id = format!("HPR{i:03}");
                        review.reviewer = "historic".into();
                        review.verdict = Some(corral_core::Verdict::Approve);
                        review.notes = None;
                        review.herdr = None;
                        review.pending_reply = None;
                        review.inbox = Default::default();
                        review
                    });
                    plan.reviews.splice(0..0, reviews.collect::<Vec<_>>());
                }
            }
            if let Some(current) = t.aggregate_attempts.last().cloned() {
                let earlier = (0..scale).map(|i| {
                    let mut attempt = current.clone();
                    attempt.id = format!("HA{i:03}");
                    attempt.acceptance = None;
                    attempt
                });
                let at = t.aggregate_attempts.len() - 1;
                t.aggregate_attempts
                    .splice(at..at, earlier.collect::<Vec<_>>());
                let attempt = t.aggregate_attempts.last_mut().unwrap();
                if let Some(template) = attempt.reviews.last().cloned() {
                    let reviews = (0..scale).map(|i| {
                        let mut review = template.clone();
                        review.id = format!("{}-H{i:03}", attempt.id);
                        review.notes = None;
                        review.herdr = None;
                        review
                    });
                    attempt.reviews.splice(0..0, reviews.collect::<Vec<_>>());
                }
            }
            // The current container's usage entries.
            if let Some(run) = t.runs.last_mut().filter(|run| run.outcome.is_none()) {
                let usage = run.usage.get_or_insert_with(Default::default);
                usage
                    .entries
                    .extend((0..scale).map(|i| corral_core::UsageEntry {
                        at: Some(fixture::now()),
                        latency_ms: Some(1),
                        model: Some(format!("historic-{i:03}")),
                        input: 1,
                        output: 1,
                        cache_read: 0,
                        cache_write: 0,
                        reported_usd: None,
                        estimated_usd: None,
                        price_source: None,
                        unpriced_tokens: 2,
                    }));
            }
            // Nested history goes before the current child and the last switch, which stay.
            if let Some(run) = t.runs.last_mut().filter(|run| !run.review_runs.is_empty()) {
                let children = (0..scale).map(|i| {
                    let mut child = bare_historic(i, root);
                    child.id = format!("{}-review-h{i:03}", run.id);
                    child.outcome = Some(corral_core::RunOutcome::Done);
                    child
                });
                run.review_runs.splice(0..0, children.collect::<Vec<_>>());
            }
            if let Some(run) = t.runs.last_mut().filter(|run| !run.switches.is_empty()) {
                let switches = (0..scale).map(|i| corral_core::ProfileSwitch {
                    from: format!("historic-{i}"),
                    to: format!("historic-{}", i + 1),
                    reason: "earlier switch".into(),
                });
                run.switches.splice(0..0, switches.collect::<Vec<_>>());
            }
            Ok(())
        })
        .unwrap();
    // Scope amendments: a plain update may not write them.
    let mut task = store.get(id).unwrap();
    let first = u64::try_from(task.approved_scope_history.len()).unwrap();
    task.approved_scope_history
        .extend(
            (0..scale as u64).map(|i| corral_core::ApprovedScopeRevision {
                revision: first + i + 1,
                approved_scope: None,
                superseded_at: fixture::now(),
            }),
        );
    store.fixture_put_task(&task).unwrap();
    for i in 0..scale {
        let run = format!("H{i:03}");
        for kind in ["fixture_history", "lessons_applied", "gate_started"] {
            store
                .log_event(
                    Some(id),
                    kind,
                    json!({"run": run, "lessons": [], "pane": "h"}),
                )
                .unwrap();
        }
    }
}

/// Emitted history: owner comments and non-approve findings with notes on earlier plans and
/// review children.
pub(super) fn emit(store: &Store, id: &str, scale: usize, kind: Kind) {
    store
        .update(id, |t| {
            for i in 0..scale + 1 {
                t.comments.push(TaskComment {
                    at: fixture::now(),
                    author: CommentAuthor::Owner,
                    text: format!("Owner note {i:02}"),
                });
            }
            // A non-approve finding on each earlier plan: a prompt's Previous plan findings.
            let count = t.plans.len();
            for plan in t.plans.iter_mut().take(count.saturating_sub(1)) {
                plan.reviews.push(serde_json::from_value(json!({
                    "id": format!("{}-finding", plan.id), "reviewer": "historic",
                    "policy_digest": "0".repeat(64), "verdict": "changes",
                    "notes": format!("Finding on {}", plan.id), "at": fixture::now(),
                }))?);
            }
            // Non-approve findings on earlier runs' review children: a review packet's
            // previous findings, which none of these kinds builds.
            for run in t.runs.iter_mut().filter(|run| run.id.starts_with('H')) {
                for child in &mut run.review_runs {
                    if let Some(review) = child.review.as_mut() {
                        review.verdict = Some(corral_core::Verdict::Changes);
                        review.notes = Some(format!("Finding on {}", child.id));
                    }
                }
            }
            // The repaired run's final obligations, frozen into its newest checkpoint.
            if matches!(kind, Kind::RepairLegacy | Kind::RepairInbox) {
                let id = t.id.clone();
                let run = t
                    .current_run_mut()
                    .ok_or_else(|| anyhow::anyhow!("repair without a run"))?;
                for i in 0..10 {
                    let (wait, decision) = final_obligations(&run.id, i);
                    run.obligations.waits.push(wait);
                    if i == 0 {
                        run.obligations.decisions.push(decision);
                    }
                }
                let frozen = run.obligations.clone();
                let revision = run.checkpoints.len() as u64 + 1;
                let mut record = checkpoint_record(&run.id, revision, &frozen);
                // Named like a saved checkpoint, so the head's newest meta keeps its size.
                record.artifact = format!(
                    "artifacts/{}/{}/{}-checkpoint-{revision}.json",
                    id,
                    run.id,
                    "0".repeat(64)
                );
                run.checkpoints.push(record);
            }
            Ok(())
        })
        .unwrap();
}
