//! Change policy, identity, or a real prepare invariant after authorization but before locks.
use super::*;

#[derive(Clone, Copy, Debug)]
enum Fault {
    Policy,
    Identity,
    Callback,
}

#[derive(Clone, Debug)]
enum Action {
    Entry(Path),
    Aggregate(String),
}

impl Action {
    fn kind(&self) -> OperatingActionKind {
        match self {
            Self::Entry(path) => path.kind(),
            Self::Aggregate(_) => OperatingActionKind::AcceptAggregate,
        }
    }

    fn enter(&self, ctx: &crate::Ctx, id: &str, automatic: bool) -> Result<()> {
        match self {
            Self::Entry(path) => path.enter(ctx, id, automatic),
            Self::Aggregate(attempt) => crate::plan_accept(ctx, id, attempt, automatic).map(|_| ()),
        }
    }

    fn invalidate_prepare(&self, task: &mut corral_core::Task) {
        match self {
            Self::Entry(Path::Worker) => {
                task.launch.as_mut().unwrap().stage = corral_core::LaunchStage::Worktree
            }
            Self::Entry(Path::Planner) => task.plan_runs.last_mut().unwrap().rounds += 1,
            Self::Entry(Path::RepairLegacy | Path::RepairInbox) => {
                task.current_run_mut().unwrap().outcome = None;
            }
            Self::Entry(Path::PlanReviewPane | Path::PlanReviewPm) => {
                task.plans
                    .last_mut()
                    .unwrap()
                    .reviews
                    .last_mut()
                    .unwrap()
                    .pending_reply = Some("changed after authorization".into());
            }
            Self::Entry(Path::AcceptPlan) => {
                task.plans
                    .last_mut()
                    .unwrap()
                    .reviews
                    .last_mut()
                    .unwrap()
                    .verdict = Some(corral_core::Verdict::Changes);
            }
            Self::Entry(Path::ReviewerPane | Path::ReviewerPm) => {
                task.current_run_mut().unwrap().review.as_mut().unwrap().sha =
                    "changed after authorization".into();
            }
            Self::Entry(Path::Gate) => {
                task.current_run_mut().unwrap().gate.as_mut().unwrap().sha =
                    "changed after authorization".into();
            }
            Self::Aggregate(_) => {
                task.aggregate_attempts.last_mut().unwrap().identity.tree =
                    "changed after authorization".into();
            }
        }
    }
}

#[derive(Clone, Debug)]
struct Case {
    action: Action,
    automatic: bool,
    fault: Fault,
}

#[derive(Default)]
struct Seen {
    entered: usize,
    refused: usize,
    snapshot: Option<corral_core::Task>,
}

fn verify(mut fx: Fx, id: String, case: Case) {
    let before = counts(&fx);
    let store = fx.ctx.store.clone();
    let bin = fx.bin.clone();
    let task_id = id.clone();
    let at_boundary = case.clone();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let observed = seen.clone();
    fx.ctx.store = fx.ctx.store.with_submission_barrier(Arc::new(move |point| {
        if point.starts_with("lock:") {
            return;
        }
        match point {
            "submission_entered" => {
                let mut seen = observed.lock().unwrap();
                seen.entered += 1;
                assert_eq!(seen.entered, 1);
                let contract =
                    corral_core::operating_contract_revision(&store.get(&task_id).unwrap())
                        .unwrap();
                match at_boundary.fault {
                    Fault::Policy => {
                        store.pause_operating_policy(true).unwrap();
                    }
                    Fault::Identity => {
                        store
                            .update(&task_id, |task| {
                                task.body
                                    .push_str("\nChanged contract after authorization.");
                                Ok(())
                            })
                            .unwrap();
                    }
                    Fault::Callback => {
                        store
                            .update(&task_id, |task| {
                                at_boundary.action.invalidate_prepare(task);
                                Ok(())
                            })
                            .unwrap();
                        // This case must reach the real prepare check, not fail identity
                        // validation or automatic policy resolution instead.
                        assert_eq!(
                            corral_core::operating_contract_revision(&store.get(&task_id).unwrap())
                                .unwrap(),
                            contract
                        );
                        if at_boundary.automatic {
                            assert!(
                                store
                                    .effective_operating_policy()
                                    .unwrap()
                                    .allows(&task_id, at_boundary.action.kind()),
                                "{at_boundary:?}: mutation accidentally revoked policy"
                            );
                        }
                    }
                }
                seen.snapshot = Some(store.get(&task_id).unwrap());
            }
            "refused_committed" => {
                let mut seen = observed.lock().unwrap();
                seen.refused += 1;
                assert_eq!(seen.refused, 1);
                assert_eq!(
                    store.get(&task_id).unwrap(),
                    *seen.snapshot.as_ref().unwrap(),
                    "{at_boundary:?}: refusal changed a task row"
                );
                let ctx = crate::Ctx::from_store(store.clone());
                assert!(
                    authorizations(&ctx, &task_id, at_boundary.action.kind())
                        .iter()
                        .any(|record| matches!(
                            record.disposition,
                            SubmissionDisposition::AuthorizationRefused { .. }
                        ))
                );
                let effects = (
                    std::fs::read_to_string(bin.join("submissions.jsonl"))
                        .unwrap_or_default()
                        .lines()
                        .count(),
                    std::fs::read_to_string(bin.join("deliveries.jsonl"))
                        .unwrap_or_default()
                        .lines()
                        .count(),
                    store
                        .events()
                        .unwrap()
                        .iter()
                        .filter(|event| event.kind == "message_posted")
                        .count(),
                );
                assert_eq!(effects, before, "{at_boundary:?}: refusal sent an effect");
            }
            other => panic!("{at_boundary:?}: refused submission reached {other}"),
        }
    }));
    assert!(
        case.action.enter(&fx.ctx, &id, case.automatic).is_err(),
        "{case:?}"
    );
    let seen = seen.lock().unwrap();
    assert_eq!((seen.entered, seen.refused), (1, 1), "{case:?}");
    assert_eq!(counts(&fx), before, "{case:?}: executor sent after refusal");
}

#[test]
fn submission_paths_late_policy_identity_and_callback_refusals_change_no_task_or_effect() {
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
                continue;
            }
            for fault in [Fault::Policy, Fault::Identity, Fault::Callback] {
                if !automatic && matches!(fault, Fault::Policy) {
                    continue;
                }
                let (mut fx, id) = path.setup();
                if automatic {
                    if matches!(path, Path::PlanReviewPm) {
                        fx.ctx.store.config.review.reviewer = Some("pm".into());
                        fx.ctx.store.save_config().unwrap();
                    }
                    fx.grant(&[&id]);
                }
                verify(
                    fx,
                    id,
                    Case {
                        action: Action::Entry(path),
                        automatic,
                        fault,
                    },
                );
            }
        }
    }
    for code in [false, true] {
        for automatic in [false, true] {
            for fault in [Fault::Policy, Fault::Identity, Fault::Callback] {
                if !automatic && matches!(fault, Fault::Policy) {
                    continue;
                }
                let (fx, id, attempt, _) = aggregate::scenario(code, automatic);
                verify(
                    fx,
                    id,
                    Case {
                        action: Action::Aggregate(attempt),
                        automatic,
                        fault,
                    },
                );
            }
        }
    }
}

#[test]
fn submission_paths_recovered_late_refusals_preserve_exact_attempt_without_effect() {
    for path in [Path::Gate, Path::ReviewerPane, Path::ReviewerPm] {
        for automatic in [false, true] {
            for fault in [Fault::Policy, Fault::Identity, Fault::Callback] {
                if !automatic && matches!(fault, Fault::Policy) {
                    continue;
                }
                let (fx, id) = recovered_setup(path, automatic);
                verify(
                    fx,
                    id,
                    Case {
                        action: Action::Entry(path),
                        automatic,
                        fault,
                    },
                );
            }
        }
    }
}
