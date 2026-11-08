//! One isolated owner-CLI journey. Input provenance is an explicit test fixture,
//! not real native owner consent; C4's authenticated choice consumer is not enabled.
use super::{Fixture, Value, json};
use corral_core::{
    assistant::{NewOwnerInput, OwnerInputSource},
    assistant_workflow::{ScopeOwner, WorkflowPhase},
    blob_ref::BlobHash,
};

fn cli(fx: &Fixture, args: &[&str]) -> Value {
    serde_json::from_str(&fx.ok(args, &[])).unwrap()
}
fn selection(fx: &Fixture, name: &str, value: &Value) -> String {
    let path = fx.root.join(name);
    std::fs::write(&path, serde_json::to_vec(&value["choice"]).unwrap()).unwrap();
    path.to_str().unwrap().to_owned()
}
fn current(fx: &Fixture, package: &str) -> Value {
    cli(fx, &["assistant", "handoff", "show", package])
}
fn bound(fx: &Fixture, action: &str, task: &str, package: &str) -> Value {
    let now = current(fx, package);
    cli(
        fx,
        &[
            "assistant",
            "handoff",
            action,
            task,
            "--transfer",
            package,
            "--revision",
            &now["revision"].as_u64().unwrap().to_string(),
            "--contract",
            now["contract_revision"].as_str().unwrap(),
        ],
    )
}

#[test]
fn guarded_owner_cli_saves_continues_and_returns_same_plan_only_scope_without_launching() {
    let fx = Fixture::new();
    let store = fx.store();
    let mut input = String::new();
    // No host/process is launched. Generation-zero fixture proves only the service
    // and CLI contract, not a real owner message or live SDK transfer.
    store
        .update_assistant_container("A-1", |c| {
            let text = "test fixture: continue research without implementation";
            input = c
                .journal_owner_input(
                    "A-1",
                    NewOwnerInput {
                        generation: c.state.binding.generation,
                        turn: None,
                        source: OwnerInputSource::Pane,
                        sha256: BlobHash::of(text.as_bytes()).to_string(),
                        text: text.into(),
                        truncated: false,
                        command: None,
                        answers: None,
                        request: "fixture-input-1".into(),
                    },
                )
                .id;
            Ok(())
        })
        .unwrap();
    cli(
        &fx,
        &["assistant", "handoff", "init", "A-1", "--input", &input],
    );
    let request = fx.root.join("request.json");
    std::fs::write(&request,serde_json::to_vec(&json!({
        "project":"corral","scope":["src"],"goal":"plan_continue",
        "context":{"mode":"task","brief":{
            "goal":"research only","constraints":["no implementation"],"decisions":[],"completed":[],
            "remaining":["planning"],"findings":[],"failed_approaches":[],"assumptions":[],"evidence":[],
            "questions":["which approach?"],"mandatory_blockers":[],"next_action":"continue planning"
        },"history":null}
    })).unwrap()).unwrap();
    let prepared = cli(
        &fx,
        &[
            "assistant",
            "handoff",
            "prepare",
            "A-1",
            "--request",
            request.to_str().unwrap(),
        ],
    );
    let package = prepared["package"].as_str().unwrap();
    let selected = cli(
        &fx,
        &[
            "assistant",
            "handoff",
            "preview",
            package,
            "--title",
            "Retained research",
            "--save",
        ],
    );
    let path = selection(&fx, "selected.json", &selected);
    let digest = selected["digest"].as_str().unwrap();
    let saved = cli(
        &fx,
        &[
            "assistant",
            "handoff",
            "commit",
            "--choice",
            &path,
            "--digest",
            digest,
        ],
    );
    let task = saved["id"].as_str().unwrap();
    assert!(saved["dispatch_hold"].as_bool().unwrap());
    let replay = cli(
        &fx,
        &[
            "assistant",
            "handoff",
            "commit",
            "--choice",
            &path,
            "--digest",
            digest,
        ],
    );
    assert_eq!(replay["id"], saved["id"]);
    assert_eq!(fx.store().list().unwrap().len(), 2); // pre-existing unrelated T-001 plus this card
    let rejected = fx.run(
        &["assistant", "handoff", "show", package],
        &[("CORRAL_ASSISTANT", "A-1")],
    );
    assert!(
        !rejected.status.success(),
        "managed assistant must not obtain owner-only transfer entry"
    );
    let next = cli(
        &fx,
        &[
            "assistant",
            "handoff",
            "continue-preview",
            task,
            "--goal",
            "plan-continue",
        ],
    );
    let next_path = selection(&fx, "continue.json", &next);
    let active = cli(
        &fx,
        &[
            "assistant",
            "handoff",
            "start",
            "--choice",
            &next_path,
            "--digest",
            next["digest"].as_str().unwrap(),
        ],
    );
    assert_eq!(active["id"], task);
    assert!(!fx.store().get(task).unwrap().dispatch_hold);
    bound(&fx, "save", task, package);
    // Old consumed continuation must not release a subsequent hold.
    cli(
        &fx,
        &[
            "assistant",
            "handoff",
            "start",
            "--choice",
            &next_path,
            "--digest",
            next["digest"].as_str().unwrap(),
        ],
    );
    assert!(fx.store().get(task).unwrap().dispatch_hold);
    let returned = bound(&fx, "take-back", task, package);
    assert_eq!(returned["state"], "ready");
    assert_eq!(returned["task"], task);
    assert_eq!(returned["plan_only"], true);
    let reopened = fx.store();
    let held = reopened.get(task).unwrap();
    assert!(
        held.dispatch_hold && !held.queued && held.runs.is_empty() && held.plan_runs.is_empty()
    );
    assert!(!held.cowork.as_ref().unwrap().owner_confirmed);
    let source = reopened
        .get_assistant_container("A-1")
        .unwrap()
        .workflow
        .unwrap()
        .current;
    assert_eq!(source.owner, ScopeOwner::Local);
    assert_eq!(source.phase, WorkflowPhase::Planning);
    assert!(source.execution.is_none());
    eprintln!(
        "OWNER_CLI: saved/replayed/continued/held/returned {task}; one retained card, zero planner/worker launches, no implementation/Ship authority; fixture provenance only"
    );
}
