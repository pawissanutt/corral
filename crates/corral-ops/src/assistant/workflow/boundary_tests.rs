// Uses the foundation's tiny disposable authenticated store, no panes or provider calls.
use super::*;
use crate::agent::test_support as ts;
use serde_json::json;

fn rejected(response: &crate::agent::CallResponse, code: crate::agent::RejectCode) {
    assert_eq!(
        response.rejection().map(|r| r.code),
        Some(code),
        "{response:?}"
    );
}

#[test]
fn assistant_boundary_orders_entry_and_actual_activity_settlement() {
    let f = Fixture::new();
    let input = f.owner_input();
    ts::committed(&f.boundary(
        51,
        json!({"action": "start", "id": "g1-write", "tool": "write", "authority_epoch": 0}),
    ));
    let entry = ts::committed(&f.boundary(
        52,
        json!({"action": "enter", "input": input, "expected": null}),
    ));
    assert_eq!(entry.result["phase"], "entering");
    let before = f.container();
    rejected(
        &f.boundary(
            53,
            json!({"action": "start", "id": "late", "tool": "bash", "authority_epoch": 1}),
        ),
        crate::agent::RejectCode::OpNotAllowed,
    );
    assert_eq!(f.container(), before);
    // Neither a host assertion nor generation fencing proves the old effect ended.
    rejected(
        &f.boundary(54, json!({"action": "drained", "expected": f.expected()})),
        crate::agent::RejectCode::BadRequest,
    );
    assert_eq!(f.container(), before);
    ts::committed(&f.boundary(
        55,
        json!({"action": "settle", "id": "g1-write", "uncertain": true}),
    ));
    assert!(boundary::activities(&f.fx.ctx.store, &f.session).unwrap()[0].uncertain);
    rejected(
        &f.boundary(56, json!({"action": "drained", "expected": f.expected()})),
        crate::agent::RejectCode::BadRequest,
    );
    ts::committed(&f.boundary(
        57,
        json!({"action": "settle", "id": "g1-write", "uncertain": false}),
    ));
    let done =
        ts::committed(&f.boundary(58, json!({"action": "drained", "expected": f.expected()})));
    assert_eq!(done.result["phase"], "planning");
    assert_eq!(done.result["authority_epoch"], 2);
    let expected = f.expected();
    ts::committed(&f.stage("own-draft", &expected, 0, "# Draft"));
    ts::committed(&f.send("todo-bookkeeping", "todo", json!({"op": "init", "phases": [{
        "id": "research", "label": "Research", "steps": [{"id": "inspect", "label": "Inspect code"}]
    }]})));
    assert_eq!(
        f.expected(),
        expected,
        "Todo is not design/execution authority"
    );
    // Generic agreement/permission/owner action cannot disable the fence.
    rejected(
        &f.send(
            "owner-bypass",
            "owner_action",
            json!({"kind": "approve", "target": "T-1", "input": input}),
        ),
        crate::agent::RejectCode::OpNotAllowed,
    );
    assert!(!boundary::ordinary(&f.container()).unwrap());
    f.no_tasks();
}

#[test]
fn assistant_boundary_generation_reload_cannot_clear_a_previous_executor_or_replay_authority() {
    let f = Fixture::new();
    let input = f.owner_input();
    ts::committed(&f.boundary(
        51,
        json!({"action": "start", "id": "retained-job", "tool": "job_start", "authority_epoch": 0}),
    ));
    ts::committed(&f.boundary(
        52,
        json!({"action": "enter", "input": input, "expected": null}),
    ));
    // Reopening the durable store does not drop the old-generation ledger row.
    let reopened = corral_core::Store::open_unobserved(f.fx.ctx.store.root()).unwrap();
    assert_eq!(
        boundary::activities(&reopened, &f.session).unwrap().len(),
        1
    );
    let before = f.container();
    let error =
        f.fx.ctx
            .store
            .update_assistant_container(&f.session, |container| {
                boundary::apply(
                    &f.fx.ctx.store,
                    &f.session,
                    container,
                    2,
                    serde_json::from_value(
                        json!({"action": "settle", "id": "retained-job", "uncertain": false}),
                    )
                    .unwrap(),
                )?;
                Ok(())
            });
    assert!(error.is_err());
    assert_eq!(f.container(), before);
    assert_eq!(
        boundary::activities(&reopened, &f.session).unwrap().len(),
        1
    );
    let wrong = f.send(
        "model-drain",
        "assistant_plan",
        json!({"action": "drained", "expected": f.expected()}),
    );
    rejected(&wrong, crate::agent::RejectCode::BadRequest);
    f.no_tasks();
}
