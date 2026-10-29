//! Fixtures for question tests: a temp store, Pi-native hosts bound through the public agent
//! API, and helpers that drive `corral agent call` and the PM and owner operations.
use std::path::PathBuf;

use corral_core::agent::{AgentQuestion, AgentRole, ContainerRef, RequestIdentity, paths};
use corral_core::inbox::{Inbox, Message, Recipient};
use corral_core::{RunOutcome, Status, Store};
use serde_json::{Value, json};

use super::{Actor, AnswerInput, Applied};
use crate::Ctx;
use crate::agent::{
    BindTarget, CallResponse, call, event_request_id, open_generation, payload_sha256,
    tool_request_id,
};

pub(super) struct Fx {
    pub(super) _tmp: tempfile::TempDir,
    pub(super) ctx: Ctx,
}

pub(super) fn fx() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = Ctx::from_store(Store::init(tmp.path()).unwrap());
    Fx { _tmp: tmp, ctx }
}

pub(super) fn run(id: &str, pane: &str) -> corral_core::Run {
    serde_json::from_value(json!({
        "id": id, "agent": "pi", "worktree": "/tmp/w", "branch": "corral/T", "base": "main",
        "base_sha": "abc", "started": "2026-10-23T00:00:00Z",
        "herdr": {"workspace_id": "w1", "pane_id": pane}
    }))
    .unwrap()
}

#[derive(Clone)]
pub(super) struct Host {
    pub(super) identity: RequestIdentity,
    pub(super) host_cap: String,
}

fn secret(label: &str) -> String {
    paths::sha256_hex(label.as_bytes())
}

pub(super) fn bind(fx: &Fx, target: &BindTarget, pane: &str, session: &str) -> Host {
    bind_in(fx, target, pane, session, "corral")
}

fn bind_in(fx: &Fx, target: &BindTarget, pane: &str, session: &str, project: &str) -> Host {
    let grant = open_generation(&fx.ctx.store, target, pane).unwrap();
    let (role, task, run, review, callback) = match target {
        BindTarget::Planner { task, run } => {
            (AgentRole::Planner, Some(task), Some(run), None, None)
        }
        BindTarget::PlanReviewer { task, plan, review } => (
            AgentRole::PlanReviewer,
            Some(task),
            Some(plan),
            Some(review),
            None,
        ),
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
    let host = Host {
        identity: RequestIdentity {
            store: paths::store_id(&fx.ctx.store),
            project: project.into(),
            role,
            task: task.cloned(),
            run: run.cloned(),
            review: review.cloned(),
            callback: callback.cloned(),
            session_id: session.into(),
            generation: grant.generation,
            turn: None,
        },
        host_cap: secret(&format!("{session}-host")),
    };
    let sessions: PathBuf = paths::container_sessions_dir(&fx.ctx.store, &grant.container);
    let ctl = secret(&format!("{session}-ctl"));
    let payload = json!({
        "ctl_cap_sha256": paths::sha256_hex(ctl.as_bytes()),
        "session_file": sessions.join("session.jsonl"),
        "pid": 4242, "start_time": "123456", "pane": pane,
    });
    let mut input = envelope_value(
        &host,
        "hello",
        &event_request_id(grant.generation, 0),
        payload,
    );
    input["auth"]["bind_nonce"] = json!(grant.bind_nonce);
    let response = call(&fx.ctx, "hello", &serde_json::to_vec(&input).unwrap()).unwrap();
    assert!(response.receipt().is_some(), "hello: {response:?}");
    host
}

fn envelope_value(host: &Host, op: &str, request_id: &str, payload: Value) -> Value {
    json!({
        "version": 1, "op": op, "request_id": request_id,
        "payload_sha256": payload_sha256(&payload), "identity": host.identity,
        "auth": {"host_cap": host.host_cap}, "payload": payload,
    })
}

/// Sends one tool-call request; `call_id` is the host's tool call ID.
pub(super) fn send(fx: &Fx, host: &Host, op: &str, call_id: &str, payload: Value) -> CallResponse {
    let request = tool_request_id(&host.identity.session_id, call_id);
    send_request(fx, host, op, &request, payload).unwrap()
}

/// Sends a request under an explicit request ID, e.g. a replacement generation's retry.
pub(super) fn send_request(
    fx: &Fx,
    host: &Host,
    op: &str,
    request: &str,
    payload: Value,
) -> anyhow::Result<CallResponse> {
    let input = envelope_value(host, op, request, payload);
    call(&fx.ctx, op, &serde_json::to_vec(&input).unwrap())
}

pub(super) fn request_id(host: &Host, call_id: &str) -> String {
    tool_request_id(&host.identity.session_id, call_id)
}

/// Makes every event write fail by putting a directory where the event log lives.
pub(super) fn break_events(fx: &Fx) {
    let log = fx.ctx.store.dir().join("events.jsonl");
    if log.exists() {
        std::fs::rename(&log, log.with_extension("saved")).unwrap();
    }
    std::fs::create_dir(&log).unwrap();
}

pub(super) fn restore_events(fx: &Fx) {
    let log = fx.ctx.store.dir().join("events.jsonl");
    std::fs::remove_dir(&log).unwrap();
    let saved = log.with_extension("saved");
    if saved.exists() {
        std::fs::rename(saved, &log).unwrap();
    }
}

pub(super) fn review_messages(fx: &Fx, task: &str, review: &str) -> Vec<Message> {
    let recipient = Recipient::ReviewRun {
        task: task.into(),
        run: "R1".into(),
        review: review.into(),
    };
    Inbox::new(&fx.ctx.store, recipient)
        .unwrap()
        .messages()
        .unwrap()
}

pub(super) fn code(response: &CallResponse) -> Option<&'static str> {
    response.rejection().map(|r| r.code.as_str())
}

fn task_with_run(fx: &Fx, project: &str, pane: &str) -> String {
    let task = fx
        .ctx
        .store
        .create("Pi worker", None, Status::Running, "")
        .unwrap();
    fx.ctx
        .store
        .update(&task.id, |t| {
            t.project = Some(project.into());
            t.runs.push(run("R1", pane));
            Ok(())
        })
        .unwrap();
    task.id
}

/// A Running task whose run R1 is bound to a healthy Pi-native worker.
pub(super) fn worker(fx: &Fx) -> (String, Host) {
    worker_in(fx, "corral")
}

pub(super) fn worker_in(fx: &Fx, project: &str) -> (String, Host) {
    let id = task_with_run(fx, project, "p1");
    let target = BindTarget::Worker {
        task: id.clone(),
        run: "R1".into(),
    };
    let host = bind_in(fx, &target, "p1", &format!("s-{id}-w"), project);
    (id, host)
}

/// A Review task whose open review child R1-review1 is bound to a Pi-native reviewer.
pub(super) fn reviewer(fx: &Fx) -> (String, Host) {
    let id = task_with_run(fx, "corral", "p1");
    fx.ctx
        .store
        .update(&id, |t| {
            t.status = Status::Review;
            t.runs[0].outcome = Some(RunOutcome::Done);
            t.runs[0].review_runs.push(run("R1-review1", "p2"));
            Ok(())
        })
        .unwrap();
    let host = review_child(fx, &id, "R1-review1", "p2", "g1:c1");
    (id, host)
}

pub(super) fn review_child(fx: &Fx, task: &str, review: &str, pane: &str, callback: &str) -> Host {
    let target = BindTarget::Reviewer {
        task: task.into(),
        run: "R1".into(),
        review: review.into(),
        callback: callback.into(),
    };
    bind(fx, &target, pane, &format!("s-{task}-{review}"))
}

pub(super) fn pm_host(fx: &Fx) -> Host {
    let target = BindTarget::Pm {
        project: "corral".into(),
    };
    bind(fx, &target, "pm-pane", "s-pm")
}

pub(super) fn ask(fx: &Fx, host: &Host, call_id: &str, payload: Value) -> CallResponse {
    send(fx, host, "ask", call_id, payload)
}

/// Asks a clarification question and returns its key.
pub(super) fn ask_ok(fx: &Fx, host: &Host, call_id: &str, blocking: bool) -> String {
    ask_kind(fx, host, call_id, "clarification", blocking)
}

pub(super) fn ask_kind(fx: &Fx, host: &Host, call_id: &str, kind: &str, blocking: bool) -> String {
    let payload =
        json!({"kind": kind, "blocking": blocking, "text": format!("question {call_id}?")});
    let response = ask(fx, host, call_id, payload);
    let receipt = response
        .receipt()
        .unwrap_or_else(|| panic!("ask: {response:?}"));
    receipt.result["key"].as_str().unwrap().to_string()
}

pub(super) fn question(fx: &Fx, key: &str) -> AgentQuestion {
    super::show(&fx.ctx, key).unwrap().question
}

pub(super) fn pending(fx: &Fx, key: &str) -> bool {
    super::show(&fx.ctx, key).unwrap().pending
}

pub(super) fn events(fx: &Fx, kind: &str) -> usize {
    let events = fx.ctx.store.events().unwrap();
    events.iter().filter(|e| e.kind == kind).count()
}

pub(super) fn spec_ref(fx: &Fx, task: &str) -> String {
    let task = fx.ctx.store.get(task).unwrap();
    format!(
        "spec:{}@{}",
        task.id,
        crate::progress_spec_revision(&task).unwrap()
    )
}

pub(super) fn pm_input(fx: &Fx, task: &str, text: &str) -> AnswerInput {
    AnswerInput {
        text: text.into(),
        option: None,
        basis_refs: vec![spec_ref(fx, task)],
        scope_note: Some("inside the approved write scope".into()),
    }
}

/// The PM answers on the CLI with a resolvable spec basis and a scope note.
pub(super) fn pm_answer(fx: &Fx, key: &str, revision: u32) -> anyhow::Result<Applied> {
    let task = key.split('/').next().unwrap();
    super::answer(
        &fx.ctx,
        key,
        revision,
        Actor::Pm(None),
        pm_input(fx, task, "use the default"),
    )
}

pub(super) fn refusal(error: &anyhow::Error) -> &'static str {
    error
        .downcast_ref::<super::QuestionRefusal>()
        .map(|r| r.kind.code())
        .unwrap_or_else(|| panic!("not a question refusal: {error:#}"))
}

pub(super) fn messages(fx: &Fx, task: &str, run: &str) -> Vec<Message> {
    let recipient = Recipient::Run {
        task: task.into(),
        run: run.into(),
    };
    Inbox::new(&fx.ctx.store, recipient)
        .unwrap()
        .messages()
        .unwrap()
}

pub(super) fn worker_container(task: &str) -> ContainerRef {
    ContainerRef::Worker {
        task: task.into(),
        run: "R1".into(),
    }
}
