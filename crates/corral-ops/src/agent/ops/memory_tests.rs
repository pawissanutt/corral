use super::*;
use crate::agent::{BindTarget, bridge::interrupted, test_support::*};
use corral_core::agent::ResumeClass;
use serde_json::{Value, json};

fn selector() -> MemorySelector {
    MemorySelector {
        scope: MemoryScope::Role(corral_core::memory::MemoryRole::Worker),
        id: "working".into(),
    }
}
fn entry(body: &str) -> MemoryEntry {
    serde_json::from_value(json!({"metadata": {
        "version":1, "id":"working", "scope":{"scope":"role","key":"worker"},
        "title":"Working guidance", "kind":"guidance", "summary":"Check evidence",
        "provenance":{"kind":"agent_inference","sources":["task:T-645"]},
        "updated_at":"2026-01-01T00:00:00Z", "author":"owner-forged"
    }, "body":body}))
    .unwrap()
}
fn setup() -> (Fx, Host, BindTarget) {
    let fx = fx();
    let task = worker_task(&fx, "p1");
    let target = BindTarget::Worker {
        task,
        run: "R1".into(),
    };
    let host = bind(&fx, &target, "p1", "memory-session");
    committed(&hello(&fx, &host));
    (fx, host, target)
}
fn put(expected: Option<&str>, body: &str) -> Value {
    json!({"selector":selector(), "entry":entry(body), "expected_hash":expected})
}
fn request(fx: &Fx, host: &Host, op: &str, id: &str, payload: Value) -> CallResponse {
    let id = crate::agent::tool_request_id(&host.identity.session_id, id);
    send(fx, &envelope(host, op, &id, payload, false), op)
}
fn read(fx: &Fx) -> Option<corral_core::memory::MemoryRead> {
    fx.ctx.store.memory_read(&selector(), 0, 4096).unwrap()
}
#[test]
fn memory_operations_bind_author_cas_replay_and_fence() {
    let (fx, host, target) = setup();
    let created = committed(&request(
        &fx,
        &host,
        "memory_put",
        "create",
        put(None, "first"),
    ));
    let hash = created.result["hash"].as_str().unwrap();
    assert!(
        created.result["metadata"]["author"]
            .as_str()
            .unwrap()
            .starts_with("worker:")
    );
    let listed = committed(&request(
        &fx,
        &host,
        "memory_list",
        "list",
        json!({"scope":selector().scope}),
    ));
    assert_eq!(listed.result["entries"][0]["hash"], hash);
    let page = committed(&request(
        &fx,
        &host,
        "memory_read",
        "read",
        json!({"selector":selector()}),
    ));
    assert_eq!(page.result["body"], "first");
    let changed = committed(&request(
        &fx,
        &host,
        "memory_put",
        "update",
        put(Some(hash), "second"),
    ));
    assert_eq!(
        code(&request(
            &fx,
            &host,
            "memory_put",
            "stale",
            put(Some(hash), "lost")
        )),
        Some("bad_request")
    );
    let replay = request(&fx, &host, "memory_put", "create", put(None, "first"));
    assert!(matches!(
        replay,
        CallResponse::Receipt { replayed: true, .. }
    ));
    assert_eq!(replay.receipt().unwrap().result, created.result);
    assert_eq!(read(&fx).unwrap().body, "second");
    let remove = json!({"selector":selector(),"expected_hash":changed.result["hash"]});
    committed(&request(
        &fx,
        &host,
        "memory_remove",
        "remove",
        remove.clone(),
    ));
    assert!(read(&fx).is_none());
    committed(&request(
        &fx,
        &host,
        "memory_put",
        "recreate",
        put(None, "newer"),
    ));
    request(&fx, &host, "memory_remove", "remove", remove);
    assert_eq!(read(&fx).unwrap().body, "newer");
    bind(&fx, &target, "p1", "next-generation");
    assert_eq!(
        code(&request(
            &fx,
            &host,
            "memory_put",
            "fenced",
            put(None, "no")
        )),
        Some("agent_fenced")
    );
    assert!(matches!(
        request(&fx, &host, "memory_put", "create", put(None, "first")),
        CallResponse::Receipt { replayed: true, .. }
    ));
}

#[test]
fn memory_pm_assistant_and_worker_use_authenticated_scopes() {
    let mut fx = fx();
    for project in ["corral", "other"] {
        fx.ctx.store.config.projects.insert(
            project.into(),
            serde_json::from_value(json!({"path":fx.ctx.store.root()})).unwrap(),
        );
    }
    fx.ctx.store.save_config().unwrap();
    let task = worker_task(&fx, "p1");
    let assistant = fx
        .ctx
        .store
        .create_assistant("pi", None, Some("a-pane"), None)
        .unwrap();
    for (target, pane, role) in [
        (
            BindTarget::Pm {
                project: "corral".into(),
            },
            "pm-pane",
            "pm",
        ),
        (
            BindTarget::Assistant {
                session: assistant.id,
            },
            "a-pane",
            "assistant",
        ),
        (
            BindTarget::Worker {
                task,
                run: "R1".into(),
            },
            "p1",
            "worker",
        ),
    ] {
        let host = bind(&fx, &target, pane, &format!("scoped-{role}"));
        committed(&hello(&fx, &host));
        let mut payload = put(None, role);
        payload["selector"]["scope"] = json!({"scope":"project","key":"corral"});
        payload["entry"]["metadata"]["scope"] = payload["selector"]["scope"].clone();
        payload["selector"]["id"] = json!(role);
        payload["entry"]["metadata"]["id"] = json!(role);
        committed(&request(&fx, &host, "memory_put", "project", payload));
        let mut workspace = put(None, role);
        workspace["selector"]["scope"] = json!({"scope":"workspace"});
        workspace["entry"]["metadata"]["scope"] = json!({"scope":"workspace"});
        workspace["selector"]["id"] = json!(role);
        workspace["entry"]["metadata"]["id"] = json!(role);
        let response = request(&fx, &host, "memory_put", "workspace", workspace);
        if role == "worker" {
            assert_eq!(code(&response), Some("out_of_scope"));
        } else {
            assert!(response.receipt().is_some());
        }
    }
}

#[test]
fn memory_async_recovery_updates_keep_receipt_classes_and_do_not_touch_files() {
    use corral_core::agent::ReceiptState;
    let (fx, host, target) = setup();
    let created = committed(&request(
        &fx,
        &host,
        "memory_put",
        "create",
        put(None, "newer bytes"),
    ));
    let hash = created.result["hash"].as_str().unwrap();
    let task_id = target.container().task().unwrap().to_owned();
    for op in ["memory_put", "memory_remove"] {
        for (saved, expected) in [
            (ReceiptState::Committed, "result"),
            (ReceiptState::Rejected, "rejected"),
            (ReceiptState::Uncertain, "outcome_unknown"),
        ] {
            let mut receipt = created.clone();
            receipt.request_id =
                crate::agent::tool_request_id("updates", &format!("{op}-{expected}"));
            receipt.op = op.into();
            receipt.state = saved;
            receipt.evidence.push(serde_json::from_value(json!({"state":"in_progress","source":"recovery_notice","at":jiff::Timestamp::now()})).unwrap());
            fx.ctx
                .store
                .update(&task_id, |task| {
                    task.runs[0]
                        .agent_host
                        .as_mut()
                        .unwrap()
                        .receipts
                        .push(receipt);
                    Ok(())
                })
                .unwrap();
            assert_eq!(
                crate::agent::recovery::post_updates(&fx.ctx, &task_id).unwrap(),
                1
            );
            let inbox = corral_core::inbox::Inbox::new(
                &fx.ctx.store,
                corral_core::inbox::Recipient::Run {
                    task: task_id.clone(),
                    run: "R1".into(),
                },
            )
            .unwrap();
            let all = inbox.messages().unwrap();
            let last = all.last().unwrap();
            assert_eq!(last.subject.as_ref().unwrap()["class"], expected);
            assert_eq!(read(&fx).unwrap().item.hash, hash);
            assert_eq!(
                crate::agent::recovery::post_updates(&fx.ctx, &task_id).unwrap(),
                0
            );
        }
    }
}

#[test]
fn memory_fence_between_admission_and_commit_refuses_file_io() {
    let (fx, host, target) = setup();
    // A deterministic admission barrier: advance the generation before final transaction.
    receipts::FAILPOINT.with(|hook| {
        let root = fx.ctx.store.root().to_owned();
        *hook.borrow_mut() = Some(Box::new(move |name| {
            if name == "agent_admitted_before_commit" {
                let store = corral_core::Store::open(&root).unwrap();
                crate::agent::open_generation(&store, &target, "p1").unwrap();
            }
        }));
    });
    let refused = request(
        &fx,
        &host,
        "memory_put",
        "fence-at-commit",
        put(None, "forbidden"),
    );
    receipts::FAILPOINT.with(|hook| *hook.borrow_mut() = None);
    assert_eq!(code(&refused), Some("agent_fenced"));
    assert!(read(&fx).is_none());
}

#[test]
fn memory_workers_cannot_write_workspace_or_other_roles() {
    let (fx, host, _) = setup();
    for scope in [
        MemoryScope::Workspace,
        MemoryScope::Role(corral_core::memory::MemoryRole::Pm),
        MemoryScope::Project("foreign".into()),
    ] {
        let mut payload = put(None, "forbidden");
        payload["selector"]["scope"] = json!(scope);
        payload["entry"]["metadata"]["scope"] = json!(scope);
        assert_eq!(
            code(&request(
                &fx,
                &host,
                "memory_put",
                &format!("deny-{scope:?}"),
                payload
            )),
            Some("out_of_scope")
        );
    }
}

#[test]
fn memory_crashes_before_and_after_io_are_unknown_without_reapplication() {
    for operation in ["create", "update", "remove"] {
        for point in ["memory_before_io", "memory_after_io_before_receipt"] {
            let (fx, host, target) = setup();
            let old = (operation != "create").then(|| {
                committed(&request(
                    &fx,
                    &host,
                    "memory_put",
                    "seed",
                    put(None, "seed"),
                ))
            });
            let hash = old.as_ref().and_then(|r| r.result["hash"].as_str());
            let op = if operation == "remove" {
                "memory_remove"
            } else {
                "memory_put"
            };
            let payload = if operation == "remove" {
                json!({"selector":selector(),"expected_hash":hash})
            } else {
                put(hash, "replacement")
            };
            receipts::FAILPOINT.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move |name| {
                    if name == point {
                        panic!("simulated crash");
                    }
                }))
            });
            let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                request(&fx, &host, op, "crash", payload)
            }));
            receipts::FAILPOINT.with(|hook| *hook.borrow_mut() = None);
            assert!(crashed.is_err());
            assert!(
                lookup(
                    &fx,
                    &host,
                    &crate::agent::tool_request_id(&host.identity.session_id, "crash")
                )
                .is_none()
            );
            let actual = read(&fx);
            let expected = match (operation, point) {
                ("create", "memory_before_io") | ("remove", "memory_after_io_before_receipt") => {
                    None
                }
                (_, "memory_before_io") => Some("seed"),
                _ => Some("replacement"),
            };
            assert_eq!(actual.as_ref().map(|r| r.body.as_str()), expected);
            // A later intentional edit/recreation must survive classification and fencing.
            let newer = fx
                .ctx
                .store
                .memory_put(
                    &selector(),
                    entry("intervening"),
                    actual.as_ref().map(|r| r.item.hash.as_str()),
                    "owner",
                    jiff::Timestamp::now(),
                )
                .unwrap();
            bind(&fx, &target, "p1", "resumed");
            let (class, detail) = interrupted::classify_corral(op, None, false);
            assert_eq!(class, ResumeClass::OutcomeUnknown);
            assert!(detail.contains("file effect unknown: re-read the entry"));
            assert_eq!(read(&fx).unwrap().item.hash, newer.hash);
            let fenced_payload = if op == "memory_remove" {
                json!({"selector":selector(), "expected_hash":newer.hash})
            } else {
                put(Some(&newer.hash), "never")
            };
            assert_eq!(
                code(&request(
                    &fx,
                    &host,
                    op,
                    "abandoned-old-generation",
                    fenced_payload
                )),
                Some("agent_fenced")
            );
        }
    }
}
