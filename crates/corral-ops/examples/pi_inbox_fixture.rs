//! Disposable canonical store for scripts/qa-pi-inbox.py. Never opens a terminal engine.
use anyhow::{Context, Result, ensure};
use corral_core::agent::{AgentBinding, AgentRole, AgentRunState, HostHealth, paths};
use corral_core::{Run, RunOutcome, Status, Store};
use serde_json::json;

fn main() -> Result<()> {
    let root = std::env::args_os()
        .nth(1)
        .context("private fixture directory required")?;
    let root = std::path::Path::new(&root);
    ensure!(
        !root.join(".corral").exists(),
        "refusing an existing workspace"
    );
    let store = Store::init(root)?;
    let task = store.create(
        "Inbox fixture",
        Some("demo"),
        Status::Running,
        "Isolated QA only",
    )?;
    store.update(&task.id, |task| {
        let mut run: Run = serde_json::from_value(json!({
            "id":"R1", "agent":"fixture", "harness":"corral-pi", "worktree":root,
            "branch":"fixture", "base":"main", "base_sha":"fixture", "started":"2026-11-02T00:00:00Z"
        }))?;
        let mut host = AgentRunState::new(AgentBinding {
            role: AgentRole::Worker, store: paths::store_id(&store), project: "demo".into(),
            generation: 1, health: HostHealth::Healthy, callback: None, closing: false,
        });
        for (n, state, text) in [(1, "open", "Choose canonical storage?"), (2, "answered", "Which route was chosen?"), (3, "cancelled", "Withdraw the obsolete proposal?")] {
            let mut question = json!({
                "version":1, "key":format!("{}/R1/Q{n}", task.id), "revision":n,
                "asked":{"request_id":format!("rq-fixture-{n}"), "payload_sha256":"fixture", "session_id":"fixture", "generation":1, "at":"2026-11-02T00:00:00Z"},
                "kind":"clarification", "blocking":false, "text":text, "state":state,
                "wake":{"route":"none", "key":"fixture", "state":"posted"}
            });
            if state == "answered" { question["answer"] = json!({"by":"owner", "text":"Use canonical storage", "at":"2026-11-02T00:01:00Z"}); }
            host.questions.push(serde_json::from_value(question)?);
        }
        run.agent_host = Some(Box::new(host));
        run.obligations.decisions.push(serde_json::from_value(json!({
            "request":{"version":1,"id":"owner-choice","run":"R1","question":"Approve the scoped alternative?","requested_action":"Owner decision in established flow","owner":"owner","source":"worker","deadline":null},
            "registered_at":"2026-11-02T00:00:00Z", "observation":{"at":"2026-11-02T00:00:00Z","state":"pending","detail":"awaiting actual owner answer"},"receipt":null
        }))?);
        // Historical container remains closed even if its question record still says open.
        let mut old = run.clone(); old.id = "R0".into(); old.outcome = Some(RunOutcome::Superseded);
        old.obligations.decisions.clear();
        if let Some(host) = old.agent_host.as_mut() {
            host.questions.truncate(1);
            for question in &mut host.questions { question.key = format!("{}/R0/Q1", task.id); }
        }
        task.runs = vec![old, run];
        Ok(())
    })?;
    store.create(
        "Unrelated task must remain unchanged",
        Some("other"),
        Status::Backlog,
        "No inspection side effects",
    )?;
    println!("{}", task.id);
    Ok(())
}
