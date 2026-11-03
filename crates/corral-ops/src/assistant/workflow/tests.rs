use corral_core::assistant::{AssistantContainer, NewOwnerInput, OwnerInputSource};
use corral_core::assistant_workflow::*;
use corral_core::blob_ref::BlobHash;
use serde_json::{Value, json};

use super::*;
use crate::agent::test_support::{self as ts, Fx, Host};
use crate::agent::tool_request_id;
use crate::agent::{AgentOp, BindTarget, CallResponse, FAILPOINT, fence_generation};

struct Fixture {
    fx: Fx,
    host: Host,
    session: String,
}

impl Fixture {
    fn new() -> Self {
        let mut fx = ts::fx();
        fx.ctx.store.config.projects.insert(
            "app".into(),
            serde_json::from_value(json!({"path": "."})).unwrap(),
        );
        let session = fx
            .ctx
            .store
            .create_assistant("pi", None, Some("a-pane"), None)
            .unwrap()
            .id;
        let host = ts::bind(
            &fx,
            &BindTarget::Assistant {
                session: session.clone(),
            },
            "a-pane",
            "workflow-test",
        );
        let hello = ts::committed(&ts::hello(&fx, &host));
        for op in [
            "assistant_plan",
            "assistant_plan_review",
            "assistant_handoff",
        ] {
            assert!(hello.result["ops"].as_array().unwrap().contains(&json!(op)));
        }
        Self { fx, host, session }
    }
    fn container(&self) -> AssistantContainer {
        self.fx
            .ctx
            .store
            .get_assistant_container(&self.session)
            .unwrap()
    }
    fn send(&self, request: &str, op: &str, payload: Value) -> CallResponse {
        let request = tool_request_id(&self.host.identity.session_id, request);
        ts::send(
            &self.fx,
            &ts::envelope(&self.host, op, &request, payload, false),
            op,
        )
    }
    /// Simulate the future authenticated host/drain seam. No public action enters planning
    /// in this foundation, and tests never claim this fixture proves the execution barrier.
    fn planning(&self) {
        self.fx
            .ctx
            .store
            .update_assistant_container(&self.session, |container| {
                let text = "Plan cache invalidation; do not implement yet.\n";
                let input = container.journal_owner_input(
                    &self.session,
                    NewOwnerInput {
                        generation: self.host.identity.generation,
                        turn: None,
                        source: OwnerInputSource::Pane,
                        sha256: BlobHash::of(text.as_bytes()).to_string(),
                        text: text.into(),
                        truncated: false,
                        command: None,
                        answers: None,
                        request: "ev-1-1".into(),
                    },
                );
                prepare_entry(&self.fx.ctx.store, &self.session, container, &input.id)?;
                let w = &mut container.workflow.as_mut().unwrap().current;
                w.phase = WorkflowPhase::Planning;
                w.state_revision += 1;
                Ok(())
            })
            .unwrap();
    }
    fn expected(&self) -> Value {
        let c = self.container();
        let w = &c.workflow.as_ref().unwrap().current;
        json!({"workflow": w.id, "state_revision": w.state_revision,
            "authority_epoch": w.authority_epoch, "plan_revision": w.current_plan.as_ref().map(|p| p.revision)})
    }
    fn stage(&self, request: &str, expected: &Value, offset: usize, chunk: &str) -> CallResponse {
        self.send(
            request,
            "assistant_plan",
            json!({"action": "stage", "expected": expected,
            "offset": offset, "chunk": chunk}),
        )
    }
    fn publication(&self, expected: &Value, text: &str) -> Value {
        json!({"action": "publish", "expected": expected, "bytes": text.len(),
            "digest": BlobHash::of(text.as_bytes()), "stage": "draft"})
    }
    fn no_tasks(&self) {
        assert!(self.fx.ctx.store.list().unwrap().is_empty());
    }
}

#[test]
fn assistant_workflow_legacy_current_and_future_records_round_trip_without_tasks() {
    let f = Fixture::new();
    let legacy = f.container();
    let mut json = serde_json::to_value(&legacy).unwrap();
    assert!(json.get("workflow").is_none());
    assert_eq!(
        serde_json::from_value::<AssistantContainer>(json.clone()).unwrap(),
        legacy
    );
    f.planning();
    let current = f.container();
    assert_eq!(current.version, CONTAINER_VERSION);
    assert_eq!(
        serde_json::from_value::<AssistantContainer>(serde_json::to_value(&current).unwrap())
            .unwrap(),
        current
    );
    // A v1-only reader's exact guard cannot accept a live fence.
    assert_ne!(current.version, corral_core::assistant::ASSISTANT_VERSION);
    let mut bad = current.clone();
    bad.version = 1;
    assert!(bad.validate_protocol().is_err());
    bad = current.clone();
    bad.workflow.as_mut().unwrap().version += 1;
    assert!(bad.validate_protocol().is_err());
    f.fx.ctx
        .store
        .update_assistant_container(&f.session, |container| {
            container.owner_inputs.clear();
            Ok(())
        })
        .unwrap();
    let retained = f.container();
    assert_eq!(
        retained.workflow, current.workflow,
        "provenance survives journal eviction"
    );
    assert_eq!(
        read_revision(&f.fx.ctx.store, &f.session, &retained, 0, 0, 1024).unwrap()["text"],
        current
            .workflow
            .as_ref()
            .unwrap()
            .current
            .owner_evidence
            .text
    );
    json["version"] = json!(99);
    f.fx.ctx
        .store
        .save_record("assistant-containers", &f.session, &json)
        .unwrap();
    assert!(f.fx.ctx.store.get_assistant_container(&f.session).is_err());
    f.no_tasks();
}

#[test]
fn assistant_workflow_exact_chunks_publication_replay_and_stale_cas_have_no_extra_effects() {
    let f = Fixture::new();
    f.planning();
    let expected = f.expected();
    let mut old_epoch = expected.clone();
    old_epoch["authority_epoch"] = json!(0);
    let before = f.container();
    assert_eq!(
        ts::code(&f.stage("old-epoch", &old_epoch, 0, "stale")),
        Some("stale_revision")
    );
    assert_eq!(f.container(), before);
    let text = "# Draft\r\nUnknowns: café 🦀\n\0end\n";
    let split = text.find("🦀").unwrap();
    ts::committed(&f.stage("s1", &expected, 0, &text[..split]));
    let first = f.stage("s1", &expected, 0, &text[..split]);
    assert!(matches!(
        first,
        CallResponse::Receipt { replayed: true, .. }
    ));
    ts::committed(&f.stage("s1-fresh", &expected, 0, &text[..split]));
    assert_eq!(
        ts::code(&f.stage("bad-offset", &expected, split + 1, "no")),
        Some("bad_request")
    );
    assert_eq!(
        ts::code(&f.stage("bad-retry", &expected, 0, "changed")),
        Some("payload_mismatch")
    );
    ts::committed(&f.stage("s2", &expected, split, &text[split..]));
    let mut wrong = f.publication(&expected, text);
    wrong["digest"] = json!(BlobHash::of(b"wrong"));
    assert_eq!(
        ts::code(&f.send("bad-digest", "assistant_plan", wrong)),
        Some("bad_request")
    );
    let receipt =
        ts::committed(&f.send("publish", "assistant_plan", f.publication(&expected, text)));
    assert!(
        serde_json::to_vec(&receipt.result).unwrap().len() <= corral_core::agent::RESULT_MAX_BYTES
    );
    let published = f.container();
    let replay = f.send("publish", "assistant_plan", f.publication(&expected, text));
    assert_eq!(replay.receipt(), Some(&receipt));
    assert!(matches!(
        replay,
        CallResponse::Receipt { replayed: true, .. }
    ));
    let stale = f.send(
        "publish-fresh",
        "assistant_plan",
        f.publication(&expected, text),
    );
    assert_eq!(ts::code(&stale), Some("stale_revision"));
    assert_eq!(
        stale.rejection().unwrap().detail.as_ref().unwrap()["current_plan"]["revision"],
        1
    );
    assert_eq!(
        f.container(),
        published,
        "stale CAS stores no receipt or workflow change"
    );
    let read = ts::committed(&f.send(
        "read-r1",
        "assistant_plan",
        json!({"action": "read_revision", "revision": 1}),
    ));
    assert_eq!(read.result["text"], text);
    assert_eq!(f.container(), published, "reads store no receipts");
    // A new revision never rewrites r1, and r1 remains byte-exact after reopen.
    let expected2 = f.expected();
    ts::committed(&f.stage("s3", &expected2, 0, "revised\n"));
    let mut ready = f.publication(&expected2, "revised\n");
    ready["stage"] = json!("ready");
    assert_eq!(
        ts::code(&f.send("ready-without-scope", "assistant_plan", ready.clone())),
        Some("bad_request")
    );
    ready["execution_scope"] =
        json!({"project": "app", "paths": ["src/cache.rs"], "actions": ["change invalidation"]});
    ready["assumptions"] = json!({"project": "app", "source_head": "1".repeat(40), "base_head": "2".repeat(40),
        "files": [{"path": "src/cache.rs", "sha256": null}]});
    let mut unsafe_scope = ready.clone();
    unsafe_scope["execution_scope"]["paths"] = json!(["../outside"]);
    assert_eq!(
        ts::code(&f.send("unsafe-scope", "assistant_plan", unsafe_scope)),
        Some("bad_request")
    );
    ts::committed(&f.send("publish2", "assistant_plan", ready));
    assert_eq!(
        f.container().workflow.as_ref().unwrap().current.phase,
        WorkflowPhase::Planning
    );
    let reopened = corral_core::Store::open(f.fx._tmp.path()).unwrap();
    let c = reopened.get_assistant_container(&f.session).unwrap();
    assert_eq!(
        read_revision(&reopened, &f.session, &c, 1, 0, 1024).unwrap()["text"],
        text
    );
    assert_eq!(
        read_state(&reopened, &f.session, &c, 0, 1).unwrap()["next_after"],
        1
    );
    f.no_tasks();
}

#[test]
fn assistant_workflow_interrupted_artifact_preparation_recovers_without_duplicate_revision() {
    let f = Fixture::new();
    f.planning();
    let expected = f.expected();
    let text = "an unfinished design\n";
    ts::committed(&f.stage("stage", &expected, 0, text));
    let before = f.container();
    let payload = f.publication(&expected, text);
    for barrier in ["workflow_artifact_prepared", "workflow_revision_staged"] {
        FAILPOINT.with_borrow_mut(|hook| {
            *hook = Some(Box::new(move |point| {
                if point == barrier {
                    panic!("crash at {barrier}, before state/receipt commit");
                }
            }))
        });
        let crash = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            f.send("publish", "assistant_plan", payload.clone())
        }));
        FAILPOINT.with_borrow_mut(|hook| *hook = None);
        assert!(crash.is_err());
        assert_eq!(f.container(), before);
        assert!(
            f.fx.ctx
                .store
                .record::<PlanRevision>(REVISIONS, &revision_key(&f.session, "W1", 1))
                .unwrap()
                .is_none()
        );
    }
    let receipt = ts::committed(&f.send("publish", "assistant_plan", payload.clone()));
    assert_eq!(receipt.result["current_plan"]["revision"], 1);
    assert_eq!(
        f.send("publish", "assistant_plan", payload).receipt(),
        Some(&receipt)
    );
    assert_eq!(f.fx.ctx.store.record_rows(REVISIONS).unwrap().len(), 1);
    assert_eq!(
        f.fx.ctx
            .store
            .record_rows("assistant-design-artifacts")
            .unwrap()
            .len(),
        2
    );
    f.no_tasks();
}

#[test]
fn assistant_workflow_pages_large_utf8_and_history_with_bounded_receipts() {
    let f = Fixture::new();
    f.planning();
    let text = "🦀".repeat(4096);
    for revision in 1..=4 {
        let expected = f.expected();
        ts::committed(&f.stage(&format!("stage-{revision}"), &expected, 0, &text));
        let mut payload = f.publication(&expected, &text);
        payload["scope_changes"] = json!("x".repeat(2048));
        ts::committed(&f.send(&format!("publish-{revision}"), "assistant_plan", payload));
    }
    let before = f.container();
    let state = ts::committed(&f.send(
        "state",
        "assistant_plan",
        json!({"action": "read", "limit": 8}),
    ));
    assert!(
        serde_json::to_vec(&state.result).unwrap().len() <= corral_core::agent::RESULT_MAX_BYTES
    );
    assert!(state.result["next_after"].is_number());
    let mut offset = 0;
    let mut restored = String::new();
    loop {
        let page = ts::committed(&f.send(
            &format!("read-{offset}"),
            "assistant_plan",
            json!({"action": "read_revision", "revision": 1, "offset": offset, "limit": 1023}),
        ));
        assert!(
            serde_json::to_vec(&page.result).unwrap().len() <= corral_core::agent::RESULT_MAX_BYTES
        );
        restored.push_str(page.result["text"].as_str().unwrap());
        match page.result["next_offset"].as_u64() {
            Some(next) => offset = next,
            None => break,
        }
    }
    assert_eq!(restored, text);
    assert_eq!(f.container(), before);
    f.no_tasks();
}

#[test]
fn assistant_workflow_closed_schemas_role_generation_and_lifecycle_fail_closed() {
    let f = Fixture::new();
    let before = f.container();
    for (op, payload) in [
        (
            "assistant_plan",
            json!({"action": "enter", "input": "ev-1-1"}),
        ),
        ("assistant_plan_review", json!({"action": "read"})),
        ("assistant_handoff", json!({"action": "read"})),
    ] {
        assert_eq!(ts::code(&f.send(op, op, payload)), Some("op_unavailable"));
    }
    assert_eq!(f.container(), before);
    assert_eq!(
        ts::code(&f.send("unknown", "assistant_plan", json!({"action": "drained"}))),
        Some("bad_request")
    );
    assert_eq!(
        ts::code(&f.send(
            "selectors",
            "assistant_plan",
            json!({"action": "read", "session": "A-2"})
        )),
        Some("bad_request")
    );
    for op in [
        AgentOp::AssistantPlan,
        AgentOp::AssistantPlanReview,
        AgentOp::AssistantHandoff,
    ] {
        assert_eq!(op.roles(), &[corral_core::agent::AgentRole::Assistant]);
    }
    let worker_id = ts::worker_task(&f.fx, "worker");
    let worker = ts::bind(
        &f.fx,
        &BindTarget::Worker {
            task: worker_id,
            run: "R1".into(),
        },
        "worker",
        "worker-session",
    );
    ts::committed(&ts::hello(&f.fx, &worker));
    let input = ts::envelope(
        &worker,
        "assistant_plan",
        &tool_request_id("worker-session", "read"),
        json!({"action": "read"}),
        false,
    );
    assert_eq!(
        ts::code(&ts::send(&f.fx, &input, "assistant_plan")),
        Some("op_not_allowed")
    );
    fence_generation(
        &f.fx.ctx.store,
        &BindTarget::Assistant {
            session: f.session.clone(),
        }
        .container(),
        1,
        "test fence",
    )
    .unwrap();
    assert_eq!(
        ts::code(&f.send("fenced", "assistant_plan", json!({"action": "read"}))),
        Some("agent_fenced")
    );
}
