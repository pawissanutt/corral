#![cfg(unix)] // Review fixtures execute plan_engine.sh directly.

use std::path::PathBuf;
use std::sync::{Arc, Barrier};

use corral_core::{AgentCmd, CriteriaManifest, Criterion, PlanReview, Status, Store, Verdict};
use corral_ops::{Ctx, DispatchOpts, PlanVerdict, approved_plan, plan_submit, plan_verdict};

const PLAN: &str = "---\ngoal: Make the widget spin\nnon_goals: []\ndecisions: []\naffected_paths: [widget.rs]\ncontracts: []\ndependencies: []\nacceptance:\n  - criterion: C1\n    scenario: widget --spin\n    expected: clockwise\nverification: [cargo test]\nowner_decisions: []\n---\nImplementation details.\n";

#[path = "plan_ops/acceptance_recovery.rs"]
mod acceptance_recovery;
#[path = "plan_ops/authorization.rs"]
mod authorization;
#[path = "plan_ops/operating_decisions.rs"]
mod operating_decisions;
#[path = "plan_ops/self_projection_recovery.rs"]
mod self_projection_recovery;

fn fixture() -> (tempfile::TempDir, Ctx, String, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    // Plan recovery exercises persisted policy, not the starter routing/pricing catalog.
    let mut store = Store::init_with(tmp.path(), |store| {
        std::fs::write(
            store.config_path(),
            "[projects.app]\npath = \".\"\n[agents.omo]\ncommand = \"true\"\n",
        )?;
        Ok(())
    })
    .unwrap();
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
        .create("Spin widget", Some("app"), Status::Ready, "Spin clockwise.")
        .unwrap();
    let path = tmp.path().join("plan.md");
    std::fs::write(&path, PLAN).unwrap();
    let engine = tmp.path().join("engine");
    std::fs::create_dir(&engine).unwrap();
    let bin = engine.join("herdr");
    std::fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/plan_engine.sh"),
        &bin,
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut ctx = corral_ops::resources::FixedResourceProbe::ctx(store);
    ctx.herdr = corral_herdr::Herdr::with_bin(bin, Some("corral-test".into()));
    (tmp, ctx, task.id, path)
}

fn pending(ctx: &Ctx, id: &str, path: &std::path::Path) -> String {
    let plan = plan_submit(ctx, id, path).unwrap();
    seed_review(ctx, id, &plan.id);
    plan.id
}

fn seed_review(ctx: &Ctx, id: &str, plan: &str) {
    let task = ctx.store.get(id).unwrap();
    let digest =
        corral_core::plan_policy::policy_digest(&ctx.store.reopen().unwrap().config, &task)
            .unwrap();
    // Seed the pending reviewer at the store seam, without launching and undoing a planner
    // merely to obtain its policy digest. Real reviewer launch is covered by live CLI QA.
    ctx.store
        .update(id, |task| {
            task.plans
                .iter_mut()
                .find(|record| record.id == plan)
                .unwrap()
                .reviews = vec![PlanReview {
                delivery: corral_core::inbox::DeliveryMode::Legacy,
                inbox: Default::default(),
                route: None,
                id: "PR1".into(),
                reviewer: "reviewer".into(),
                policy_digest: digest,
                verdict: None,
                notes: None,
                at: jiff::Timestamp::now(),
                herdr: None,
                pending_reply: None,
            }];
            Ok(())
        })
        .unwrap();
}

fn confirm_plan_authority(store: &Store, id: &str) {
    let preview = store
        .preview_operating_policy(&corral_core::OperatingSelection {
            tasks: std::collections::BTreeSet::from([id.into()]),
            authority: corral_core::OperatingAuthority {
                dispatch: true,
                plan_accept: true,
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
    store
        .confirm_operating_policy(&preview, "owner", "2026-10-18T08:00:00Z".parse().unwrap())
        .unwrap();
}

#[test]
fn reviewer_approval_waits_for_explicit_owner_acceptance() {
    let (_tmp, ctx, id, path) = fixture();
    let plan = pending(&ctx, &id, &path);
    plan_verdict(
        &ctx,
        &id,
        &PlanVerdict {
            plan: &plan,
            attempt: Some("PR1"),
            verdict: Verdict::Approve,
            notes: "Independent review passed",
        },
    )
    .unwrap();
    let task = ctx.store.get(&id).unwrap();
    assert!(task.plans[0].approval.is_none());
    assert_eq!(task.status, Status::NeedsMe);
    assert!(
        approved_plan(&ctx, &task)
            .unwrap_err()
            .to_string()
            .contains("owner approval")
    );
    plan_verdict(
        &ctx,
        &id,
        &PlanVerdict {
            plan: &plan,
            attempt: None,
            verdict: Verdict::Approve,
            notes: "Owner accepts",
        },
    )
    .unwrap();
    let task = ctx.store.get(&id).unwrap();
    assert_eq!(task.plans[0].approval.as_ref().unwrap().by, "owner");
    assert!(approved_plan(&ctx, &task).is_ok());
}

#[test]
fn unsupported_replacement_is_rejected_before_owner_or_auto_acceptance() {
    for auto in [false, true] {
        for barrier in ["children", "implementation", "launch", "foreign", "journal"] {
            let (tmp, mut ctx, id, path) = fixture();
            let first = pending(&ctx, &id, &path);
            for attempt in [Some("PR1"), None] {
                plan_verdict(
                    &ctx,
                    &id,
                    &PlanVerdict {
                        plan: &first,
                        attempt,
                        verdict: Verdict::Approve,
                        notes: "Accept initial plan",
                    },
                )
                .unwrap();
            }
            if auto {
                ctx.store.config.review.plan_accept = Some(corral_core::Accept::Auto);
                ctx.store.save_config().unwrap();
            }
            let second = plan_submit(&ctx, &id, &path).unwrap().id;
            ctx.store.update(&id, |task| {
                match barrier {
                    "children" => task.children.push("T-999".into()),
                    "implementation" => task.runs.push(serde_json::from_value(serde_json::json!({
                        "id": "R1", "agent": "reviewer", "worktree": tmp.path(), "branch": "fixture",
                        "base": "main", "base_sha": "fixture", "started": jiff::Timestamp::now(),
                    })).unwrap()),
                    "launch" => task.launch = Some(corral_core::LaunchReservation {
                        authorization: None,
                        approved_plan: None, socket: None, stage: corral_core::LaunchStage::Reserved,
                        reserved: true, error: None,
                    }),
                    "foreign" => task.plan = Some("T-999@P1".into()),
                    "journal" => std::fs::write(ctx.store.state_dir().join(format!("plan-decompose-{id}.json")), "{}").unwrap(),
                    _ => unreachable!(),
                }
                Ok(())
            }).unwrap();
            seed_review(&ctx, &id, &second);
            if auto {
                confirm_plan_authority(&ctx.store, &id);
            }
            if !auto {
                plan_verdict(
                    &ctx,
                    &id,
                    &PlanVerdict {
                        plan: &second,
                        attempt: Some("PR1"),
                        verdict: Verdict::Approve,
                        notes: "Review replacement",
                    },
                )
                .unwrap();
            }
            let before = ctx.store.get(&id).unwrap();
            let result = plan_verdict(
                &ctx,
                &id,
                &PlanVerdict {
                    plan: &second,
                    attempt: auto.then_some("PR1"),
                    verdict: Verdict::Approve,
                    notes: "Accept replacement",
                },
            );
            let after = ctx.store.get(&id).unwrap();
            if auto {
                // Independent review remains a collected receipt even when the
                // separate automatic acceptance refuses an illegal attachment.
                assert_eq!(result.unwrap().verdict, Some(Verdict::Approve));
                assert!(after.plans.last().unwrap().approval.is_none());
            } else {
                assert!(result.is_err(), "{barrier}, auto={auto}");
                assert_eq!(after.plans, before.plans, "{barrier}, auto={auto}");
            }
            assert_eq!(after.plan_runs, before.plan_runs);
            assert_eq!(after.plan, before.plan);
        }
    }
}

#[test]
fn owner_changes_can_replace_an_earlier_accepted_self_link() {
    let (_tmp, ctx, id, path) = fixture();
    let first = pending(&ctx, &id, &path);
    for attempt in [Some("PR1"), None] {
        plan_verdict(
            &ctx,
            &id,
            &PlanVerdict {
                plan: &first,
                attempt,
                verdict: Verdict::Approve,
                notes: "Initial plan",
            },
        )
        .unwrap();
    }
    plan_verdict(
        &ctx,
        &id,
        &PlanVerdict {
            plan: &first,
            attempt: None,
            verdict: Verdict::Changes,
            notes: "Replace this design",
        },
    )
    .unwrap();
    assert!(ctx.store.get(&id).unwrap().plans[0].approval.is_none());
    let second = pending(&ctx, &id, &path);
    for attempt in [Some("PR1"), None] {
        plan_verdict(
            &ctx,
            &id,
            &PlanVerdict {
                plan: &second,
                attempt,
                verdict: Verdict::Approve,
                notes: "Replacement plan",
            },
        )
        .unwrap();
    }
    let task = ctx.store.get(&id).unwrap();
    assert_eq!(task.plan, Some(format!("{id}@{second}")));
    assert_eq!(approved_plan(&ctx, &task).unwrap().unwrap().id, second);
    assert_eq!(task.plans.len(), 2);
}

#[test]
fn automatic_plan_acceptance_is_separate_from_implementation_acceptance() {
    let (_tmp, mut ctx, id, path) = fixture();
    ctx.store.config.review.plan_accept = Some(corral_core::Accept::Auto);
    ctx.store.save_config().unwrap();
    let plan = pending(&ctx, &id, &path);
    confirm_plan_authority(&ctx.store, &id);
    plan_verdict(
        &ctx,
        &id,
        &PlanVerdict {
            plan: &plan,
            attempt: Some("PR1"),
            verdict: Verdict::Approve,
            notes: "Independent review passed",
        },
    )
    .unwrap();
    let task = ctx.store.get(&id).unwrap();
    assert_eq!(task.plans[0].approval.as_ref().unwrap().by, "reviewer");
    assert!(task.runs.is_empty());
    assert!(approved_plan(&ctx, &task).is_ok());
    ctx.store.config.review.plan_accept = Some(corral_core::Accept::Owner);
    ctx.store.save_config().unwrap();
    assert!(approved_plan(&ctx, &ctx.store.get(&id).unwrap()).is_err());
}

fn planning_run(ctx: &Ctx, id: &str) {
    ctx.store
        .update(id, |task| {
            task.plan_runs.push(corral_core::PlanRun {
                execution: None,
                route: None,
                delivery: corral_core::inbox::DeliveryMode::Legacy,
                inbox: Default::default(),
                id: "PL1".into(),
                profile: "gpt-sol".into(),
                harness: "omo".into(),
                model: Some("fixture-planner".into()),
                vendor: Some("openai".into()),
                effort: None,
                spec_revision: corral_ops::plan_spec_revision(task)?,
                started: jiff::Timestamp::now(),
                ended: None,
                herdr: None,
                output_dir: "plan-output".into(),
                submitted: None,
                rounds: 0,
                state: corral_core::PlanRunState::Planning,
                pending_reply: None,
            });
            Ok(())
        })
        .unwrap();
}

#[test]
fn submission_binds_current_planner_and_rejects_stale_planner_revision() {
    let (_tmp, ctx, id, path) = fixture();
    planning_run(&ctx, &id);
    let plan = plan_submit(&ctx, &id, &path).unwrap();
    let task = ctx.store.get(&id).unwrap();
    assert_eq!(plan.run.as_deref(), Some("PL1"));
    assert_eq!(task.status, Status::Review);
    assert_eq!(
        task.plan_runs[0].state,
        corral_core::PlanRunState::Submitted
    );
    assert_eq!(task.plan_runs[0].submitted.as_deref(), Some("P1"));
    assert!(task.plan_runs[0].ended.is_some());
    ctx.store
        .update(&id, |t| {
            t.plan_runs[0].state = corral_core::PlanRunState::Changes;
            t.body.push_str(" Different spec.");
            Ok(())
        })
        .unwrap();
    assert!(
        plan_submit(&ctx, &id, &path)
            .unwrap_err()
            .to_string()
            .contains("stale planner")
    );
    assert_eq!(ctx.store.get(&id).unwrap().plans.len(), 1);
}

#[test]
fn owner_resolves_each_plan_decision_explicitly_in_notes() {
    let (_tmp, ctx, id, path) = fixture();
    std::fs::write(
        &path,
        PLAN.replace("owner_decisions: []", "owner_decisions: [Choose storage]"),
    )
    .unwrap();
    let plan = pending(&ctx, &id, &path);
    plan_verdict(
        &ctx,
        &id,
        &PlanVerdict {
            plan: &plan,
            attempt: Some("PR1"),
            verdict: Verdict::Approve,
            notes: "Review passed; owner decides storage",
        },
    )
    .unwrap();
    plan_verdict(
        &ctx,
        &id,
        &PlanVerdict {
            plan: &plan,
            attempt: None,
            verdict: Verdict::Approve,
            notes: "Resolved: Choose storage => SQLite",
        },
    )
    .unwrap();
    assert!(approved_plan(&ctx, &ctx.store.get(&id).unwrap()).is_ok());
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
fn changed_family_requirement_invalidates_prior_plan_approval() {
    // Given a manual plan approved under the default family policy.
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
    // When only the reviewer diversity requirement changes.
    ctx.store.config.review.require_cross_family = Some(true);
    ctx.store.save_config().unwrap();
    // Then the previous approval cannot authorize dispatch under the new policy.
    assert!(approved_plan(&ctx, &ctx.store.get(&id).unwrap()).is_err());
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
fn enabled_policy_dispatches_plan_without_implementation_candidate() {
    let (_tmp, mut ctx, id, _path) = fixture();
    ctx.store.config.projects.insert(
        "app".into(),
        corral_core::Project {
            dispatch: Default::default(),
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
    let outcome = corral_ops::dispatch(
        &ctx,
        &id,
        &DispatchOpts {
            profile: Some("reviewer".into()),
            queue_if_blocked: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(matches!(
        outcome,
        corral_ops::DispatchOutcome::Started { .. }
    ));
    assert_eq!(corral_ops::slots(&ctx).unwrap().0, 1);
    let task = ctx.store.get(&id).unwrap();
    assert!(task.runs.is_empty());
    assert_eq!(task.plan_runs.len(), 1);
    assert!(!task.queued);
}
