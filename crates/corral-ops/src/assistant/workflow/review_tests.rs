//! Offline protocol proof: no provider, task, pane or board review is involved.
use super::super::review;
use super::*;
use corral_core::agent::{AgentRole, AuxiliaryOutcome};
use serde_json::{Value, json};

fn fixture() -> Fixture {
    let f = Fixture::new();
    f.planning();
    let config: corral_core::agent_capabilities::AgentCapabilities =
        serde_json::from_value(json!({"assistant":{"subagents":{"enabled":true}}})).unwrap();
    f.fx.ctx
        .store
        .update_assistant_container(&f.session, |c| {
            c.state.generations[0].capabilities = Some(config.resolve(AgentRole::Assistant, None));
            Ok(())
        })
        .unwrap();
    for revision in 1..=3 {
        let expected = f.expected();
        let text = format!("# Draft r{revision}\r\nPrecise UTF-8: café 🦀\n");
        ts::committed(&f.stage(&format!("stage-{revision}"), &expected, 0, &text));
        ts::committed(&f.send(
            &format!("publish-{revision}"),
            "assistant_plan",
            f.publication(&expected, &text),
        ));
    }
    f
}
fn start(f: &Fixture) -> Value {
    ts::committed(&f.send(
        "start-review",
        "assistant_plan_review",
        json!({"action":"start", "expected":f.expected(), "input":f.owner_input()}),
    ))
    .result
}
fn event(f: &Fixture, seq: u64, r: &Value, action: &str, extra: Value) -> CallResponse {
    let mut payload = json!({"kind":"auxiliary", "action":action, "usage_kind":"explore", "request":r["invocation"],
        "plan_review":{"workflow":r["workflow"], "review":r["review"], "digest":r["digest"]}});
    for (key, value) in extra.as_object().unwrap() {
        payload[key] = value.clone();
    }
    ts::send(
        &f.fx,
        &ts::envelope(&f.host, "event", &format!("ev-1-{seq}"), payload, false),
        "event",
    )
}
fn lifecycle(f: &Fixture, seq: u64, r: &Value, value: Value) -> Value {
    ts::committed(&event(f, seq, r, "review", json!({"review_event":value}))).result
}

#[test]
fn assistant_local_review_exact_snapshot_stale_result_terminal_accounting_and_no_board_effects() {
    let f = fixture();
    let r = start(&f);
    assert_eq!(r["revision"], 3);
    assert_eq!(r["lifecycle"], "prepared");
    let exact = std::fs::read(r["snapshot_file"].as_str().unwrap()).unwrap();
    assert_eq!(BlobHash::of(&exact).as_str(), r["digest"].as_str().unwrap());
    let before = f.container();
    let retry = f.send(
        "fresh-start-review",
        "assistant_plan_review",
        json!({"action":"start", "expected":f.expected(), "input":f.owner_input()}),
    );
    assert_eq!(ts::committed(&retry).result["review"], "LR1");
    assert_eq!(f.container(), before);
    assert_eq!(
        lifecycle(&f, 101, &r, json!({"action":"claim"}))["claimed"],
        true
    );
    assert_eq!(
        lifecycle(&f, 102, &r, json!({"action":"claim"}))["claimed"],
        false
    );
    ts::committed(&event(
        &f,
        103,
        &r,
        "prepare",
        json!({"model":"openai-codex/offline-fixture"}),
    ));
    ts::committed(&event(&f, 104, &r, "started", json!({})));
    // Main-session publication remains admitted while inference is held elsewhere.
    let expected = f.expected();
    let r4 = "# r4\nChanged eviction, still no implementation consent.\n";
    ts::committed(&f.stage("stage-r4", &expected, 0, r4));
    ts::committed(&f.send("publish-r4", "assistant_plan", f.publication(&expected, r4)));
    let findings = "No blocking findings; src/cache.rs:1 needs verification.\n";
    lifecycle(
        &f,
        105,
        &r,
        json!({"action":"stage", "offset":0,"chunk":findings}),
    );
    ts::committed(&event(
        &f,
        106,
        &r,
        "finish",
        json!({"outcome":"completed"}),
    ));
    let done = lifecycle(
        &f,
        107,
        &r,
        json!({"action":"settle", "lifecycle":"completed", "summary":"Advisory only", "bytes":findings.len(),"digest":BlobHash::of(findings.as_bytes())}),
    );
    assert_eq!(done["revision"], 3);
    assert_eq!(done["current_revision"], 4);
    assert_eq!(done["freshness"], "stale");
    assert_eq!(done["accounting"], "completed");
    assert_eq!(done["source_freshness"], "not_observed");
    let read = ts::committed(&f.send(
        "read-review",
        "assistant_plan_review",
        json!({"action":"read","review":"LR1"}),
    ))
    .result;
    assert_eq!(read["text"], findings);
    let terminal = f.container();
    assert!(
        terminal
            .workflow
            .as_ref()
            .unwrap()
            .current
            .execution
            .is_none()
    );
    assert_eq!(terminal.state.usage_sessions.len(), 1);
    assert_eq!(
        terminal.state.usage_sessions[0].outcome,
        Some(AuxiliaryOutcome::Completed)
    );
    assert_eq!(
        lifecycle(
            &f,
            108,
            &r,
            json!({"action":"settle","lifecycle":"failed","summary":"late"})
        )["lifecycle"],
        "completed"
    );
    assert_eq!(f.container(), terminal, "first terminal wins");
    let reopened = corral_core::Store::open(f.fx._tmp.path()).unwrap();
    let container = reopened.get_assistant_container(&f.session).unwrap();
    assert_eq!(
        review::read(&reopened, &f.session, &container, Some("LR1"), 0).unwrap()["freshness"],
        "stale"
    );
    f.no_tasks();
}
