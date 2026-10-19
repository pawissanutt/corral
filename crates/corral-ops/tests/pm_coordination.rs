#![cfg(unix)]

#[path = "operating_launch/fixture.rs"]
mod fixture;

use corral_core::{DecisionRequest, ObligationSource, Status};
use corral_ops::{OperatingStatus, PmActionKind, PmActionOutcome};
use fixture::{Fx, now};

fn decision(fx: &Fx, task: &str) {
    corral_ops::register_decision(
        &fx.ctx,
        task,
        DecisionRequest {
            version: 1,
            id: "scope-choice".into(),
            run: "R1".into(),
            question: "Keep the existing scope?".into(),
            requested_action: "Answer the scope choice".into(),
            owner: "owner".into(),
            source: ObligationSource::Adapter,
            deadline: Some(now()),
        },
        now(),
    )
    .unwrap();
}

#[test]
fn pending_owner_question_does_not_starve_verified_independent_dependency_chain() {
    // Given A's unanswered decision and delegated B -> C, with no approval for future work.
    let mut fx = Fx::new();
    let a = fx.task("owner choice", Status::Ready);
    let b = fx.task("independent", Status::Ready);
    let c = fx.task("successor", Status::Backlog);
    fx.ctx
        .store
        .update(&c.id, |task| {
            task.depends_on.push(b.id.clone());
            Ok(())
        })
        .unwrap();
    fx.grant(&[&a.id, &b.id, &c.id]);
    fx.start(&a.id);
    decision(&fx, &a.id);
    let first = corral_ops::pm_workset(&fx.fresh()).unwrap();
    let question = first.decisions.iter().find(|d| d.task == a.id).unwrap();
    let question_id = question.id.clone();
    corral_ops::pm_present(&fx.ctx, &question_id, "owner-pane:receipt-1", true, now()).unwrap();
    let workset = corral_ops::pm_workset(&fx.fresh()).unwrap();
    assert_eq!(workset.status, OperatingStatus::NeedsYourDecision);
    assert!(workset.independent_work_continues);
    assert_eq!(workset.next.task.as_deref(), Some(b.id.as_str()));
    assert!(!workset.next.requires_owner);
    let action_id = workset
        .actions
        .iter()
        .find(|a| a.action.task.as_deref() == Some(&b.id))
        .unwrap()
        .id
        .clone();

    // When the PM acts and fresh syncs verify both independent candidates.
    assert!(matches!(
        corral_ops::pm_act(&fx.fresh(), &action_id).unwrap().outcome,
        PmActionOutcome::Progressed
    ));
    for (id, file) in [(&b.id, "independent.txt"), (&c.id, "successor.txt")] {
        assert_eq!(fx.ctx.store.get(id).unwrap().status, Status::Running);
        fx.done(id, file);
        corral_ops::sync_at(&fx.fresh(), now()).unwrap();
        let task = fx.ctx.store.get(id).unwrap();
        let review = task.current_run().unwrap().review.as_ref().unwrap();
        corral_ops::verdict_bound(
            &fx.fresh(),
            id,
            corral_core::Verdict::Approve,
            "Independent approval",
            Some(&review.identity.as_ref().unwrap().attempt),
        )
        .unwrap();
        corral_ops::sync_at(&fx.fresh(), now()).unwrap();
        assert_eq!(fx.ctx.store.get(id).unwrap().status, Status::Done);
    }

    // Then restart, expiry and repeated wakes cannot resolve or re-present A.
    for _ in 0..2 {
        let restarted = fx.fresh();
        corral_ops::sync_at(&restarted, now()).unwrap();
        let state = corral_ops::pm_workset(&restarted).unwrap();
        assert_eq!(state.status, OperatingStatus::NeedsYourDecision);
        assert_eq!(state.progress.accepted, 2);
        let pending = state
            .decisions
            .iter()
            .find(|d| d.id == question_id)
            .unwrap();
        assert!(pending.presentation.as_ref().unwrap().confirmed);
        assert_ne!(state.next.kind, PmActionKind::Question);
        assert!(
            restarted
                .store
                .get(&a.id)
                .unwrap()
                .current_run()
                .unwrap()
                .obligations
                .pending_decision()
                .is_some()
        );
    }
    assert!(fx.temp.path().join("app/independent.txt").exists());
    assert!(fx.temp.path().join("app/successor.txt").exists());
}

#[test]
fn stale_or_revoked_action_returns_fresh_state_without_submission() {
    // Given a prepared PM recommendation under a finite grant.
    let mut fx = Fx::new();
    let task = fx.task("delegated", Status::Ready);
    fx.grant(&[&task.id]);
    let current = corral_ops::pm_workset(&fx.ctx).unwrap();
    let action = current.actions.first().unwrap();
    fx.ctx.store.assist_operating_policy().unwrap();
    // When a stale action is delivered from an already-live PM.
    let result = corral_ops::pm_act(&fx.fresh(), &action.id).unwrap();
    // Then neither a submission nor an explicit-owner fallback can occur.
    assert!(matches!(result.outcome, PmActionOutcome::Stale));
    assert!(result.workset.actions.is_empty());
    assert!(fx.records("submissions.jsonl").is_empty());
    assert!(fx.ctx.store.get(&task.id).unwrap().runs.is_empty());
}

#[test]
fn stale_review_action_survives_telemetry_but_not_candidate_changes() {
    // Given an executable repair for an obsolete verdict on a finished candidate.
    let mut fx = Fx::new();
    let task = fx.task("stale review", Status::Ready);
    fx.grant(&[&task.id]);
    fx.start(&task.id);
    fx.done(&task.id, "candidate.txt");
    fx.ctx
        .store
        .update(&task.id, |task| {
            task.current_run_mut().unwrap().review = Some(
                serde_json::from_value(serde_json::json!({
                    "reviewer":"rev", "sha":"old", "at":"2026-10-18T12:00:00Z",
                    "verdict":"approve"
                }))
                .unwrap(),
            );
            Ok(())
        })
        .unwrap();
    let first = corral_ops::pm_workset(&fx.fresh()).unwrap();
    let action = first
        .actions
        .iter()
        .find(|action| action.action.kind == PmActionKind::StaleVerdict)
        .unwrap();
    // When repeated usage refreshes update only the target's runtime telemetry.
    for note in ["first usage refresh", "second usage refresh"] {
        fx.ctx
            .store
            .update(&task.id, |task| {
                task.current_run_mut().unwrap().usage_note = Some(note.into());
                Ok(())
            })
            .unwrap();
        let current = corral_ops::pm_workset(&fx.fresh()).unwrap();
        // Then the same repair remains executable despite task timestamp changes.
        let repair = current
            .actions
            .iter()
            .find(|action| action.action.kind == PmActionKind::StaleVerdict)
            .unwrap();
        assert_eq!(current.policy_revision, first.policy_revision);
        assert_eq!(repair.id, action.id);
    }
    // A different candidate must still invalidate the old repair identity.
    let worktree = fx
        .ctx
        .store
        .get(&task.id)
        .unwrap()
        .current_run()
        .unwrap()
        .worktree
        .clone();
    fixture::git(
        &worktree,
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-qm",
            "new candidate",
        ],
    );
    let result = corral_ops::pm_act(&fx.fresh(), &action.id).unwrap();
    assert!(matches!(result.outcome, PmActionOutcome::Stale));
}

#[test]
fn repeated_readbacks_are_write_free_and_empty_scope_is_not_complete() {
    // Given Assist and a Ready task that has never been authorized.
    let fx = Fx::new();
    fx.task("recommendation", Status::Ready);
    let before = (fx.ctx.store.list().unwrap(), fx.ctx.store.events().unwrap());
    let state_files = std::fs::read_dir(fx.ctx.store.state_dir()).unwrap().count();
    // When all shared readbacks are requested repeatedly.
    for _ in 0..2 {
        let state = corral_ops::pm_workset(&fx.ctx).unwrap();
        assert!(state.actions.is_empty());
        assert_ne!(state.status, OperatingStatus::Complete);
        corral_ops::pm_next(&fx.ctx).unwrap();
        corral_ops::pm_health(&fx.ctx).unwrap();
    }
    // Then permission stays owner-only and there are no presentation or policy writes.
    assert_eq!(
        before,
        (fx.ctx.store.list().unwrap(), fx.ctx.store.events().unwrap())
    );
    assert_eq!(
        state_files,
        std::fs::read_dir(fx.ctx.store.state_dir()).unwrap().count()
    );
}

#[test]
fn uncertain_presentation_survives_reopen_without_becoming_an_answer() {
    // Given a native-independent portable decision.
    let fx = Fx::new();
    let task = fx.task("owner choice", Status::Ready);
    fx.start(&task.id);
    decision(&fx, &task.id);
    let state = corral_ops::pm_workset(&fx.ctx).unwrap();
    let question = state.decisions.first().unwrap();
    // When presentation is unconfirmed and the store is reopened.
    corral_ops::pm_present(&fx.ctx, &question.id, "uncertain-delivery", false, now()).unwrap();
    let reopened = corral_ops::pm_workset(&fx.fresh()).unwrap();
    // Then uncertainty remains visible and cannot be settled via workspace acknowledgement.
    let pending = reopened
        .decisions
        .iter()
        .find(|d| d.id == question.id)
        .unwrap();
    assert!(!pending.presentation.as_ref().unwrap().confirmed);
    assert_eq!(reopened.status, OperatingStatus::NeedsYourDecision);
    assert!(corral_ops::pm_resolve(&fx.fresh(), &question.id, "pretend acknowledgement").is_err());
}

#[test]
fn owner_resolution_is_explicit_and_never_clears_the_underlying_hold() {
    // Given a held task with no worker run to host a portable obligation.
    let mut fx = Fx::new();
    let task = fx.task("owner disposition", Status::Ready);
    fx.ctx
        .store
        .update(&task.id, |task| {
            task.dispatch_hold = true;
            Ok(())
        })
        .unwrap();
    fx.grant(&[&task.id]);
    let workset = corral_ops::pm_workset(&fx.ctx).unwrap();
    let question = workset.decisions.first().unwrap();
    corral_ops::pm_present(&fx.ctx, &question.id, "owner-pane:held", true, now()).unwrap();
    // When the owner gives an actual disposition, rather than a transport acknowledgement.
    corral_ops::pm_resolve(&fx.fresh(), &question.id, "Keep this task held").unwrap();
    // Then the answer persists without pretending that a hold was lifted or work accepted.
    let reopened = corral_ops::pm_workset(&fx.fresh()).unwrap();
    assert!(reopened.decisions.iter().all(|d| d.id != question.id));
    assert!(fx.ctx.store.get(&task.id).unwrap().dispatch_hold);
    assert!(fx.ctx.store.get(&task.id).unwrap().runs.is_empty());
    assert_ne!(reopened.status, OperatingStatus::Complete);
}

#[test]
fn owner_blocker_precedes_git_freshness_and_done_cannot_hide_a_pending_question() {
    // Given a task with a pending question whose worktree has disappeared.
    let mut fx = Fx::new();
    let task = fx.task("owner input", Status::Ready);
    fx.grant(&[&task.id]);
    fx.start(&task.id);
    decision(&fx, &task.id);
    fx.ctx
        .store
        .update(&task.id, |task| {
            task.current_run_mut().unwrap().worktree.push("missing");
            task.status = Status::Done;
            Ok(())
        })
        .unwrap();
    // When read without running any executor or Git repair.
    let workset = corral_ops::pm_workset(&fx.ctx).unwrap();
    // Then the exact question remains visible and no missing-worktree repair replaces it.
    assert_eq!(workset.decisions[0].action.kind, PmActionKind::Question);
    assert_eq!(workset.status, OperatingStatus::NeedsYourDecision);
    assert_eq!(workset.progress.accepted, 0);
    assert_ne!(workset.status, OperatingStatus::Complete);
}

#[test]
fn shared_status_requires_actual_finite_acceptance_and_exposes_pause() {
    // Given an empty Assist workspace, then one exactly delegated task.
    let mut fx = Fx::new();
    assert_eq!(
        corral_ops::pm_workset(&fx.ctx).unwrap().status,
        OperatingStatus::Waiting
    );
    let task = fx.task("finite completion", Status::Ready);
    fx.grant(&[&task.id]);
    assert_eq!(
        corral_ops::pm_workset(&fx.ctx).unwrap().status,
        OperatingStatus::Working
    );
    fx.ctx.store.pause_operating_policy(true).unwrap();
    let paused = corral_ops::pm_workset(&fx.fresh()).unwrap();
    assert_eq!(paused.status, OperatingStatus::NeedsYourDecision);
    assert!(paused.paused);
    assert!(paused.actions.is_empty());
    fx.ctx.store.pause_operating_policy(false).unwrap();
    // When existing executors independently check and accept its real Git candidate.
    corral_ops::sync_at(&fx.fresh(), now()).unwrap();
    fx.done(&task.id, "complete.txt");
    corral_ops::sync_at(&fx.fresh(), now()).unwrap();
    let reviewed = fx.ctx.store.get(&task.id).unwrap();
    let review = reviewed.current_run().unwrap().review.as_ref().unwrap();
    corral_ops::verdict_bound(
        &fx.fresh(),
        &task.id,
        corral_core::Verdict::Approve,
        "Independent approval",
        Some(&review.identity.as_ref().unwrap().attempt),
    )
    .unwrap();
    corral_ops::sync_at(&fx.fresh(), now()).unwrap();
    // Then Complete describes the finite accepted obligation, not merely idle capacity.
    fx.task("future unapproved work", Status::Ready);
    let complete = corral_ops::pm_workset(&fx.fresh()).unwrap();
    assert_eq!(complete.status, OperatingStatus::Complete);
    assert_eq!(complete.progress.total, 1);
    assert_eq!(complete.progress.accepted, 1);
    assert!(complete.decisions.is_empty());
}

#[test]
fn expired_question_identity_survives_idle_and_working_terminal_samples() {
    // Given a presented portable question.
    let fx = Fx::new();
    let task = fx.task("owner input", Status::Ready);
    fx.start(&task.id);
    decision(&fx, &task.id);
    let workset = corral_ops::pm_workset(&fx.ctx).unwrap();
    let id = workset.decisions[0].id.clone();
    corral_ops::pm_present(&fx.ctx, &id, "owner-pane:stable", true, now()).unwrap();
    // When unrelated terminal samples alternate across reopened stores.
    for state in ["idle", "working", "done", "idle"] {
        fx.ctx
            .store
            .update(&task.id, |task| {
                task.current_run_mut().unwrap().agent_state = Some(state.into());
                Ok(())
            })
            .unwrap();
        let workset = corral_ops::pm_workset(&fx.fresh()).unwrap();
        // Then neither the full identity nor the presentation is reset by activity.
        assert_eq!(workset.decisions[0].id, id);
        assert!(
            workset.decisions[0]
                .presentation
                .as_ref()
                .unwrap()
                .confirmed
        );
        assert_eq!(workset.status, OperatingStatus::NeedsYourDecision);
    }
}

fn promotion_fixture(
    second_mode: corral_core::PromoteIdeas,
    include_second: bool,
) -> (tempfile::TempDir, corral_ops::Ctx, [corral_core::Task; 2]) {
    let temp = tempfile::tempdir().unwrap();
    for project in ["app", "other"] {
        std::fs::create_dir_all(temp.path().join(project).join(".git")).unwrap();
    }
    let mut store = corral_core::Store::init(temp.path()).unwrap();
    store.config.limits.running = 3;
    store.config.limits.review = 1;
    store
        .config
        .projects
        .get_mut("app")
        .unwrap()
        .dispatch
        .promote_ideas = Some(corral_core::PromoteIdeas::Auto);
    store
        .config
        .projects
        .get_mut("other")
        .unwrap()
        .dispatch
        .promote_ideas = Some(second_mode);
    store.save_config().unwrap();
    let first = store
        .create(
            "delegated idea",
            Some("app"),
            Status::Idea,
            "## Acceptance\nFinite first task works.",
        )
        .unwrap();
    let second = store
        .create(
            "other idea",
            Some("other"),
            Status::Idea,
            "## Acceptance\nFinite second task works.",
        )
        .unwrap();
    store
        .update(&first.id, |task| {
            task.priority = "P0".into();
            Ok(())
        })
        .unwrap();
    let selection = corral_core::OperatingSelection {
        tasks: if include_second {
            [first.id.clone(), second.id.clone()].into()
        } else {
            [first.id.clone()].into()
        },
        authority: corral_core::OperatingAuthority {
            dispatch: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let preview = store.preview_operating_policy(&selection).unwrap();
    store.config = store
        .confirm_operating_policy(&preview, "fixture owner", now())
        .unwrap();
    (
        temp,
        corral_ops::resources::FixedResourceProbe::ctx(store),
        [first, second],
    )
}

#[test]
fn promotion_options_authorize_each_target_in_mixed_auto_and_suggest_projects() {
    // Given one scoped Auto idea and an unscoped Suggest alternative.
    let (_temp, ctx, [included, excluded]) =
        promotion_fixture(corral_core::PromoteIdeas::Suggest, false);
    // When the shared live card projects both ranked choices.
    let state = corral_ops::pm_workset(&ctx).unwrap();
    let first = state
        .next
        .options
        .iter()
        .find(|option| option.id == format!("promote_{}", included.id))
        .unwrap();
    let other = state
        .next
        .options
        .iter()
        .find(|option| option.id == format!("promote_{}", excluded.id))
        .unwrap();
    // Then only the exact scoped task has a bound automatic command.
    assert!(!first.requires_owner);
    assert!(first.command.contains(" pm act "));
    assert!(other.requires_owner, "unscoped command: {}", other.command);
    assert!(
        state
            .actions
            .iter()
            .all(|a| a.action.task.as_ref() == Some(&included.id))
    );
}

#[test]
fn alternate_promotion_action_is_bound_and_refuses_revoked_scope() {
    // Given two authorized promotion options, including a nonrecommended one.
    let (_temp, mut ctx, [first, second]) =
        promotion_fixture(corral_core::PromoteIdeas::Auto, true);
    let original = corral_ops::pm_workset(&ctx).unwrap();
    let alternate = original
        .actions
        .iter()
        .find(|action| action.action.task.as_ref() == Some(&second.id))
        .expect("the authorized alternative needs its own automatic action");
    let option = original
        .next
        .options
        .iter()
        .find(|option| option.id == format!("promote_{}", second.id))
        .unwrap();
    assert!(
        option
            .command
            .contains(&format!(" pm act {} --json", alternate.id))
    );
    assert_eq!(alternate.action.command, option.command);
    let selection = corral_core::OperatingSelection {
        tasks: [first.id.clone()].into(),
        authority: corral_core::OperatingAuthority {
            dispatch: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let preview = ctx.store.preview_operating_policy(&selection).unwrap();
    ctx.store.config = ctx
        .store
        .confirm_operating_policy(&preview, "fixture owner", now())
        .unwrap();
    // When a delayed alternative arrives after only its target is revoked.
    let result = corral_ops::pm_act(&ctx, &alternate.id).unwrap();
    // Then it is stale, while the still-scoped first option remains available.
    assert!(matches!(result.outcome, PmActionOutcome::Stale));
    assert!(
        result
            .workset
            .actions
            .iter()
            .any(|a| a.action.task.as_ref() == Some(&first.id))
    );
    for task in [&first, &second] {
        let persisted = ctx.store.get(&task.id).unwrap();
        assert_eq!(persisted.status, Status::Idea);
        assert!(persisted.runs.is_empty());
    }
}

#[test]
fn scoped_promotion_remains_available_behind_an_unscoped_higher_ranked_suggestion() {
    // Given a suggested owner-only idea ranked before an authorized Auto idea.
    let (_temp, ctx, [included, excluded]) =
        promotion_fixture(corral_core::PromoteIdeas::Suggest, false);
    ctx.store
        .update(&included.id, |task| {
            task.priority = "P2".into();
            Ok(())
        })
        .unwrap();
    ctx.store
        .update(&excluded.id, |task| {
            task.priority = "P0".into();
            Ok(())
        })
        .unwrap();
    // When the multi-target card's nominal task is outside the finite scope.
    let state = corral_ops::pm_workset(&ctx).unwrap();
    // Then the authorized alternative still has its own scoped action.
    assert_eq!(state.next.task.as_ref(), Some(&included.id));
    assert!(!state.next.requires_owner);
    assert!(
        state
            .actions
            .iter()
            .any(|a| a.action.task.as_ref() == Some(&included.id))
    );
}
