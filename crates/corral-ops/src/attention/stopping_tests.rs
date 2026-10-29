use super::*;
use corral_core::agent::{AgentBinding, AgentRole, AgentRunState, HostHealth};
use corral_core::{EffectiveTaskPolicy, GateResult, OperatingMode, RunOutcome};
use serde_json::json;

fn at() -> Timestamp {
    "2026-10-29T08:00:00Z".parse().unwrap()
}

fn worker() -> Run {
    serde_json::from_value(json!({
        "id":"R1", "agent":"fixture", "worktree":"/fixture", "branch":"fixture",
        "base":"main", "base_sha":"abc", "started":"2026-10-29T07:00:00Z",
        "agent_state":"working"
    }))
    .unwrap()
}

fn task(status: Status) -> Task {
    let mut task = corral_core::parse_task(
        "---\nid: T-550\ntitle: Fixture\nstatus: running\ncreated: 2026-10-29T07:00:00Z\nupdated: 2026-10-29T07:00:00Z\n---\n",
        std::path::Path::new("T-550.md"),
    )
    .unwrap();
    task.status = status;
    task.status_since = Some("2026-10-29T07:00:00Z".parse().unwrap());
    task.runs.push(worker());
    task
}

fn reviewer(health: HostHealth, blocking: Option<bool>) -> Task {
    let mut task = task(Status::Review);
    let parent = &mut task.runs[0];
    parent.outcome = Some(RunOutcome::Done);
    parent.ended = Some(parent.started);
    parent.review = Some(
        serde_json::from_value(json!({
            "reviewer":"fixture","sha":"abc","at":parent.started,"pane":"reviewer"
        }))
        .unwrap(),
    );
    let mut child = worker();
    child.id = "rv1".into();
    let mut host = AgentRunState::new(AgentBinding {
        role: AgentRole::Reviewer,
        store: "fixture".into(),
        project: "fixture".into(),
        generation: 1,
        health,
        callback: None,
        closing: false,
    });
    if let Some(blocking) = blocking {
        host.questions.push(
            serde_json::from_value(json!({
                "version":1,"key":"T-550/R1/rv1/Q1","revision":1,"kind":"clarification",
                "blocking":blocking,"text":"Which constraint?","state":"open",
                "asked":{"request_id":"ask","payload_sha256":"sha","session_id":"session",
                    "generation":1,"at":"2026-10-29T07:30:00Z"},
                "wake":{"route":"none","key":"wake","state":"pending"}
            }))
            .unwrap(),
        );
    }
    child.agent_host = Some(Box::new(host));
    parent.review_runs.push(child);
    task
}

fn assessment(task: &Task, configure: impl FnOnce(&mut Neighborhood<'_>)) -> Assessment {
    let before = task.clone();
    let config = Config::default_for(std::path::Path::new("/fixture"));
    let policy = EffectivePolicy {
        mode: OperatingMode::Autopilot,
        paused: false,
        revision: "fixture".into(),
        authorization: Some("grant".into()),
        reasons: vec![],
        tasks: BTreeMap::from([(
            task.id.clone(),
            EffectiveTaskPolicy {
                review: config.task_review_policy(task),
                acceptance_source: corral_core::PolicySource::Task,
                plan_acceptance_source: corral_core::PolicySource::Task,
                refusals: BTreeMap::new(),
            },
        )]),
    };
    let mut n = Neighborhood::new(std::slice::from_ref(task), &config, &policy);
    configure(&mut n);
    let counts = corral_core::work::Work::ALL.map(corral_core::work::count);
    let result = assess(task, &n, at());
    assert_eq!(*task, before);
    for (index, work) in corral_core::work::Work::ALL.iter().enumerate() {
        assert_eq!(
            corral_core::work::count(*work),
            counts[index] + u64::from(*work == corral_core::work::Work::AttentionVisit)
        );
    }
    result
}

#[test]
fn active_reviewer_question_stops_only_when_blocking() {
    for blocking in [false, true] {
        // Given a completed worker and its active reviewer question.
        let task = reviewer(HostHealth::Healthy, Some(blocking));
        // When the frozen projection assesses the child.
        let a = assessment(&task, |_| {});
        let stopped = corral_core::attention::blocked(&task, &a);
        // Then an advisory question keeps progress, but a blocking one needs PM action.
        assert_eq!(stopped.is_some(), blocking, "{a:#?}");
        if let Some(condition) = stopped {
            assert!(a.progress.is_none());
            assert_eq!(condition.kind, "question_open");
            assert_eq!(condition.responsible, Responsible::Pm);
            assert_eq!(condition.unblocker(), "PM");
            assert_eq!(
                condition.since,
                Some("2026-10-29T07:30:00Z".parse().unwrap())
            );
        } else {
            assert_eq!(a.progress.unwrap().executor, "reviewer");
        }
    }
}

#[test]
fn lost_and_fenced_reviewer_hosts_stop_the_completed_parents_task() {
    for health in [HostHealth::Lost, HostHealth::Fenced] {
        // Given recorded child-host loss, not a new pane probe.
        let task = reviewer(health, None);
        let a = assessment(&task, |_| {});
        // Then the child owns recovery even though the parent worker already ended.
        let condition = corral_core::attention::blocked(&task, &a).unwrap();
        assert!(a.progress.is_none());
        assert_eq!(condition.kind, "agent_lost");
        assert_eq!(condition.responsible, Responsible::Pm);
        assert!(condition.resume.iter().any(|check| {
            check.kind == "run_recovered" && check.target.as_deref() == Some("rv1")
        }));
    }
}

#[test]
fn retained_reviewer_loss_requires_the_exact_pane_and_start_identity() {
    let mut task = reviewer(HostHealth::Healthy, None);
    let child = &mut task.runs[0].review_runs[0];
    child.herdr = Some(
        serde_json::from_value(json!({
            "pane_id":"reviewer","workspace_id":"fixture","socket":"/fixture/herdr.sock"
        }))
        .unwrap(),
    );
    let exact = format!("reviewer@{}", child.started);
    let since: Timestamp = "2026-10-29T07:40:00Z".parse().unwrap();
    for (identity, lost) in [
        ("other@2026-10-29T07:00:00Z".into(), false),
        ("reviewer@2026-10-29T06:00:00Z".into(), false),
        (exact, true),
    ] {
        let a = assessment(&task, |n| {
            n.missing_reviewers.insert(identity, since);
        });
        let condition = corral_core::attention::blocked(&task, &a);
        assert_eq!(condition.is_some(), lost, "{a:#?}");
        if let Some(condition) = condition {
            assert_eq!(condition.kind, "agent_lost");
            assert_eq!(condition.since, Some(since));
            assert!(a.progress.is_none());
        } else {
            assert_eq!(a.progress.unwrap().executor, "reviewer");
        }
    }
}

#[test]
fn finished_reviewer_loss_does_not_poison_a_healthy_replacement() {
    let mut task = reviewer(HostHealth::Lost, Some(true));
    let mut replacement = worker();
    replacement.id = "rv2".into();
    let old = &mut task.runs[0].review_runs[0];
    old.outcome = Some(RunOutcome::Done);
    old.ended = Some(at());
    task.runs[0].review_runs.push(replacement);
    let a = assessment(&task, |_| {});
    assert!(corral_core::attention::blocked(&task, &a).is_none());
    assert_eq!(a.progress.unwrap().executor, "reviewer");
}

fn gate_wait(run: &mut Run, result: GateResult, sha: &str, pane: &str) {
    run.gate = Some(
        serde_json::from_value(json!({
            "sha":sha,"pane":pane,"at":"2026-10-29T07:30:00Z","result":result
        }))
        .unwrap(),
    );
    run.obligations.waits.push(
        serde_json::from_value(json!({
            "request":{"version":1,"id":"W1","run":run.id,"condition":"Gate completes",
                "owner":"worker","source":"corral",
                "handle":{"kind":"gate","sha":"abc","pane":"gate"}},
            "registered_at":"2026-10-29T07:15:00Z",
            "observation":{"at":"2026-10-29T07:15:00Z","state":"pending","detail":""}
        }))
        .unwrap(),
    );
}

#[test]
fn terminal_gate_results_require_a_receipt_without_fabricating_one() {
    for result in [GateResult::Pass, GateResult::Fail, GateResult::Stale] {
        let mut task = task(Status::Running);
        gate_wait(&mut task.runs[0], result, "abc", "gate");
        let a = assessment(&task, |_| {});
        let condition = corral_core::attention::blocked(&task, &a).unwrap();
        assert_eq!(condition.reason_code, "wait_receipt_missing");
        assert_eq!(condition.responsible, Responsible::Pm);
        assert_eq!(
            condition.next_action.command.as_deref(),
            Some("corral sync")
        );
        assert!(
            condition
                .resume
                .iter()
                .any(|check| { check.kind == "wait_receipt" && check.satisfied == Some(false) })
        );
        assert!(condition.resume.iter().any(|check| {
            check.kind == "gate_result"
                && check.expected.as_deref() == Some(result.as_str())
                && check.satisfied == Some(true)
        }));
        assert_eq!(
            condition.since,
            Some("2026-10-29T07:15:00Z".parse().unwrap())
        );
        assert!(task.runs[0].obligations.waits[0].receipt.is_none());
    }
}

#[test]
fn running_changed_or_missing_gates_do_not_claim_a_completed_condition() {
    for (result, sha, pane, missing) in [
        (GateResult::Running, "abc", "gate", false),
        (GateResult::Pass, "other", "gate", false),
        (GateResult::Fail, "abc", "other", false),
        (GateResult::Stale, "abc", "gate", true),
    ] {
        let mut task = task(Status::Running);
        gate_wait(&mut task.runs[0], result, sha, pane);
        if missing {
            task.runs[0].gate = None;
        }
        let a = assessment(&task, |_| {});
        let condition = corral_core::attention::blocked(&task, &a).unwrap();
        assert_eq!(condition.reason_code, "wait_pending", "{a:#?}");
        assert!(
            !condition
                .resume
                .iter()
                .any(|check| check.kind == "gate_result")
        );
        assert!(task.runs[0].obligations.waits[0].receipt.is_none());
    }
}

#[test]
fn reviewer_gate_wait_uses_its_own_gate_not_the_completed_workers_gate() {
    for child_gate in [false, true] {
        let mut task = reviewer(HostHealth::Healthy, None);
        gate_wait(
            &mut task.runs[0].review_runs[0],
            GateResult::Pass,
            "abc",
            "gate",
        );
        task.runs[0].gate = task.runs[0].review_runs[0].gate.clone();
        if !child_gate {
            task.runs[0].review_runs[0].gate = None;
        }
        let a = assessment(&task, |_| {});
        let condition = corral_core::attention::blocked(&task, &a).unwrap();
        assert_eq!(
            condition.reason_code,
            if child_gate {
                "wait_receipt_missing"
            } else {
                "wait_pending"
            }
        );
        assert!(
            task.runs[0].review_runs[0].obligations.waits[0]
                .receipt
                .is_none()
        );
    }
}
