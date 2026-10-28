//! Shared fixtures for the agent request-path tests: a temp store, bound hosts and envelopes.
use std::path::PathBuf;

use corral_core::agent::{
    AgentRole, AgentRunState, ContainerRef, ReceiptState, RequestIdentity, RequestReceipt, paths,
    pm_state,
};
use corral_core::{RunOutcome, Status, Store};
use serde_json::{Value, json};

use super::*;
use crate::Ctx;

thread_local! {
    static DATA_ROOT: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

pub(crate) fn data_root_override() -> Option<PathBuf> {
    DATA_ROOT.with_borrow(Clone::clone)
}

/// A private managed-install root for this test thread, restored even on panic.
/// Unlike changing HOME/XDG_DATA_HOME, this cannot affect concurrently running tests.
pub(crate) struct PrivateDataRoot {
    _tmp: tempfile::TempDir,
    previous: Option<PathBuf>,
    _thread: std::marker::PhantomData<std::rc::Rc<()>>,
}

pub(crate) fn private_data_root() -> PrivateDataRoot {
    let tmp = tempfile::tempdir().unwrap();
    let previous = DATA_ROOT.replace(Some(tmp.path().to_path_buf()));
    PrivateDataRoot {
        _tmp: tmp,
        previous,
        _thread: std::marker::PhantomData,
    }
}

impl Drop for PrivateDataRoot {
    fn drop(&mut self) {
        DATA_ROOT.set(self.previous.take());
    }
}

#[test]
fn private_data_roots_are_thread_local_and_restore_after_unwinding() {
    assert!(data_root_override().is_none());
    let _outer = private_data_root();
    let root = super::install::data_root().unwrap();
    assert!(root.is_dir());
    std::thread::spawn(|| assert!(data_root_override().is_none()))
        .join()
        .unwrap();
    let panic = std::panic::catch_unwind(|| {
        let _inner = private_data_root();
        assert_ne!(super::install::data_root().unwrap(), root);
        panic!("exercise cleanup");
    });
    assert!(panic.is_err());
    assert_eq!(super::install::data_root().unwrap(), root);
    drop(_outer);
    assert!(data_root_override().is_none());
    assert!(!root.exists());
}

pub(crate) struct Fx {
    pub(crate) _tmp: tempfile::TempDir,
    pub(crate) ctx: Ctx,
}

pub(crate) fn fx() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = Ctx::from_store(Store::init(tmp.path()).unwrap());
    Fx { _tmp: tmp, ctx }
}

pub(crate) fn run(id: &str, pane: &str) -> corral_core::Run {
    serde_json::from_value(json!({
        "id": id, "agent": "pi", "worktree": "/tmp/w", "branch": "corral/T-001", "base": "main",
        "base_sha": "abc", "started": "2026-10-23T00:00:00Z",
        "herdr": {"workspace_id": "w1", "pane_id": pane}
    }))
    .unwrap()
}

pub(crate) fn worker_task(fx: &Fx, pane: &str) -> String {
    let task = fx
        .ctx
        .store
        .create("Pi worker", None, Status::Running, "")
        .unwrap();
    fx.ctx
        .store
        .update(&task.id, |t| {
            t.project = Some("corral".into());
            t.runs.push(run("R1", pane));
            Ok(())
        })
        .unwrap();
    task.id
}

pub(crate) fn reviewer_task(fx: &Fx) -> (String, BindTarget) {
    let id = worker_task(fx, "p1");
    fx.ctx
        .store
        .update(&id, |t| {
            t.status = Status::Review;
            t.runs[0].outcome = Some(RunOutcome::Done);
            t.runs[0].review_runs.push(run("R1-review1", "p2"));
            Ok(())
        })
        .unwrap();
    let target = BindTarget::Reviewer {
        task: id.clone(),
        run: "R1".into(),
        review: "R1-review1".into(),
        callback: "g1:c1".into(),
    };
    (id, target)
}

#[derive(Clone)]
pub(crate) struct Host {
    pub(crate) identity: RequestIdentity,
    pub(crate) host_cap: String,
    pub(crate) ctl_cap: String,
    pub(crate) nonce: String,
    pub(crate) pane: String,
    pub(crate) sessions: PathBuf,
}

pub(crate) fn secret(label: &str, generation: u32) -> String {
    paths::sha256_hex(format!("{label}/{generation}").as_bytes())
}

pub(crate) fn bind(fx: &Fx, target: &BindTarget, pane: &str, session: &str) -> Host {
    let grant = open_generation(&fx.ctx.store, target, pane).unwrap();
    let (role, task, run, review, callback) = match target {
        BindTarget::Worker { task, run } => (AgentRole::Worker, Some(task), Some(run), None, None),
        BindTarget::Reviewer {
            task,
            run,
            review,
            callback,
        } => (
            AgentRole::Reviewer,
            Some(task),
            Some(run),
            Some(review),
            Some(callback),
        ),
        BindTarget::Pm { .. } => (AgentRole::Pm, None, None, None, None),
    };
    Host {
        identity: RequestIdentity {
            store: paths::store_id(&fx.ctx.store),
            project: "corral".into(),
            role,
            task: task.cloned(),
            run: run.cloned(),
            review: review.cloned(),
            callback: callback.cloned(),
            session_id: session.into(),
            generation: grant.generation,
            turn: None,
        },
        host_cap: secret(&format!("{session}-host"), grant.generation),
        ctl_cap: secret(&format!("{session}-ctl"), grant.generation),
        nonce: grant.bind_nonce,
        pane: pane.into(),
        sessions: paths::container_sessions_dir(&fx.ctx.store, &grant.container),
    }
}

pub(crate) fn envelope(
    host: &Host,
    op: &str,
    request_id: &str,
    payload: Value,
    nonce: bool,
) -> Vec<u8> {
    let mut auth = json!({"host_cap": host.host_cap});
    if nonce {
        auth["bind_nonce"] = json!(host.nonce);
    }
    serde_json::to_vec(&json!({
        "version": 1, "op": op, "request_id": request_id,
        "payload_sha256": payload_sha256(&payload), "identity": host.identity,
        "auth": auth, "payload": payload,
    }))
    .unwrap()
}

pub(crate) fn hello_input(host: &Host, request: &str) -> Vec<u8> {
    let payload = json!({
        "ctl_cap_sha256": paths::sha256_hex(host.ctl_cap.as_bytes()),
        "session_file": host.sessions.join("session.jsonl"),
        "pid": 4242, "start_time": "123456", "pane": host.pane,
    });
    envelope(host, "hello", request, payload, true)
}

pub(crate) fn hello(fx: &Fx, host: &Host) -> CallResponse {
    let request = event_request_id(host.identity.generation, 0);
    call(&fx.ctx, "hello", &hello_input(host, &request)).unwrap()
}

pub(crate) fn event_input(host: &Host, seq: u64, kind: &str, turn: u64) -> Vec<u8> {
    let request = event_request_id(host.identity.generation, seq);
    envelope(
        host,
        "event",
        &request,
        json!({"kind": kind, "turn": turn}),
        false,
    )
}

pub(crate) fn send(fx: &Fx, input: &[u8], op: &str) -> CallResponse {
    call(&fx.ctx, op, input).unwrap()
}

pub(crate) fn lookup(fx: &Fx, host: &Host, request: &str) -> Option<RequestReceipt> {
    let id = tool_request_id(&host.identity.session_id, request);
    let input = envelope(
        host,
        "receipt_lookup",
        &id,
        json!({"request_id": request}),
        false,
    );
    match send(fx, &input, "receipt_lookup") {
        CallResponse::Lookup { found, .. } => found.map(|receipt| *receipt),
        other => panic!("expected a lookup answer, got {other:?}"),
    }
}

pub(crate) fn code(response: &CallResponse) -> Option<&'static str> {
    response.rejection().map(|r| r.code.as_str())
}

pub(crate) fn committed(response: &CallResponse) -> RequestReceipt {
    match response {
        CallResponse::Receipt {
            receipt,
            replayed: false,
        } if receipt.state == ReceiptState::Committed => receipt.clone(),
        other => panic!("expected a fresh committed receipt, got {other:?}"),
    }
}

pub(crate) fn state(fx: &Fx, container: &ContainerRef) -> AgentRunState {
    match container.task() {
        Some(task) => {
            let task = fx.ctx.store.get(task).unwrap();
            corral_core::agent::container_state(&task, container)
                .unwrap()
                .clone()
        }
        None => pm_state::load(&fx.ctx.store).unwrap().unwrap().state,
    }
}

pub(crate) fn task_bytes(fx: &Fx, id: &str) -> Vec<u8> {
    std::fs::read(fx.ctx.store.get(id).unwrap().path).unwrap()
}

pub(crate) fn events(fx: &Fx, kind: &str) -> usize {
    let events = fx.ctx.store.events().unwrap();
    events.iter().filter(|e| e.kind == kind).count()
}

/// A receipt of `host`'s Pi session call `call`, for the recovery tests.
#[cfg(target_os = "linux")]
pub(crate) fn receipt(
    host: &Host,
    call: &str,
    op: &str,
    state: &str,
    result: Value,
) -> corral_core::agent::RequestReceipt {
    serde_json::from_value(json!({
        "version": 1, "request_id": tool_request_id(super::bridge::fixture::SESSION, call),
        "op": op, "payload_sha256": "00", "identity": host.identity, "state": state,
        "result": result, "at": "2026-10-25T00:00:00Z",
    }))
    .unwrap()
}
