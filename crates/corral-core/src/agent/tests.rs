use jiff::Timestamp;
use serde_json::json;

use super::*;
use crate::{RunOutcome, Status, Task, Verdict};

fn run(id: &str) -> crate::Run {
    serde_json::from_value(json!({
        "id": id, "agent": "pi", "worktree": "/tmp/w", "branch": "corral/T-001",
        "base": "main", "base_sha": "abc", "started": "2026-10-23T00:00:00Z",
        "herdr": {"workspace_id": "w1", "pane_id": "p1"}
    }))
    .unwrap()
}

fn state(role: AgentRole, callback: Option<&str>) -> Box<AgentRunState> {
    let mut state = AgentRunState::new(AgentBinding {
        role,
        store: "st-0".into(),
        project: "corral".into(),
        generation: 1,
        health: HostHealth::Healthy,
        callback: callback.map(Into::into),
        closing: false,
    });
    state.generations.push(AgentGeneration {
        n: 1,
        bind_nonce_sha256: "00".into(),
        pane: "p1".into(),
        created: Timestamp::UNIX_EPOCH,
        bound: None,
        fenced: None,
        nudged_at: None,
        event_seq: 0,
        todo: false,
        capabilities: None,
        capability_transition: None,
    });
    state.turns.push(AgentTurn {
        generation: 1,
        seq: 1,
        origin: TurnOrigin {
            kind: TurnOriginKind::Bridge,
            idempotency_key: None,
        },
        started: Some(Timestamp::UNIX_EPOCH),
        ended: None,
        class: None,
        provider_error: None,
    });
    Box::new(state)
}

fn task(status: Status) -> Task {
    let mut task: Task = serde_json::from_value(json!({
        "id": "T-001", "title": "t", "status": status.as_str(), "project": "corral",
        "created": "2026-10-23T00:00:00Z", "updated": "2026-10-23T00:00:00Z"
    }))
    .unwrap();
    let mut worker = run("R1");
    worker.agent_host = Some(state(AgentRole::Worker, None));
    task.runs.push(worker);
    task
}

fn worker(settles_turn: Option<u64>) -> RequestBinding {
    RequestBinding {
        container: ContainerRef::Worker {
            task: "T-001".into(),
            run: "R1".into(),
        },
        generation: 1,
        callback: None,
        settles_turn,
    }
}

fn with_review(mut task: Task) -> (Task, RequestBinding) {
    let mut child = run("R1-review1");
    child.agent_host = Some(state(AgentRole::Reviewer, Some("g1:c1")));
    task.runs[0].outcome = Some(RunOutcome::Done);
    task.runs[0].review_runs.push(child);
    task.status = Status::Review;
    let binding = RequestBinding {
        container: ContainerRef::Reviewer {
            task: "T-001".into(),
            run: "R1".into(),
            review: "R1-review1".into(),
        },
        generation: 1,
        callback: Some("g1:c1".into()),
        settles_turn: None,
    };
    (task, binding)
}

fn verdict(task: &mut Task) {
    let record: crate::ReviewRecord = serde_json::from_value(json!({
        "verdict": "approve", "reviewer": "pi", "sha": "abc", "at": "2026-10-23T00:00:00Z"
    }))
    .unwrap();
    assert_eq!(record.verdict, Some(Verdict::Approve));
    task.runs[0].review_runs[0].review = Some(record);
}

fn code(result: Result<(), AgentRefusal>) -> Option<RefusalCode> {
    result.err().map(|refusal| refusal.code)
}

#[test]
fn check_live_accepts_a_current_worker_and_refuses_each_ended_or_fenced_state() {
    let live = task(Status::Running);
    assert_eq!(code(check_live(&live, &worker(None))), None);

    let mut done = task(Status::Review);
    done.runs[0].outcome = Some(RunOutcome::Done);
    assert_eq!(
        code(check_live(&done, &worker(None))),
        Some(RefusalCode::RunEnded)
    );
    assert_eq!(
        code(check_live(&done, &worker(Some(1)))),
        None,
        "the open done turn settles"
    );
    assert_eq!(
        code(check_live(&done, &worker(Some(999)))),
        Some(RefusalCode::RunEnded),
        "an unopened turn never settles"
    );
    let mut settled = done.clone();
    settled.runs[0].agent_host.as_mut().unwrap().turns[0].ended = Some(Timestamp::UNIX_EPOCH);
    assert_eq!(
        code(check_live(&settled, &worker(Some(1)))),
        Some(RefusalCode::RunEnded),
        "an ended turn settles only once"
    );

    let mut retried = task(Status::Running);
    retried.runs[0].outcome = Some(RunOutcome::Superseded);
    retried.runs.push(run("R2"));
    assert_eq!(
        code(check_live(&retried, &worker(Some(1)))),
        Some(RefusalCode::RunEnded)
    );

    let mut fenced = task(Status::Running);
    let host = fenced.runs[0].agent_host.as_mut().unwrap();
    host.generations[0].fenced = Some(GenerationFence {
        reason: "resume".into(),
        at: Timestamp::UNIX_EPOCH,
    });
    assert_eq!(
        code(check_live(&fenced, &worker(None))),
        Some(RefusalCode::AgentFenced)
    );

    let mut newer = task(Status::NeedsMe);
    newer.runs[0]
        .agent_host
        .as_mut()
        .unwrap()
        .binding
        .generation = 2;
    assert_eq!(
        code(check_live(&newer, &worker(None))),
        Some(RefusalCode::AgentFenced)
    );
}

#[test]
fn check_live_binds_a_reviewer_to_its_open_last_child_and_callback() {
    let (live, binding) = with_review(task(Status::Running));
    assert_eq!(code(check_live(&live, &binding)), None);

    let mut wrong = binding.clone();
    wrong.callback = Some("g1:other".into());
    assert_eq!(
        code(check_live(&live, &wrong)),
        Some(RefusalCode::ReviewClosed)
    );

    let (mut judged, binding) = with_review(task(Status::Running));
    verdict(&mut judged);
    assert_eq!(
        code(check_live(&judged, &binding)),
        Some(RefusalCode::ReviewClosed)
    );
    let settle = RequestBinding {
        settles_turn: Some(1),
        ..binding.clone()
    };
    assert_eq!(
        code(check_live(&judged, &settle)),
        None,
        "the open verdict turn settles"
    );
    let invented = RequestBinding {
        settles_turn: Some(999),
        ..binding.clone()
    };
    assert_eq!(
        code(check_live(&judged, &invented)),
        Some(RefusalCode::ReviewClosed),
        "an unopened turn never settles"
    );
    let settle_after = |change: fn(&mut Task)| {
        let (mut judged, binding) = with_review(task(Status::Running));
        verdict(&mut judged);
        change(&mut judged);
        let settle = RequestBinding {
            settles_turn: Some(1),
            ..binding
        };
        code(check_live(&judged, &settle))
    };
    let newer_child: fn(&mut Task) = |t| t.runs[0].review_runs.push(run("R1-review2"));
    let reopened: fn(&mut Task) = |t| {
        t.status = Status::Running;
        t.runs[0].outcome = None;
    };
    let superseded: fn(&mut Task) =
        |t| t.runs[0].review_runs[0].outcome = Some(RunOutcome::Superseded);
    for change in [newer_child, reopened, superseded] {
        assert_eq!(
            settle_after(change),
            Some(RefusalCode::ReviewClosed),
            "the open turn waives only the verdict"
        );
    }

    let (mut newer, binding) = with_review(task(Status::Running));
    newer.runs[0].review_runs.push(run("R1-review2"));
    assert_eq!(
        code(check_live(&newer, &binding)),
        Some(RefusalCode::ReviewClosed)
    );
    assert_eq!(
        container_closure(&newer, &binding.container),
        Some(ClosureReason::ReviewSuperseded)
    );
}

#[test]
fn closure_predicate_covers_every_worker_and_review_child_path() {
    let container = worker(None).container;
    let open = task(Status::Running);
    assert_eq!(container_closure(&open, &container), None);
    let mut review = task(Status::Review);
    review.runs[0].outcome = Some(RunOutcome::Done);
    assert_eq!(
        container_closure(&review, &container),
        None,
        "done in Review"
    );
    let mut landed = review.clone();
    landed.status = Status::Done;
    assert_eq!(
        container_closure(&landed, &container),
        Some(ClosureReason::RunEnded)
    );
    for outcome in [
        RunOutcome::Discarded,
        RunOutcome::Superseded,
        RunOutcome::Abandoned,
    ] {
        let mut ended = task(Status::NeedsMe);
        ended.runs[0].outcome = Some(outcome);
        assert_eq!(
            container_closure(&ended, &container),
            Some(ClosureReason::RunEnded)
        );
    }
    let mut failed = task(Status::NeedsMe);
    failed.runs[0].outcome = Some(RunOutcome::Failed);
    assert_eq!(container_closure(&failed, &container), None, "host loss");

    let (mut superseded, binding) = with_review(task(Status::Running));
    superseded.runs[0].review_runs[0].outcome = Some(RunOutcome::Superseded);
    assert_eq!(
        container_closure(&superseded, &binding.container),
        Some(ClosureReason::ReviewSuperseded)
    );
    let (mut interrupted, binding) = with_review(task(Status::Running));
    interrupted.runs[0].review_runs[0].outcome = Some(RunOutcome::Interrupted);
    assert_eq!(
        container_closure(&interrupted, &binding.container),
        Some(ClosureReason::ReviewInterrupted)
    );
    let (mut discarded, binding) = with_review(task(Status::Running));
    discarded.runs[0].outcome = Some(RunOutcome::Discarded);
    assert_eq!(
        container_closure(&discarded, &binding.container),
        Some(ClosureReason::RunEnded)
    );
}

#[test]
fn records_round_trip_through_the_task_file_and_default_to_absent() {
    let mut task = task(Status::Running);
    let host = task.runs[0].agent_host.as_mut().unwrap();
    host.receipts.push(RequestReceipt {
        version: AGENT_RECORD_VERSION,
        request_id: "ev-1-1".into(),
        op: "event".into(),
        payload_sha256: "0".repeat(64),
        identity: RequestIdentity {
            store: "st-0".into(),
            project: "corral".into(),
            role: AgentRole::Worker,
            task: Some("T-001".into()),
            run: Some("R1".into()),
            review: None,
            callback: None,
            session_id: "s".into(),
            generation: 1,
            turn: Some(1),
        },
        state: ReceiptState::Committed,
        result: json!({"kind": "turn_started"}),
        executor: None,
        effect: Some(ReceiptEffect {
            kind: "turn".into(),
            reference: "1/1".into(),
        }),
        evidence: Vec::new(),
        post_pending: Vec::new(),
        at: Timestamp::UNIX_EPOCH,
        committed_at: Some(Timestamp::UNIX_EPOCH),
    });
    task.path = std::path::PathBuf::from("/tmp/T-001-t.md");
    let text = crate::render_task(&task);
    let parsed = crate::parse_task(&text, &task.path).unwrap();
    assert_eq!(parsed.runs[0].agent_host, task.runs[0].agent_host);
    assert!(text.contains("\"ref\"") || text.contains("ref: 1/1"));
    let legacy = run("R9");
    assert!(legacy.agent_host.is_none());
    assert!(
        !serde_json::to_string(&legacy)
            .unwrap()
            .contains("agent_host")
    );
}

fn planning_task() -> (Task, RequestBinding, RequestBinding) {
    let mut task = task(Status::Ready);
    let revision = crate::plan_contract_revision(&task).unwrap();
    let mut planner: crate::PlanRun = serde_json::from_value(json!({
        "id": "D1", "profile": "pi", "harness": "pi-native",
        "spec_revision": revision, "started": "2026-10-23T00:00:00Z",
        "output_dir": "/tmp/plan", "rounds": 0, "state": "planning",
        "herdr": {"workspace_id": "w1", "pane_id": "p1"}
    }))
    .unwrap();
    planner.agent_host = Some(state(AgentRole::Planner, None));
    task.plan_runs.push(planner);
    let mut plan: crate::PlanRecord = serde_json::from_value(json!({
        "id": "P1", "run": "D1", "spec_revision": revision,
        "artifact": {
            "run": "D1", "kind": "plan", "title": "Plan",
            "path": "plans/P1.md", "source": "/tmp/plan.md",
            "sha256": "abc", "bytes": 1, "added": "2026-10-23T00:00:00Z"
        },
        "document": {
            "goal": "Plan", "non_goals": [], "decisions": [],
            "affected_paths": ["core"], "contracts": [], "dependencies": [],
            "acceptance": [], "verification": ["test"], "owner_decisions": []
        },
        "reviews": [{
            "id": "PR1", "reviewer": "pi", "policy_digest": "policy",
            "at": "2026-10-23T00:00:00Z",
            "herdr": {"workspace_id": "w1", "pane_id": "p2"}
        }]
    }))
    .unwrap();
    plan.reviews[0].agent_host = Some(state(AgentRole::PlanReviewer, None));
    task.plans.push(plan);
    let planner = RequestBinding {
        container: ContainerRef::Planner {
            task: task.id.clone(),
            run: "D1".into(),
        },
        generation: 1,
        callback: None,
        settles_turn: None,
    };
    let reviewer = RequestBinding {
        container: ContainerRef::PlanReviewer {
            task: task.id.clone(),
            plan: "P1".into(),
            review: "PR1".into(),
        },
        ..planner.clone()
    };
    (task, planner, reviewer)
}

#[test]
fn planning_bindings_round_trip_and_resolve_without_implementation_runs() {
    let (mut task, planner, reviewer) = planning_task();
    task.runs.clear();
    for binding in [&planner, &reviewer] {
        assert!(container_run(&task, &binding.container).is_none());
        assert!(container_run_mut(&mut task, &binding.container).is_none());
        assert!(container_state(&task, &binding.container).is_some());
        assert!(container_pane(&task, &binding.container).is_some());
        assert_eq!(code(check_live(&task, binding)), None);
        let serialized = serde_json::to_string(&binding.container).unwrap();
        assert_eq!(
            serde_json::from_str::<ContainerRef>(&serialized).unwrap(),
            binding.container
        );
    }
    let text = crate::render_task(&task);
    let restored = crate::parse_task(&text, std::path::Path::new("/tmp/T-001-t.md")).unwrap();
    assert_eq!(restored.plan_runs, task.plan_runs);
    assert_eq!(restored.plans, task.plans);
    assert_eq!(
        task.plan_runs[0].agent_host.as_ref().unwrap().version,
        AGENT_RECORD_VERSION
    );
    assert_eq!(AgentRole::Planner.as_str(), "planner");
    assert_eq!(AgentRole::PlanReviewer.as_str(), "plan_reviewer");
    *container_host_mut(&mut task, &planner.container).unwrap() = None;
    assert_eq!(
        code(check_live(&task, &planner)),
        Some(RefusalCode::AgentFenced)
    );
    *container_host_mut(&mut task, &planner.container).unwrap() =
        Some(state(AgentRole::Planner, None));
    let wrong = ContainerRef::Planner {
        task: "T-002".into(),
        run: "D1".into(),
    };
    assert!(container_state(&task, &wrong).is_none());
    assert!(container_host_mut(&mut task, &wrong).is_none());
    assert!(container_pane(&task, &wrong).is_none());
}

#[test]
fn planning_liveness_requires_current_active_unfenced_containers() {
    let (task, planner, reviewer) = planning_task();
    for binding in [&planner, &reviewer] {
        let mut fenced = task.clone();
        container_state_mut(&mut fenced, &binding.container)
            .unwrap()
            .generations[0]
            .fenced = Some(GenerationFence {
            reason: "lost".into(),
            at: Timestamp::UNIX_EPOCH,
        });
        assert_eq!(
            code(check_live(&fenced, binding)),
            Some(RefusalCode::AgentFenced)
        );
        let mut stale = binding.clone();
        stale.generation = 2;
        assert_eq!(
            code(check_live(&task, &stale)),
            Some(RefusalCode::AgentFenced)
        );
        let mut dropped = task.clone();
        dropped.status = Status::Dropped;
        assert_eq!(
            container_closure(&dropped, &binding.container),
            Some(ClosureReason::RunEnded)
        );
        assert_eq!(
            code(check_live(&dropped, binding)),
            Some(RefusalCode::RunEnded)
        );
    }
    for state in [
        crate::PlanRunState::Submitted,
        crate::PlanRunState::Cancelled,
    ] {
        let mut ended = task.clone();
        ended.plan_runs[0].state = state;
        assert_eq!(
            code(check_live(&ended, &planner)),
            Some(RefusalCode::RunEnded)
        );
    }
    let mut changes = task.clone();
    changes.plan_runs[0].state = crate::PlanRunState::Changes;
    assert!(check_live(&changes, &planner).is_ok());
    changes.plan_runs[0].ended = Some(Timestamp::UNIX_EPOCH);
    assert_eq!(
        code(check_live(&changes, &planner)),
        Some(RefusalCode::RunEnded)
    );
    let mut newer = task.clone();
    let mut run = newer.plan_runs[0].clone();
    run.id = "D2".into();
    newer.plan_runs.push(run);
    assert_eq!(
        code(check_live(&newer, &planner)),
        Some(RefusalCode::RunEnded)
    );
    assert_eq!(
        container_closure(&newer, &reviewer.container),
        Some(ClosureReason::ReviewSuperseded)
    );
    // Historical bindings remain addressable for receipts even though mutations are closed.
    assert!(container_state(&newer, &planner.container).is_some());
}

#[test]
fn planning_review_closes_on_verdict_supersession_and_spec_change() {
    let (task, _, reviewer) = planning_task();
    let mut closed = task.clone();
    closed.plans[0].reviews[0].verdict = Some(Verdict::Approve);
    assert_eq!(
        container_closure(&closed, &reviewer.container),
        Some(ClosureReason::ReviewClosed)
    );
    assert_eq!(
        code(check_live(&closed, &reviewer)),
        Some(RefusalCode::ReviewClosed)
    );
    let mut settling = reviewer.clone();
    settling.settles_turn = Some(1);
    assert!(check_live(&closed, &settling).is_ok());
    settling.settles_turn = Some(2);
    assert_eq!(
        code(check_live(&closed, &settling)),
        Some(RefusalCode::ReviewClosed)
    );
    let mut changed = task.clone();
    changed.body = "Changed task specification".into();
    assert_eq!(
        container_closure(&changed, &reviewer.container),
        Some(ClosureReason::ReviewSuperseded)
    );
    let mut newer_review = task.clone();
    let mut review = newer_review.plans[0].reviews[0].clone();
    review.id = "PR2".into();
    newer_review.plans[0].reviews.push(review);
    assert_eq!(
        container_closure(&newer_review, &reviewer.container),
        Some(ClosureReason::ReviewSuperseded)
    );
    let mut newer_plan = task.clone();
    let mut plan = newer_plan.plans[0].clone();
    plan.id = "P2".into();
    newer_plan.plans.push(plan);
    assert_eq!(
        container_closure(&newer_plan, &reviewer.container),
        Some(ClosureReason::ReviewSuperseded)
    );
    let mut callback = reviewer.clone();
    callback.callback = Some("unexpected".into());
    assert_eq!(
        code(check_live(&task, &callback)),
        Some(RefusalCode::ReviewClosed)
    );
}

#[test]
fn planning_sessions_have_distinct_namespaces_and_inbox_closure() {
    let (mut task, planner, reviewer) = planning_task();
    let tmp = tempfile::tempdir().unwrap();
    let store = crate::Store::init(tmp.path()).unwrap();
    for (binding, suffix) in [
        (&planner, "T-001/plan-D1"),
        (&reviewer, "T-001/plan-P1/PR1"),
    ] {
        assert_eq!(binding.container.key(), suffix);
        assert!(paths::container_sessions_dir(&store, &binding.container).ends_with(suffix));
    }
    let planner_inbox = crate::inbox::Inbox::new(
        &store,
        crate::inbox::Recipient::PlanRun {
            task: task.id.clone(),
            plan_run: "D1".into(),
        },
    )
    .unwrap();
    let review_inbox = crate::inbox::Inbox::new(
        &store,
        crate::inbox::Recipient::PlanReview {
            task: task.id.clone(),
            plan: "P1".into(),
            review: "PR1".into(),
        },
    )
    .unwrap();
    assert!(!planner_inbox.recipient_ended(&task));
    assert!(!review_inbox.recipient_ended(&task));
    task.status = Status::Dropped;
    assert!(planner_inbox.recipient_ended(&task));
    assert!(review_inbox.recipient_ended(&task));
}
