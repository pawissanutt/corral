//! Deterministic journal contract at Git-advanced/task-marker-absent, not a process-crash test.
use super::*;
use crate::gate_tier::GatePhase;

fn landing_commands() -> usize {
    let path = std::env::var_os("GIT_TRACE2_EVENT").unwrap();
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter(|line| {
            let event: serde_json::Value = serde_json::from_str(line).unwrap();
            event["event"] == "start"
                && event["argv"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|arg| matches!(arg.as_str(), Some("merge" | "rebase" | "update-ref")))
        })
        .count()
}

#[test]
fn exact_landing_journal_reconciles_without_the_task_commit_marker() {
    if isolated("landing::exact_landing_journal_reconciles_without_the_task_commit_marker") {
        return;
    }
    // Both journal kinds must preserve cancellation at the Git/task commit boundary.
    for (plan_target, cancelled) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut f = crate::gate_tier::tests::fixture(false);
        let hook_marker = f.repo.parent().unwrap().join("hook-must-not-run");
        f.ctx.store.config.projects.get_mut("app").unwrap().post_land.push(serde_json::from_value(serde_json::json!({
            "name":"no-replay", "argv":["touch", hook_marker], "cwd":".", "env":{}, "timeout_secs":1, "max_attempts":1
        })).unwrap());
        f.ctx.store.save_config().unwrap();
        let (task, plan) = if plan_target {
            let (mut members, plan) = f.members(1);
            (members.remove(0), Some(plan))
        } else if cancelled {
            let (member, plan) = f.main_plan_member();
            (member, Some(plan))
        } else {
            (f.main_bound(), None)
        };
        let id = task.id;
        f.ctx
            .store
            .update(&id, |task| {
                task.status = Status::Review;
                task.current_run_mut().unwrap().outcome = Some(RunOutcome::Done);
                Ok(())
            })
            .unwrap();
        let gate = super::rebinding::receipt_result(
            &f,
            &id,
            GatePhase::Landing,
            "failed",
            None,
            GateResult::Fail,
        );
        let merge = if plan_target {
            RecoveryMerge::Rebase
        } else {
            RecoveryMerge::Ff
        };
        let input = RecoveryInput {
            action: RecoveryAction::AcceptCode { merge },
            exceptions: BTreeSet::from([RecoveryCheck::GateOutcome]),
            notes: String::new(),
        };
        let preview = recover_preview(&f.ctx, &id, &input).unwrap();
        assert!(
            preview.remaining_blockers.is_empty(),
            "{:?}",
            preview.remaining_blockers
        );
        let record = RecoveryRecord {
            version: RECOVERY_VERSION,
            id: format!("REC-{}", preview.digest),
            authorization: confirmation(&preview).authorization,
            preview,
            outcomes: vec![
                outcome(RecoveryState::Authorized, "owner intent", None),
                outcome(RecoveryState::Applying, "landing entered", None),
            ],
            links: vec![],
        };
        f.ctx.store.save_recovery_record(&id, &record).unwrap();
        let (task, run) = f.current(&id);
        let run = RunHead::of(&run).unwrap();
        let candidate = gate.identity.as_ref().unwrap();
        let main_before = corral_git::rev_sha(&f.repo, "main").unwrap();
        let mut target = crate::plan_target::validate_run(&f.ctx.store, &task, &run).unwrap();
        let op = if let Some(target) = target.as_mut() {
            target
                .prepare_landing(
                    &task,
                    &run,
                    &candidate.head,
                    "rebase",
                    None,
                    crate::plan_target::LandingOptions {
                        contract: &corral_core::TaskRef::contract_revision(&task).unwrap(),
                        remove_worktree: false,
                        keep_branch: true,
                        started: corral_core::clock::now(),
                    },
                )
                .unwrap();
            None
        } else {
            crate::hooks::prepare(
                &f.ctx,
                &task,
                &run,
                &f.repo,
                &candidate.integration_base,
                &candidate.head,
            )
            .unwrap()
        };
        super::super::bind_landing(&f.ctx, &record, op.as_ref().map(|op| op.key.as_str())).unwrap();
        let commands_before = landing_commands();
        let not_delivered = recover_reconcile(&f.ctx, &id, &record.id).unwrap();
        assert_eq!(state(&not_delivered).unwrap(), RecoveryState::Unknown);
        assert_eq!(
            corral_git::rev_sha(&f.repo, &run.base).unwrap(),
            candidate.integration_base
        );
        assert!(!super::super::code::code_committed(&f.ctx, &record).unwrap());
        assert_eq!(landing_commands(), commands_before);
        let checkout = target
            .as_ref()
            .map_or(f.repo.as_path(), |target| target.checkout.as_path());
        // Exactly one fixture-owned Git effect; omit the task/marker transaction deliberately.
        corral_git::land_checked(
            checkout,
            &run.worktree,
            &run.branch,
            &run.base,
            &candidate.head,
            &candidate.integration_base,
        )
        .unwrap();
        assert_eq!(landing_commands(), commands_before + 1);
        assert_eq!(f.current(&id).0.status, Status::Review);
        assert!(!super::super::code::code_committed(&f.ctx, &record).unwrap());
        let before = corral_git::rev_sha(&f.repo, &run.base).unwrap();
        if cancelled {
            let plan = plan.as_ref().unwrap();
            crate::plan_cancel(&f.ctx, plan).unwrap();
            // The durable inventory is terminal even if the card's status is later edited.
            f.ctx
                .store
                .update(plan, |task| {
                    task.status = Status::Ready;
                    Ok(())
                })
                .unwrap();
            let rejected = recover_reconcile(&f.ctx, &id, &record.id).unwrap();
            assert_eq!(
                state(&rejected).unwrap(),
                RecoveryState::Unknown,
                "{:?}",
                rejected.outcomes
            );
            let member = f.current(&id).0;
            assert_eq!(member.status, Status::Review);
            assert!(member.landed.is_none());
            assert!(!super::super::code::code_committed(&f.ctx, &record).unwrap());
            assert!(
                f.ctx
                    .store
                    .last_event_where(Some(&id), &["approved"], Some(&run.id), |_| true)
                    .unwrap()
                    .is_none()
            );
            if plan_target {
                let (target, _) = crate::plan_target::recovery_landing(
                    &f.ctx, &member, &run, candidate, "rebase",
                )
                .unwrap();
                assert!(target.landed(&id, &run.id).is_none());
            } else {
                assert!(!crate::post_land_operations(&f.ctx, None).unwrap()[0].landed);
            }
            assert_eq!(corral_git::rev_sha(&f.repo, &run.base).unwrap(), before);
            assert_eq!(landing_commands(), commands_before + 1);
            assert!(!hook_marker.exists());
            continue;
        }
        // A different reverse link cannot borrow this exact Git/journal effect.
        let area = corral_core::records::RecordArea::<(
            RecoveryReference,
            RecoveryEffect,
            Option<String>,
        )>::new("recovery-landings");
        let exact = area.load(&f.ctx.store, &record.id).unwrap().unwrap();
        let mut other = exact.clone();
        other.0.operation = "REC-other".into();
        area.save(&f.ctx.store, &record.id, &other).unwrap();
        assert_eq!(
            state(&recover_reconcile(&f.ctx, &id, &record.id).unwrap()).unwrap(),
            RecoveryState::Unknown
        );
        assert_eq!(f.current(&id).0.status, Status::Review);
        assert!(!super::super::code::code_committed(&f.ctx, &record).unwrap());
        area.save(&f.ctx.store, &record.id, &exact).unwrap();
        let old_policy = f.ctx.store.config.review.max_rounds;
        f.ctx.store.config.review.max_rounds = Some(9);
        f.ctx.store.save_config().unwrap();
        assert_eq!(
            state(&recover_reconcile(&f.ctx, &id, &record.id).unwrap()).unwrap(),
            RecoveryState::Unknown
        );
        assert_eq!(f.current(&id).0.status, Status::Review);
        assert!(!super::super::code::code_committed(&f.ctx, &record).unwrap());
        f.ctx.store.config.review.max_rounds = old_policy;
        f.ctx.store.save_config().unwrap();
        f.ctx
            .store
            .update(&id, |task| {
                task.current_run_mut()
                    .unwrap()
                    .gate
                    .as_mut()
                    .unwrap()
                    .log_sha256 = None;
                Ok(())
            })
            .unwrap();
        assert_eq!(
            state(&recover_reconcile(&f.ctx, &id, &record.id).unwrap()).unwrap(),
            RecoveryState::Unknown
        );
        assert_eq!(f.current(&id).0.status, Status::Review);
        assert!(!super::super::code::code_committed(&f.ctx, &record).unwrap());
        f.ctx
            .store
            .update(&id, |task| {
                task.current_run_mut().unwrap().gate = Some(gate.clone());
                Ok(())
            })
            .unwrap();
        let reconciled = recover_reconcile(&f.ctx, &id, &record.id).unwrap();
        assert_eq!(
            state(&reconciled).unwrap(),
            RecoveryState::Applied,
            "{:?}",
            reconciled.outcomes
        );
        assert_eq!(f.current(&id).0.status, Status::Done);
        assert_eq!(
            f.current(&id).0.landed.as_ref().unwrap().sha,
            candidate.head
        );
        assert!(super::super::code::code_committed(&f.ctx, &record).unwrap());
        assert_eq!(f.current(&id).1.gate.as_ref().unwrap(), &gate);
        assert!(
            reconciled
                .links
                .iter()
                .any(|link| matches!(link.receipt, RecoveryReceipt::LandingJournal { .. }))
        );
        assert!(
            reconciled
                .links
                .iter()
                .filter_map(|link| link.acceptance.as_ref())
                .all(|acceptance| !acceptance.policy_satisfied)
        );
        let again = recover_reconcile(&f.ctx, &id, &record.id).unwrap();
        assert_eq!(again.outcomes, reconciled.outcomes);
        assert_eq!(again.links, reconciled.links);
        assert_eq!(corral_git::rev_sha(&f.repo, &run.base).unwrap(), before);
        assert_eq!(
            landing_commands(),
            commands_before + 1,
            "reconciliation replayed a landing command"
        );
        assert!(
            !hook_marker.exists(),
            "reconciliation must not execute hooks"
        );
        if plan_target {
            assert_eq!(corral_git::rev_sha(&f.repo, "main").unwrap(), main_before);
            let target = crate::plan_target::validate_run(&f.ctx.store, &f.current(&id).0, &run)
                .unwrap()
                .unwrap();
            assert_eq!(target.landed(&id, &run.id).unwrap().0, candidate.head);
        } else {
            let operations = crate::post_land_operations(&f.ctx, None).unwrap();
            assert!(operations[0].landed);
            assert!(operations[0].hooks[0].attempts.is_empty());
        }
    }
}
