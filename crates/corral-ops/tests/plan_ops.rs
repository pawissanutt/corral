use std::path::PathBuf;
use std::sync::{Arc, Barrier};

use corral_core::{AgentCmd, CriteriaManifest, Criterion, PlanReview, Status, Store, Verdict};
use corral_ops::{Ctx, DispatchOpts, PlanVerdict, approved_plan, plan_submit, plan_verdict};

const PLAN: &str = "---\ngoal: Make the widget spin\nnon_goals: []\ndecisions: []\naffected_paths: [widget.rs]\ncontracts: []\ndependencies: []\nacceptance:\n  - criterion: C1\n    scenario: widget --spin\n    expected: clockwise\nverification: [cargo test]\nowner_decisions: []\n---\nImplementation details.\n";

fn fixture() -> (tempfile::TempDir, Ctx, String, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.review.plan_required = Some(true);
    store.config.agents.insert(
        "reviewer".into(),
        AgentCmd {
            command: "true".into(),
        },
    );
    store.config.review.reviewer = Some("reviewer".into());
    store.save_config().unwrap();
    let task = store
        .create("Spin widget", None, Status::Ready, "Spin clockwise.")
        .unwrap();
    let path = tmp.path().join("plan.md");
    std::fs::write(&path, PLAN).unwrap();
    (tmp, Ctx::from_store(store), task.id, path)
}

fn pending(ctx: &Ctx, id: &str, path: &std::path::Path) -> String {
    let plan = plan_submit(ctx, id, path).unwrap();
    // Seed a launched attempt at the store seam; real reviewer launch is covered by live CLI QA.
    ctx.store
        .update(id, |task| {
            task.plans.last_mut().unwrap().reviews.push(PlanReview {
                id: "PR1".into(),
                reviewer: "reviewer".into(),
                policy_digest: String::new(),
                verdict: None,
                notes: None,
                at: jiff::Timestamp::now(),
                herdr: None,
                pending_reply: None,
            });
            Ok(())
        })
        .unwrap();
    plan.id
}

#[test]
fn missing_plan_blocks_before_project_or_terminal_side_effects() {
    let (_tmp, ctx, id, _path) = fixture();
    let task = ctx.store.get(&id).unwrap();
    assert!(
        approved_plan(&ctx, &task)
            .unwrap_err()
            .to_string()
            .contains("plan required")
    );
    assert!(task.runs.is_empty());
    assert!(!task.queued);
}

#[test]
fn submitted_plan_is_pending_and_snapshots_exact_bytes_without_coding_run() {
    let (_tmp, ctx, id, path) = fixture();
    let plan = plan_submit(&ctx, &id, &path).unwrap();
    let task = ctx.store.get(&id).unwrap();
    assert!(
        approved_plan(&ctx, &task)
            .unwrap_err()
            .to_string()
            .contains("pending")
    );
    assert_eq!(
        std::fs::read(ctx.store.dir().join(plan.artifact.path)).unwrap(),
        PLAN.as_bytes()
    );
    assert!(task.runs.is_empty());
    assert_eq!(task.status, Status::Ready);
}

#[test]
fn source_edit_invalidates_plan_before_verdict() {
    let (_tmp, ctx, id, path) = fixture();
    let plan = plan_submit(&ctx, &id, &path).unwrap();
    std::fs::write(&path, format!("{PLAN}Changed decisions.\n")).unwrap();
    let error = plan_verdict(
        &ctx,
        &id,
        &PlanVerdict {
            plan: &plan.id,
            attempt: None,
            verdict: Verdict::Approve,
            notes: "Approved",
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("plan bytes changed"));
    assert!(ctx.store.get(&id).unwrap().plans[0].reviews.is_empty());
}

#[test]
fn same_bytes_resubmitted_from_new_path_use_new_source() {
    let (tmp, mut ctx, id, path) = fixture();
    ctx.store.config.review.reviewer = Some("none".into());
    ctx.store.save_config().unwrap();
    plan_submit(&ctx, &id, &path).unwrap();
    let replacement = tmp.path().join("replacement.md");
    std::fs::write(&replacement, PLAN).unwrap();
    let plan = plan_submit(&ctx, &id, &replacement).unwrap();
    std::fs::remove_file(path).unwrap();
    plan_verdict(
        &ctx,
        &id,
        &PlanVerdict {
            plan: &plan.id,
            attempt: None,
            verdict: Verdict::Approve,
            notes: "Review the replacement",
        },
    )
    .unwrap();
    assert_eq!(
        approved_plan(&ctx, &ctx.store.get(&id).unwrap())
            .unwrap()
            .unwrap()
            .id,
        plan.id
    );
}

#[test]
fn changed_criteria_invalidate_plan_revision() {
    let (_tmp, ctx, id, path) = fixture();
    let plan = plan_submit(&ctx, &id, &path).unwrap();
    ctx.store
        .update(&id, |t| {
            t.completion = Some(CriteriaManifest {
                version: 1,
                spec_digest: corral_ops::completion_spec_digest(t)?,
                reviewed_by: "owner".into(),
                audited: true,
                require_gate: false,
                criteria: vec![Criterion {
                    id: "C1".into(),
                    required: true,
                    live: true,
                    scenario: "widget --reverse".into(),
                    expected: "counterclockwise".into(),
                }],
                owner_decisions: vec![],
            });
            Ok(())
        })
        .unwrap();
    let error = plan_verdict(
        &ctx,
        &id,
        &PlanVerdict {
            plan: &plan.id,
            attempt: None,
            verdict: Verdict::Approve,
            notes: "Approved",
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("spec or criteria changed"));
}

#[test]
fn replaced_plan_rejects_old_callback_and_preserves_history() {
    let (_tmp, ctx, id, path) = fixture();
    let first = pending(&ctx, &id, &path);
    let second = plan_submit(&ctx, &id, &path).unwrap();
    let error = plan_verdict(
        &ctx,
        &id,
        &PlanVerdict {
            plan: &first,
            attempt: Some("PR1"),
            verdict: Verdict::Approve,
            notes: "Old callback",
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("replaced"));
    let task = ctx.store.get(&id).unwrap();
    assert_eq!(task.plans.len(), 2);
    assert_eq!(task.plans[0].reviews.len(), 1);
    assert_eq!(task.plans[1].id, second.id);
    assert!(task.plans[1].reviews.is_empty());
}

#[test]
fn manual_owner_cannot_impersonate_policy_reviewer() {
    let (_tmp, ctx, id, path) = fixture();
    let plan = plan_submit(&ctx, &id, &path).unwrap();
    let verdict = plan_verdict(
        &ctx,
        &id,
        &PlanVerdict {
            plan: &plan.id,
            attempt: None,
            verdict: Verdict::Approve,
            notes: "Owner call",
        },
    )
    .unwrap();
    assert_eq!(verdict.verdict, Some(Verdict::Escalate));
    assert_eq!(verdict.reviewer, "owner");
    let task = ctx.store.get(&id).unwrap();
    assert_eq!(task.review, None);
    assert!(task.runs.is_empty());
    assert_eq!(ctx.store.list().unwrap().len(), 1);
}

#[test]
fn unresolved_owner_decision_escalates_even_manual_owner_approval() {
    let (_tmp, mut ctx, id, path) = fixture();
    ctx.store.config.review.reviewer = Some("none".into());
    ctx.store.save_config().unwrap();
    std::fs::write(
        &path,
        PLAN.replace("owner_decisions: []", "owner_decisions: [Choose storage]"),
    )
    .unwrap();
    let plan = plan_submit(&ctx, &id, &path).unwrap();
    let verdict = plan_verdict(
        &ctx,
        &id,
        &PlanVerdict {
            plan: &plan.id,
            attempt: None,
            verdict: Verdict::Approve,
            notes: "Approve",
        },
    )
    .unwrap();
    assert_eq!(verdict.verdict, Some(Verdict::Escalate));
}

#[test]
fn owner_approval_allows_only_explicit_dispatch_and_leaves_acceptance_unchanged() {
    let (_tmp, mut ctx, id, path) = fixture();
    ctx.store.config.review.reviewer = Some("none".into());
    ctx.store.save_config().unwrap();
    ctx.store
        .update(&id, |t| {
            t.queued = true;
            Ok(())
        })
        .unwrap();
    let plan = plan_submit(&ctx, &id, &path).unwrap();
    let verdict = plan_verdict(
        &ctx,
        &id,
        &PlanVerdict {
            plan: &plan.id,
            attempt: None,
            verdict: Verdict::Approve,
            notes: "No owner decisions",
        },
    )
    .unwrap();
    assert_eq!(verdict.verdict, Some(Verdict::Approve));
    let task = ctx.store.get(&id).unwrap();
    assert_eq!(approved_plan(&ctx, &task).unwrap().unwrap().id, plan.id);
    assert_eq!(task.status, Status::Ready);
    assert!(!task.queued, "approval requires a new explicit dispatch");
    assert!(task.runs.is_empty());
    assert!(corral_ops::verdict(&ctx, &id, Verdict::Approve, "not implementation").is_err());
    assert!(
        corral_ops::approve(&ctx, &id, corral_ops::ApproveStrategy::Rebase, false, false).is_err()
    );
}

#[test]
fn changed_reviewer_policy_invalidates_approval() {
    let (_tmp, mut ctx, id, path) = fixture();
    ctx.store.config.review.reviewer = Some("none".into());
    ctx.store.save_config().unwrap();
    let plan = plan_submit(&ctx, &id, &path).unwrap();
    plan_verdict(
        &ctx,
        &id,
        &PlanVerdict {
            plan: &plan.id,
            attempt: None,
            verdict: Verdict::Approve,
            notes: "Approve",
        },
    )
    .unwrap();
    ctx.store.config.review.reviewer = Some("reviewer".into());
    ctx.store.save_config().unwrap();
    let error = approved_plan(&ctx, &ctx.store.get(&id).unwrap()).unwrap_err();
    assert!(error.to_string().contains("reviewer policy changed"));
}

#[test]
fn concurrent_callback_cannot_overwrite_first_verdict() {
    let (_tmp, mut ctx, id, path) = fixture();
    ctx.store.config.review.reviewer = Some("none".into());
    ctx.store.save_config().unwrap();
    let plan = plan_submit(&ctx, &id, &path).unwrap();
    let owner = plan_verdict(
        &ctx,
        &id,
        &PlanVerdict {
            plan: &plan.id,
            attempt: None,
            verdict: Verdict::Approve,
            notes: "Initial owner review",
        },
    )
    .unwrap();
    // Reopen the exact attempt only as a fixture for two simultaneous callbacks.
    ctx.store
        .update(&id, |t| {
            t.plans[0].reviews[0].verdict = None;
            Ok(())
        })
        .unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let handles: Vec<_> = [Verdict::Approve, Verdict::Changes]
        .into_iter()
        .map(|verdict| {
            let (ctx, id, plan, attempt, barrier) = (
                ctx.clone(),
                id.clone(),
                plan.id.clone(),
                owner.id.clone(),
                barrier.clone(),
            );
            std::thread::spawn(move || {
                barrier.wait();
                plan_verdict(
                    &ctx,
                    &id,
                    &PlanVerdict {
                        plan: &plan,
                        attempt: Some(&attempt),
                        verdict,
                        notes: "Concurrent callback",
                    },
                )
            })
        })
        .collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(ctx.store.get(&id).unwrap().plans[0].reviews.len(), 1);
}

#[test]
fn plan_requires_registered_criterion_ids_when_manifest_enabled() {
    let (_tmp, ctx, id, path) = fixture();
    let task = ctx.store.get(&id).unwrap();
    corral_ops::register_criteria(
        &ctx,
        &id,
        &CriteriaManifest {
            version: 1,
            spec_digest: corral_ops::completion_spec_digest(&task).unwrap(),
            reviewed_by: "owner".into(),
            audited: true,
            require_gate: false,
            criteria: vec![Criterion {
                id: "C2".into(),
                required: true,
                live: false,
                scenario: "widget --spin".into(),
                expected: "clockwise".into(),
            }],
            owner_decisions: vec![],
        },
    )
    .unwrap();
    assert!(
        plan_submit(&ctx, &id, &path)
            .unwrap_err()
            .to_string()
            .contains("unknown criterion C1")
    );
}

#[test]
fn enabled_policy_refuses_dispatch_without_consuming_run_slot() {
    let (_tmp, mut ctx, id, _path) = fixture();
    ctx.store.config.projects.insert(
        "app".into(),
        corral_core::Project {
            artifact_retention: None,
            post_land: Vec::new(),
            path: ".".into(),
            default_profile: None,
            base: None,
            review: Default::default(),
        },
    );
    ctx.store.save_config().unwrap();
    ctx.store
        .update(&id, |t| {
            t.project = Some("app".into());
            Ok(())
        })
        .unwrap();
    let error = corral_ops::dispatch(
        &ctx,
        &id,
        &DispatchOpts {
            profile: Some("reviewer".into()),
            queue_if_blocked: true,
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("plan required"));
    assert_eq!(corral_ops::slots(&ctx).unwrap().0, 0);
    assert!(ctx.store.get(&id).unwrap().runs.is_empty());
    assert!(!ctx.store.get(&id).unwrap().queued);
}
