//! Aggregate paths use the same real import, gate, evidence and review entries as plan tests.
use super::*;
use corral_core::{AggregateStrategy, Origin, Run, Verdict};
use serde_json::json;

pub(super) fn scenario(code: bool, automatic: bool) -> (Fx, String, String, String) {
    let mut fx = Fx::new();
    let source = fx.temp.path().join("aggregate.md");
    let target = if code {
        "\n  delivery_target: {kind: integration, branch: feature/submission}"
    } else {
        ""
    };
    std::fs::write(&source, format!(
        "---\ntitle: Aggregate\nproject: app\napproved_scope:\n  goal: Combined regression\n  acceptance:\n    - {{criterion: C1, scenario: combined, expected: verified}}{target}\ntasks:\n  - {{key: member, title: Member}}\n---\n"
    )).unwrap();
    let imported = crate::import_plan(&fx.ctx, &source, false, Origin::Agent).unwrap();
    let member = &imported.tasks[0];
    let repo = fx.ctx.store.project_path("app").unwrap();
    let before = fixture::git(&repo, &["rev-parse", "main"]);
    if code {
        let _lock = corral_git::landing_lock(&repo).unwrap();
        let (mut target, binding) = crate::plan_target::prepare_dispatch(&fx.ctx, member)
            .unwrap()
            .unwrap();
        let worktree = repo.join(".worktrees").join(&member.id);
        let branch = format!("corral/{}", member.id);
        corral_git::worktree_add(&repo, &worktree, &branch, &target.head).unwrap();
        std::fs::write(worktree.join("candidate.txt"), "combined regression\n").unwrap();
        corral_git::commit_all(&worktree, "candidate").unwrap();
        let head = corral_git::rev_sha(&worktree, "HEAD").unwrap();
        let run: Run = serde_json::from_value(json!({
            "id":"R1", "agent":"fake", "worktree":worktree, "branch":branch,
            "base":"feature/submission", "base_sha":target.head, "plan_target":binding,
            "started":"2026-10-18T00:00:00Z", "outcome":"done"
        }))
        .unwrap();
        let task = fx
            .ctx
            .store
            .update(&member.id, |t| {
                t.status = Status::Review;
                t.runs.push(run);
                Ok(())
            })
            .unwrap();
        target
            .prepare_landing(
                &task,
                task.current_run().unwrap(),
                &head,
                "fast_forward",
                None,
                crate::plan_target::LandingOptions {
                    contract: &corral_core::operating_contract_revision(&task).unwrap(),
                    remove_worktree: false,
                    keep_branch: false,
                    started: fixture::now(),
                },
            )
            .unwrap();
        corral_git::land_checked(
            &target.checkout,
            &worktree,
            &branch,
            "feature/submission",
            &head,
            &target.head,
        )
        .unwrap();
        target.reconcile(&fx.ctx).unwrap();
    } else {
        fx.ctx
            .store
            .update(&member.id, |t| {
                t.status = Status::Done;
                t.runs.push(serde_json::from_value(json!({
                    "id":"R1", "agent":"fake", "worktree":repo, "branch":"main",
                    "base":"main", "base_sha":before,
                    "started":"2026-10-18T00:00:00Z", "outcome":"done"
                }))?);
                Ok(())
            })
            .unwrap();
        fx.ctx
            .store
            .log_event(
                Some(&member.id),
                "approved",
                json!({"run":"R1","sha":before,"strategy":"fast_forward","base":"main"}),
            )
            .unwrap();
    }
    let id = imported.tracker.id;
    let attempt = verified_attempt(&mut fx, &id, automatic);
    (fx, id, attempt, before)
}

#[test]
fn submission_paths_aggregate_code_and_no_code_land_exactly_once() {
    for code in [false, true] {
        for automatic in [false, true] {
            let (mut fx, id, attempt, before) = scenario(code, automatic);
            let stages = Arc::new(Mutex::new(Vec::new()));
            let captured = stages.clone();
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
                if !point.starts_with("effect_")
                    && !matches!(point, "submission_entered" | "refused_committed")
                {
                    captured.lock().unwrap().push(point.to_owned());
                }
            }));
            let transport_before = counts(&fx);
            let result = crate::plan_accept(&fx.ctx, &id, &attempt, automatic).unwrap();
            assert_eq!(
                result.strategy,
                if code {
                    AggregateStrategy::FastForward
                } else {
                    AggregateStrategy::NoCode
                }
            );
            assert_eq!(
                *stages.lock().unwrap(),
                [
                    "prepare_returned",
                    "authorization_committed",
                    "send_returned",
                    "submitted_committed"
                ]
            );
            assert_eq!(counts(&fx), transport_before);
            let repo = fx.ctx.store.project_path("app").unwrap();
            let after = fixture::git(&repo, &["rev-parse", "main"]);
            assert_eq!(after == before, !code);
            assert_eq!(fx.ctx.store.get(&id).unwrap().status, Status::Done);
            let record = authorizations(&fx.ctx, &id, OperatingActionKind::AcceptAggregate)
                .pop()
                .unwrap();
            assert_eq!(record.disposition, SubmissionDisposition::Submitted);
            assert_eq!(
                crate::plan_accept(&fx.ctx, &id, &attempt, automatic).unwrap(),
                result
            );
            assert_eq!(fixture::git(&repo, &["rev-parse", "main"]), after);
            assert_eq!(
                stages.lock().unwrap().len(),
                4,
                "retry resubmitted acceptance"
            );
        }
    }
}

/// Grants automatic aggregate acceptance on `id` when `automatic`, then verifies the Plan,
/// records the combined scenario's evidence and an approving aggregate review: the attempt.
pub(super) fn verified_attempt(fx: &mut Fx, id: &str, automatic: bool) -> String {
    let id = id.to_owned();
    if automatic {
        let selection = corral_core::OperatingSelection {
            tasks: [id.clone()].into(),
            authority: corral_core::OperatingAuthority {
                aggregate_accept: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let preview = fx.ctx.store.preview_operating_policy(&selection).unwrap();
        fx.ctx.store.config = fx
            .ctx
            .store
            .confirm_operating_policy(&preview, "fixture owner", fixture::now())
            .unwrap();
    }
    let attempt = crate::plan_verify(&fx.ctx, &id).unwrap();
    let observation = serde_json::from_value(json!({
        "version":1,"key":"submission-C1","criterion":"C1","run":"aggregate",
        "attempt":attempt.id,"status":"pass","action":"run combined scenario",
        "environment":"fixture","expected":"verified","observed":"verified",
        "artifacts":[],"captured":corral_core::clock::now(),"producer":"test","live":false
    }))
    .unwrap();
    crate::plan_evidence(&fx.ctx, &id, &observation).unwrap();
    let review = crate::plan_aggregate_review(&fx.ctx, &id, &attempt.id).unwrap();
    crate::plan_aggregate_verdict(
        &fx.ctx,
        &id,
        &crate::AggregateVerdict {
            attempt: &attempt.id,
            review: &review.id,
            verdict: Verdict::Approve,
            notes: "verified",
        },
    )
    .unwrap();
    attempt.id
}
