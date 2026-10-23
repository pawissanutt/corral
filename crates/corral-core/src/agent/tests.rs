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
