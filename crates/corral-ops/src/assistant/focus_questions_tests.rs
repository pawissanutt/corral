use super::*;
use corral_core::agent::{AgentBinding, AgentRole, AgentRunState, HostHealth};
use serde_json::json;

fn state(role: AgentRole, label: &str) -> Box<AgentRunState> {
    let mut state = AgentRunState::new(AgentBinding {
        role,
        store: "st-fixture".into(),
        project: "corral".into(),
        generation: 1,
        health: HostHealth::Healthy,
        callback: None,
        closing: false,
    });
    for (n, status) in ["open", "answered", "withdrawn", "cancelled", "expired"]
        .iter()
        .enumerate()
    {
        state.questions.push(serde_json::from_value(json!({
            "version":1, "key":format!("{label}/Q{n}"), "revision":0,
            "asked":{"request_id":"q", "payload_sha256":"hash", "session_id":"session", "generation":1, "at":"2026-10-01T00:00:00Z"},
            "kind":"clarification", "blocking":false, "text":format!("{label}-{status}-text"), "state":status,
            "wake":{"route":"none", "key":"wake", "state":"posted"}
        })).unwrap());
    }
    let mut terminal = state.questions[0].clone();
    terminal.key = format!("{label}/Q6");
    terminal.text = format!("{label}-terminal-text");
    terminal.terminal = Some(corral_core::agent::QuestionTerminal {
        state: corral_core::agent::QuestionState::Cancelled,
        reason: "closed".into(),
        at: "2026-10-02T00:00:00Z".parse().unwrap(),
    });
    state.questions.push(terminal);
    Box::new(state)
}

fn run(role: AgentRole, label: &str) -> corral_core::Run {
    let mut run: corral_core::Run = serde_json::from_value(json!({
        "id":label, "agent":"pi", "worktree":"/fixture", "branch":"test", "base":"main", "base_sha":"abc", "started":"2026-10-01T00:00:00Z"
    })).unwrap();
    run.agent_host = Some(state(role, label));
    run
}

#[test]
fn brief_questions_cover_effectively_pending_worker_reviewer_and_planning_containers() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::init(temp.path()).unwrap();
    let task = store
        .create("questions", None, corral_core::Status::Running, "spec")
        .unwrap();
    store.update(&task.id, |task| {
        let mut worker = run(AgentRole::Worker, "R1");
        worker.review_runs.push(run(AgentRole::Reviewer, "review-child"));
        task.runs.push(worker);
        task.plan_runs.push(serde_json::from_value(json!({
            "id":"D1", "profile":"pi", "harness":"pi-native", "spec_revision":"spec", "started":"2026-10-01T00:00:00Z",
            "output_dir":"/fixture", "rounds":0, "state":"planning", "agent_host":state(AgentRole::Planner, "planner")
        })).unwrap());
        task.plans.push(serde_json::from_value(json!({
            "id":"P1", "spec_revision":corral_core::plan_contract_revision(task).unwrap(), "run":"D1",
            "artifact":{"run":"D1", "kind":"plan", "title":"plan", "path":"artifacts/plan.md", "source":"plan.md", "sha256":"abc", "bytes":10, "added":"2026-10-01T00:00:00Z"},
            "document":{"goal":"goal", "non_goals":[], "decisions":[], "affected_paths":[], "contracts":[], "dependencies":[], "acceptance":[], "verification":[], "owner_decisions":[]},
            "reviews":[{"id":"PR1", "reviewer":"pi", "policy_digest":"policy", "at":"2026-10-01T00:00:00Z", "agent_host":state(AgentRole::PlanReviewer, "plan-reviewer")}]
        })).unwrap());
        Ok(())
    }).unwrap();
    let brief = task_brief(&store, &task.id).unwrap();
    let questions = brief
        .split("## Open questions\n")
        .nth(1)
        .unwrap()
        .split("## Run obligations")
        .next()
        .unwrap();
    for label in ["R1", "review-child", "planner", "plan-reviewer"] {
        assert!(
            questions.contains(&format!("{label}-open-text")),
            "{questions}"
        );
        for state in ["answered", "withdrawn", "cancelled", "expired", "terminal"] {
            assert!(
                !questions.contains(&format!("{label}-{state}-text")),
                "{questions}"
            );
        }
    }
    assert!(brief.len() <= BRIEF_BYTES);
    // Recorded open questions become non-pending immediately on container closure,
    // without a lifecycle sweep rewriting their recorded states.
    store
        .update(&task.id, |task| {
            task.status = corral_core::Status::Dropped;
            Ok(())
        })
        .unwrap();
    let brief = task_brief(&store, &task.id).unwrap();
    let questions = brief
        .split("## Open questions\n")
        .nth(1)
        .unwrap()
        .split("## Run obligations")
        .next()
        .unwrap();
    assert!(!questions.contains("open-text"), "{questions}");
    assert_eq!(
        store.get(&task.id).unwrap().runs[0]
            .agent_host
            .as_ref()
            .unwrap()
            .questions[0]
            .state,
        corral_core::agent::QuestionState::Open
    );
}
