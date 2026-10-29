use serde_json::{Value, json};

use super::test_support::{self as ts, Host, committed, envelope, hello, send};
use super::{BindTarget, tool_request_id};

fn init() -> Value {
    json!({"op":"init","phases":[{"id":"work","label":"Work","steps":[
        {"id":"build","label":"Build the feature"},
        {"id":"extra","label":"Optional work"}
    ]}]})
}

fn todo(fx: &ts::Fx, host: &Host, id: &str, payload: Value) -> super::CallResponse {
    let request = tool_request_id(&host.identity.session_id, id);
    send(
        fx,
        &envelope(host, "todo", &request, payload, false),
        "todo",
    )
}

#[test]
fn todo_worker_updates_canonical_progress_once_per_request() {
    // Given a bound worker and an empty canonical ledger.
    let fx = ts::fx();
    let task = ts::worker_task(&fx, "p1");
    let host = ts::bind(
        &fx,
        &BindTarget::Worker {
            task: task.clone(),
            run: "R1".into(),
        },
        "p1",
        "worker",
    );
    committed(&hello(&fx, &host));
    // When the same init request is delivered twice.
    let first = committed(&todo(&fx, &host, "init", init()));
    let replay = todo(&fx, &host, "init", init());
    // Then one canonical revision and the original receipt are retained.
    assert_eq!(first.result["revision"], 1);
    assert_eq!(replay.receipt(), Some(&first));
    assert!(matches!(
        replay,
        super::CallResponse::Receipt { replayed: true, .. }
    ));
    let stored = fx.ctx.store.get(&task).unwrap();
    assert_eq!(
        stored
            .current_run()
            .unwrap()
            .progress
            .as_ref()
            .unwrap()
            .revision,
        1
    );
}

#[test]
fn todo_pm_updates_own_ledger_without_changing_worker_progress() {
    // Given a PM and a separate worker task.
    let fx = ts::fx();
    let task = ts::worker_task(&fx, "p1");
    let before = ts::task_bytes(&fx, &task);
    let host = ts::bind(
        &fx,
        &BindTarget::Pm {
            project: "corral".into(),
        },
        "p2",
        "pm",
    );
    committed(&hello(&fx, &host));
    // When the PM initializes its ledger.
    let result = todo(&fx, &host, "init", init());
    // Then the receipt commits without touching a task ledger.
    assert_eq!(committed(&result).result["revision"], 1);
    assert_eq!(ts::task_bytes(&fx, &task), before);
}

#[test]
fn todo_ledgers_survive_reopen_resume_and_retain_drop_reasons() {
    // Given worker and PM bindings with independently initialized ledgers.
    for pm in [false, true] {
        let mut fx = ts::fx();
        let id = ts::worker_task(&fx, "p1");
        let target = if pm {
            BindTarget::Pm {
                project: "corral".into(),
            }
        } else {
            BindTarget::Worker {
                task: id.clone(),
                run: "R1".into(),
            }
        };
        let host = ts::bind(&fx, &target, "p1", "s1");
        committed(&hello(&fx, &host));
        committed(&todo(&fx, &host, "init", init()));
        committed(&todo(
            &fx,
            &host,
            "start",
            json!({"op":"start","step":"build"}),
        ));
        committed(&todo(
            &fx,
            &host,
            "done",
            json!({"op":"done","step":"build"}),
        ));
        committed(&todo(
            &fx,
            &host,
            "drop",
            json!({
                "op":"drop","step":"extra","reason":"Covered by the targeted check"
            }),
        ));
        let append =
            json!({"op":"append","phase":"work","steps":[{"id":"verify","label":"Verify result"}]});
        committed(&todo(&fx, &host, "append", append));
        // When the store is reopened and the same container explicitly resumes.
        fx.ctx.store = corral_core::Store::open_unobserved(fx.ctx.store.root()).unwrap();
        let resumed = ts::bind(&fx, &target, "p1", "s2");
        committed(&hello(&fx, &resumed));
        let result = committed(&todo(
            &fx,
            &resumed,
            "resume",
            json!({"op":"start","step":"verify"}),
        ));
        // Then both durable state and a late replay retain exactly the earlier changes.
        let progress: corral_core::RunProgress =
            serde_json::from_value(result.result["progress"].clone()).unwrap();
        assert_eq!(progress.revision, 6);
        assert_eq!(
            progress.steps().nth(1).unwrap().status,
            corral_core::StepState::Dropped {
                reason: "Covered by the targeted check".into()
            }
        );
        let replay = todo(&fx, &host, "init", init());
        assert!(matches!(
            replay,
            super::CallResponse::Receipt { replayed: true, .. }
        ));
        if pm {
            let value =
                super::install::state_value(&fx.ctx, &super::cli::AgentTarget::Pm, &json!({}))
                    .unwrap();
            assert_eq!(value["container"]["todo"]["revision"], 6);
            assert_eq!(value["container"]["todo_enabled"], true);
        } else {
            let task = fx.ctx.store.get(&id).unwrap();
            assert_eq!(
                task.current_run().unwrap().progress.as_ref(),
                Some(&progress)
            );
        }
        assert_eq!(ts::events(&fx, "progress_updated"), 6);
    }
}

#[test]
fn todo_strict_and_role_disable_apply_at_generation_boundaries() {
    use corral_core::config::agent_capabilities::Preset;
    // Given each supported role with strict or its own explicit disable.
    for pm in [false, true] {
        for strict in [false, true] {
            let mut fx = ts::fx();
            let task = ts::worker_task(&fx, "p1");
            let target = if pm {
                BindTarget::Pm {
                    project: "corral".into(),
                }
            } else {
                BindTarget::Worker {
                    task: task.clone(),
                    run: "R1".into(),
                }
            };
            let active = ts::bind(&fx, &target, "p1", "active");
            committed(&hello(&fx, &active));
            if strict {
                fx.ctx.store.config.agent.capabilities.preset = Preset::Strict;
            } else if pm {
                fx.ctx.store.config.agent.capabilities.pm.todo = Some(false);
            } else {
                fx.ctx.store.config.agent.capabilities.worker.todo = Some(false);
            }
            // When config changes during a generation, then on explicit resume.
            committed(&todo(&fx, &active, "init", init()));
            let host = ts::bind(&fx, &target, "p1", "disabled");
            let hello = committed(&hello(&fx, &host));
            let result = todo(&fx, &host, "new", json!({"op":"done","step":"build"}));
            // Then active settings stay stable and the new generation removes the op.
            assert!(
                !hello.result["ops"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("todo"))
            );
            assert_eq!(ts::code(&result), Some("op_not_allowed"));
            assert_eq!(ts::events(&fx, "progress_updated"), 1);
        }
    }
}

#[test]
fn todo_foreign_roles_and_model_selected_identity_cannot_mutate_ledgers() {
    // Given a valid worker plus a separate worker's task.
    let fx = ts::fx();
    let task = ts::worker_task(&fx, "p1");
    let other = ts::worker_task(&fx, "p2");
    let host = ts::bind(
        &fx,
        &BindTarget::Worker {
            task,
            run: "R1".into(),
        },
        "p1",
        "s",
    );
    committed(&hello(&fx, &host));
    let before = ts::task_bytes(&fx, &other);
    // When a caller tries foreign authority or supplies its own target.
    for role in [
        corral_core::agent::AgentRole::Reviewer,
        corral_core::agent::AgentRole::Planner,
        corral_core::agent::AgentRole::PlanReviewer,
    ] {
        let mut foreign = host.clone();
        foreign.identity.role = role;
        match role {
            corral_core::agent::AgentRole::Reviewer => {
                foreign.identity.review = Some("R1-review1".into());
                foreign.identity.callback = Some("g1:c1".into());
            }
            corral_core::agent::AgentRole::Planner => foreign.identity.run = Some("D1".into()),
            corral_core::agent::AgentRole::PlanReviewer => {
                foreign.identity.run = Some("P1".into());
                foreign.identity.review = Some("PR1".into());
            }
            corral_core::agent::AgentRole::Worker | corral_core::agent::AgentRole::Pm => {}
        }
        assert_eq!(
            ts::code(&todo(&fx, &foreign, role.as_str(), init())),
            Some("op_not_allowed")
        );
    }
    let mut payload = init();
    payload["task"] = json!(other);
    assert_eq!(
        ts::code(&todo(&fx, &host, "target", payload)),
        Some("bad_request")
    );
    // Then no target ledger or todo event was written.
    assert_eq!(ts::task_bytes(&fx, &other), before);
    assert_eq!(ts::events(&fx, "progress_updated"), 0);
}

#[test]
fn todo_rejects_invalid_transitions_without_partial_changes() {
    // Given an initialized worker ledger.
    let fx = ts::fx();
    let task = ts::worker_task(&fx, "p1");
    let host = ts::bind(
        &fx,
        &BindTarget::Worker {
            task: task.clone(),
            run: "R1".into(),
        },
        "p1",
        "s",
    );
    committed(&hello(&fx, &host));
    committed(&todo(&fx, &host, "init", init()));
    committed(&todo(
        &fx,
        &host,
        "start",
        json!({"op":"start","step":"build"}),
    ));
    let before = ts::task_bytes(&fx, &task);
    // When commands would erase history or create invalid progress.
    let invalid = [
        init(),
        json!({"op":"start","step":"extra"}),
        json!({"op":"drop","step":"extra","reason":" "}),
        json!({"op":"done","step":"missing"}),
        json!({"op":"append","phase":"work","steps":[{"id":"build","label":"Duplicate"}]}),
        json!({"op":"append","phase":"work","steps":[{"id":"c","label":"Unknown criterion","criteria":["A99"]}]}),
    ];
    for (n, payload) in invalid.into_iter().enumerate() {
        assert_eq!(
            ts::code(&todo(&fx, &host, &format!("invalid-{n}"), payload)),
            Some("bad_request")
        );
    }
    // Then the ledger and its receipts stay unchanged.
    assert_eq!(ts::task_bytes(&fx, &task), before);
    assert_eq!(ts::events(&fx, "progress_updated"), 2);
}
