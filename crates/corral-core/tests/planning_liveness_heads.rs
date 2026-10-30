//! Planning liveness preserves full-facade refusals without reading settled history.
mod store_support;
use corral_core::agent::{self, AgentRole, AgentRunState, ContainerRef, RequestBinding};
use corral_core::work::{self, Counter, HotScope};
use corral_core::{PlanRunState, Status, Store, Task};
use serde_json::json;
use store_support::at;

#[derive(Clone, Copy, Debug)]
enum Case {
    Open,
    Dropped,
    WrongTask,
    OldContainer,
    MissingHost,
    WrongGeneration,
    MissingGeneration,
    Fenced,
    PlannerEnded,
    PlannerSubmitted,
    ChangedSpec,
    OldPlanRun,
    WrongReview,
    VerdictClosed,
    SettlesOpen,
    SettlesWrong,
    SettlesEnded,
    CallbackMismatch,
}

fn host(role: AgentRole) -> Box<AgentRunState> {
    let mut host = AgentRunState::new(
        serde_json::from_value(json!({
            "role":role, "store":"fixture", "project":"app", "generation":10000,
            "callback":"callback"
        }))
        .unwrap(),
    );
    host.generations.push(
        serde_json::from_value(json!({
            "n":10000,"bind_nonce_sha256":"nonce","pane":"pane","created":at(0)
        }))
        .unwrap(),
    );
    Box::new(host)
}

fn setup(store: &Store, scale: usize, role: AgentRole) -> (Task, RequestBinding) {
    // Both fixtures have a previous ended planner: grow settled history 1x/10x,
    // not the presence/absence of the head's last-ended summary.
    let mut task = store_support::rich(store, 1, 1, scale + 1, 1);
    task.comments.truncate(scale);
    task.artifacts.truncate(scale);
    task.approved_scope_history.truncate(scale);
    task.created = at(0);
    task.updated = at(0);
    task.status_since = Some(at(0));
    let revision = corral_core::plan_contract_revision(&task).unwrap();
    let run = task.plan_runs.last_mut().unwrap();
    run.ended = None;
    run.state = PlanRunState::Planning;
    run.agent_host = Some(host(AgentRole::Planner));
    let planner = run.id.clone();
    let plan = task.plans.last_mut().unwrap();
    plan.spec_revision = revision;
    plan.run = Some(planner.clone());
    let child = plan.reviews.last_mut().unwrap();
    child.verdict = None;
    child.agent_host = Some(host(AgentRole::PlanReviewer));
    let container = if role == AgentRole::Planner {
        ContainerRef::Planner {
            task: task.id.clone(),
            run: planner,
        }
    } else {
        ContainerRef::PlanReviewer {
            task: task.id.clone(),
            plan: plan.id.clone(),
            review: child.id.clone(),
        }
    };
    (
        task,
        RequestBinding {
            container,
            generation: 10000,
            callback: Some("callback".into()),
            settles_turn: None,
        },
    )
}

fn change(task: &mut Task, binding: &mut RequestBinding, case: Case) {
    match case {
        Case::Open => {}
        Case::Dropped => task.status = Status::Dropped,
        Case::WrongTask => match &mut binding.container {
            ContainerRef::Planner { task, .. } | ContainerRef::PlanReviewer { task, .. } => {
                *task = "T-999".into()
            }
            _ => unreachable!(),
        },
        Case::OldContainer => match &mut binding.container {
            ContainerRef::Planner { run, .. } => *run = "old".into(),
            ContainerRef::PlanReviewer { plan, .. } => *plan = "old".into(),
            _ => unreachable!(),
        },
        Case::MissingHost => match binding.container {
            ContainerRef::Planner { .. } => task.plan_runs.last_mut().unwrap().agent_host = None,
            ContainerRef::PlanReviewer { .. } => {
                task.plans
                    .last_mut()
                    .unwrap()
                    .reviews
                    .last_mut()
                    .unwrap()
                    .agent_host = None
            }
            _ => unreachable!(),
        },
        Case::WrongGeneration => binding.generation -= 1,
        Case::MissingGeneration => agent::container_state_mut(task, &binding.container)
            .unwrap()
            .generations
            .clear(),
        Case::Fenced => {
            agent::container_state_mut(task, &binding.container)
                .unwrap()
                .generations[0]
                .fenced =
                Some(serde_json::from_value(json!({"reason":"replaced","at":at(1)})).unwrap())
        }
        Case::PlannerEnded => task.plan_runs.last_mut().unwrap().ended = Some(at(1)),
        Case::PlannerSubmitted => {
            task.plan_runs.last_mut().unwrap().state = PlanRunState::Submitted
        }
        Case::ChangedSpec => task.body.push_str("changed"),
        Case::OldPlanRun => task.plans.last_mut().unwrap().run = Some("old".into()),
        Case::WrongReview => match &mut binding.container {
            ContainerRef::PlanReviewer { review, .. } => *review = "old".into(),
            _ => unreachable!(),
        },
        Case::VerdictClosed | Case::SettlesOpen | Case::SettlesWrong | Case::SettlesEnded => {
            task.plans
                .last_mut()
                .unwrap()
                .reviews
                .last_mut()
                .unwrap()
                .verdict = Some(corral_core::Verdict::Approve);
            if !matches!(case, Case::VerdictClosed) {
                binding.settles_turn = Some(if matches!(case, Case::SettlesWrong) {
                    2
                } else {
                    1
                });
                agent::container_state_mut(task, &binding.container)
                    .unwrap()
                    .turns
                    .push(
                        serde_json::from_value(json!({
                            "generation":10000,"seq":1,"origin":{"kind":"bridge"},"started":at(0),
                            "ended":if matches!(case, Case::SettlesEnded) {Some(at(1))} else {None}
                        }))
                        .unwrap(),
                    );
            }
        }
        Case::CallbackMismatch => binding.callback = Some("other".into()),
    }
}

#[test]
fn planning_liveness_matches_full_facades_at_one_and_tenfold_history() {
    for role in [AgentRole::Planner, AgentRole::PlanReviewer] {
        let mut cases = vec![
            Case::Open,
            Case::Dropped,
            Case::WrongTask,
            Case::OldContainer,
            Case::MissingHost,
            Case::WrongGeneration,
            Case::MissingGeneration,
            Case::Fenced,
        ];
        if role == AgentRole::Planner {
            cases.extend([Case::PlannerEnded, Case::PlannerSubmitted]);
        } else {
            cases.extend([
                Case::ChangedSpec,
                Case::OldPlanRun,
                Case::WrongReview,
                Case::VerdictClosed,
                Case::SettlesOpen,
                Case::SettlesWrong,
                Case::SettlesEnded,
                Case::CallbackMismatch,
            ]);
        }
        for case in cases {
            let mut measured = Vec::new();
            for scale in [1, 10] {
                let root = tempfile::tempdir().unwrap();
                let store = Store::init(root.path()).unwrap();
                let (mut task, mut binding) = setup(&store, scale, role);
                change(&mut task, &mut binding, case);
                let expected = agent::check_live(&task, &binding);
                if matches!(case, Case::Open | Case::SettlesOpen) {
                    assert!(expected.is_ok(), "{role:?} {case:?}: {expected:?}");
                } else {
                    assert!(expected.is_err(), "{role:?} {case:?}");
                }
                assert_eq!(
                    agent::check_live_ref(&task, &binding).unwrap(),
                    expected,
                    "full ref: {role:?} {case:?}"
                );
                let closure = agent::container_closure(&task, &binding.container);
                assert_eq!(
                    agent::container_closure_ref(&task, &binding.container).unwrap(),
                    closure
                );
                let recipient = match &binding.container {
                    ContainerRef::Planner { task, run } => corral_core::inbox::Recipient::PlanRun {
                        task: task.clone(),
                        plan_run: run.clone(),
                    },
                    ContainerRef::PlanReviewer { task, plan, review } => {
                        corral_core::inbox::Recipient::PlanReview {
                            task: task.clone(),
                            plan: plan.clone(),
                            review: review.clone(),
                        }
                    }
                    _ => unreachable!(),
                };
                let inbox = corral_core::inbox::Inbox::new(&store, recipient).unwrap();
                let ended = inbox.recipient_ended(&task);
                assert_eq!(inbox.recipient_ended_ref(&task).unwrap(), ended);
                store.fixture_put_task(&task).unwrap();
                let _hot = HotScope::enter();
                let before = work::snapshot();
                let head = store.head(&task.id).unwrap();
                assert_eq!(
                    agent::check_live_ref(&head, &binding).unwrap(),
                    expected,
                    "head ref: {role:?} {case:?}"
                );
                assert_eq!(
                    agent::container_closure_ref(&head, &binding.container).unwrap(),
                    closure,
                    "closure: {role:?} {case:?}"
                );
                assert_eq!(
                    inbox.recipient_ended_ref(&head).unwrap(),
                    ended,
                    "recipient: {role:?} {case:?}"
                );
                let after = work::snapshot();
                measured.push(
                    [
                        Counter::TaskHeadDecodes,
                        Counter::NestedRowDecodes,
                        Counter::HistoryRowDecodes,
                        Counter::FullTaskHydrations,
                        Counter::DecodedDocBytes,
                        Counter::ViewsOpened,
                    ]
                    .map(|counter| after.total(counter) - before.total(counter)),
                );
            }
            assert_eq!(measured[0], measured[1], "growth: {role:?} {case:?}");
            assert_eq!(measured[0][3], 0, "hydrations: {role:?} {case:?}");
        }
    }
}
