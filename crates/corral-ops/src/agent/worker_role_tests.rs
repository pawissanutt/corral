//! T-252 worker-role acceptance: the worker ops through `corral agent call`, the gate wake across
//! a corral restart, refused and uncertain gates, done with a pending question, causal turn
//! classification with the OD11 nudge, owner-typed turns, the run prompt, waits, the vendor
//! allowlist and the shipped skill. Each test drives a bound worker host through envelopes.
use std::os::unix::fs::PermissionsExt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::process::Stdio;

use corral_core::agent::{
    self as core_agent, AgentRole, ContainerRef, ReceiptState, TurnClass, WakeState,
};
use corral_core::inbox::{Inbox, Message, Recipient};
use corral_core::{GateResult, RunOutcome, Status, Store, Task, WaitState};
use corral_herdr::Herdr;
use jiff::{SignedDuration, Timestamp};
use serde_json::{Value, json};

use crate::Ctx;
use crate::agent::test_support::{self as ts, Host, committed, envelope, hello, send};
use crate::agent::{AgentOp, BindTarget, CallResponse, event_request_id, tool_request_id};

const FAKE: &str = r#"#!/usr/bin/env bash
cd "$(dirname "$0")" || exit 1
printf '%s\n' "$*" >> calls.log
if [ "${1:-}" = "--session" ]; then shift 2; fi
ok() { printf '{"id":"fake","result":%s}\n' "$1"; }
case "$1 $2" in
  "workspace create"|"tab create")
    n=1
    while [ -f "created-$n" ]; do n=$((n+1)); done
    touch "created-$n" "w1:p$n"
    ok "{\"workspace\":{\"workspace_id\":\"w1\"},\"root_pane\":{\"workspace_id\":\"w1\",\"tab_id\":\"w1:t$n\",\"pane_id\":\"w1:p$n\"}}" ;;
  "workspace list") ok '{"workspaces":[]}' ;;
  "pane list") ok '{"panes":[{"pane_id":"w0:p1"}]}' ;;
  "pane get")
    if [ -f "$3" ]; then ok "{\"pane\":{\"pane_id\":\"$3\",\"workspace_id\":\"w1\",\"tab_id\":\"w1:t1\"}}"; else ok '{}'; fi ;;
  "pane close") if [ -f fail-close ]; then printf '{"error":{"code":"failed","message":"close failed"}}\n'; exit 1; fi; rm -f "$3"; ok '{}' ;;
  "pane read")
    if [ -f fail-read ]; then printf '{"error":{"code":"failed","message":"fixture transport failure"}}\n'; exit 1; fi
    if [ -f "$3" ]; then ok '{"text":"reviewer screen"}'; else printf '{"error":{"code":"pane_not_found","message":"gone"}}\n'; exit 1; fi ;;
  *) ok '{}' ;;
esac
"#;
const SUMMARY: &str = "Implemented it.\nVerified: fixture\nNot verified: none\n";

fn git(dir: &Path, args: &[&str]) -> String {
    let out = crate::test_support::command("git", dir)
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success(), "{args:?}: {out:?}");
    String::from_utf8_lossy(&out.stdout).trim().into()
}

fn ctx_at(root: &Path) -> Ctx {
    Ctx {
        resource_probe: crate::resources::FixedResourceProbe::arc(),
        store: Store::discover(root).unwrap(),
        herdr: Herdr::with_bin(root.join("herdr"), None),
    }
}

/// A Running task in project `corral` whose R1 is bound to a pi-native worker host that said hello.
struct Worker {
    fx: ts::Fx,
    id: String,
    host: Host,
    seq: u64,
}

fn worker(gate: bool) -> Worker {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let app = root.join("app");
    std::fs::create_dir(&app).unwrap();
    git(&app, &["init", "-qb", "main"]);
    for (key, value) in [("user.name", "t"), ("user.email", "t@example.invalid")] {
        git(&app, &["config", key, value]);
    }
    std::fs::write(app.join("tracked"), "initial\n").unwrap();
    git(&app, &["add", "."]);
    git(&app, &["commit", "-qm", "initial"]);
    let bin = root.join("herdr");
    std::fs::write(&bin, FAKE).unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(root.join("w0:p1"), "").unwrap();
    let mut store = Store::init(&root).unwrap();
    store.config.limits.review = 4;
    store.config.limits.count_reviews = false;
    store.config.limits.count_gates = false;
    store.config.dispatch.paused = true;
    if gate {
        store.config.review.gate = Some(vec!["true".into()]);
    }
    let project = serde_json::from_value(json!({"path": "app"})).unwrap();
    store.config.projects.insert("corral".into(), project);
    store.save_config().unwrap();
    let ctx = ctx_at(&root);
    let task = ctx
        .store
        .create("Pi worker", Some("corral"), Status::Running, "spec")
        .unwrap();
    let herdr = Herdr::with_bin(bin, None);
    let run: corral_core::Run = serde_json::from_value(json!({
        "id": "R1", "agent": "pi", "harness": "pi-native", "worktree": app, "branch": "main",
        "base": "main", "base_sha": git(&app, &["rev-parse", "HEAD"]),
        "started": "2026-01-01T00:00:00Z",
        "herdr": {"workspace_id": "w0", "pane_id": "w0:p1", "socket": herdr.socket_path().unwrap()},
    }))
    .unwrap();
    ctx.store
        .update(&task.id, |t| {
            t.runs.push(run);
            Ok(())
        })
        .unwrap();
    let fx = ts::Fx { _tmp: tmp, ctx };
    let target = BindTarget::Worker {
        task: task.id.clone(),
        run: "R1".into(),
    };
    let host = ts::bind(&fx, &target, "w0:p1", "s1");
    committed(&hello(&fx, &host));
    Worker {
        fx,
        id: task.id,
        host,
        seq: 0,
    }
}

impl Worker {
    fn ctx(&self) -> &Ctx {
        &self.fx.ctx
    }

    fn task(&self) -> Task {
        self.ctx().store.get(&self.id).unwrap()
    }

    fn container(&self) -> ContainerRef {
        ContainerRef::Worker {
            task: self.id.clone(),
            run: "R1".into(),
        }
    }

    fn state(&self) -> core_agent::AgentRunState {
        ts::state(&self.fx, &self.container())
    }

    /// One corral tool call, made inside host turn `turn` when given.
    fn op(&self, turn: Option<u64>, call: &str, op: &str, payload: Value) -> CallResponse {
        let mut host = self.host.clone();
        host.identity.turn = turn;
        let id = tool_request_id("s1", call);
        send(&self.fx, &envelope(&host, op, &id, payload, false), op)
    }

    fn event(&mut self, kind: &str, turn: u64, origin: Option<Value>) {
        self.seq += 1;
        let mut payload = json!({"kind": kind, "turn": turn});
        if let Some(origin) = origin {
            payload["origin"] = origin;
        }
        let id = event_request_id(self.host.identity.generation, self.seq);
        committed(&send(
            &self.fx,
            &envelope(&self.host, "event", &id, payload, false),
            "event",
        ));
    }

    /// A whole host turn: started with `origin`, then ended.
    fn turn(&mut self, turn: u64, origin: Value) {
        self.event("turn_started", turn, Some(origin));
        self.event("turn_ended", turn, None);
    }

    fn sync(&self, now: Timestamp) -> crate::agent::hooks::sync::TaskSync {
        crate::agent::hooks::sync::task(self.ctx(), &self.task(), now).unwrap()
    }

    fn class(&self, turn: u64) -> Option<TurnClass> {
        let state = self.state();
        state
            .turns
            .iter()
            .find(|t| t.seq == turn)
            .and_then(|t| t.class)
    }

    fn messages(&self, kind: &str) -> Vec<(Message, String)> {
        let recipient = Recipient::Run {
            task: self.id.clone(),
            run: "R1".into(),
        };
        let inbox = Inbox::new(&self.ctx().store, recipient).unwrap();
        let messages = inbox.messages().unwrap();
        let of_kind = messages.into_iter().filter(|m| m.kind == kind);
        of_kind
            .map(|m| {
                let text = std::fs::read_to_string(inbox.message_path(m.seq)).unwrap();
                (m, text)
            })
            .collect()
    }

    fn calls_of(&self, prefix: &str) -> usize {
        let log = self.ctx().store.root().join("calls.log");
        let log = std::fs::read_to_string(log).unwrap_or_default();
        log.lines().filter(|l| l.starts_with(prefix)).count()
    }

    fn wake(&self, request: &str) -> WakeState {
        let state = self.state();
        let found = state.wakes.iter().find(|w| w.request_id == request);
        found.unwrap().state
    }
}

fn bridge(key: &str) -> Value {
    json!({"kind": "bridge", "idempotency_key": key})
}

fn rejection(response: &CallResponse) -> (&'static str, String) {
    let rejection = response.rejection().expect("a rejected answer");
    (rejection.code.as_str(), rejection.reason.clone())
}

/// Runs `op` with a failpoint hook installed for this thread.
fn with_failpoint<T>(hook: impl FnMut(&str) + 'static, op: impl FnOnce() -> T) -> T {
    crate::agent::receipts::FAILPOINT.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
    let out = op();
    crate::agent::receipts::FAILPOINT.with(|slot| *slot.borrow_mut() = None);
    out
}

#[test]
fn hello_advertises_every_worker_op_and_the_shipped_skill_is_the_managed_corral_worker_skill() {
    let ops = AgentOp::implemented_for(AgentRole::Worker);
    for op in [
        "ask",
        "withdraw",
        "wait",
        "progress",
        "checkpoint",
        "evidence",
        "artifact",
        "gate",
        "done",
    ] {
        assert!(ops.contains(&op), "{op} missing from {ops:?}");
    }
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let skill = "packages/corral-pi/src/roles/worker/skills/corral-worker/SKILL.md";
    let shipped = std::fs::read_to_string(repo.join(skill)).unwrap();
    let unversioned = |text: &str| -> String {
        let line = |l: &str| {
            let words = l.split(' ').filter(|w| !w.starts_with("version="));
            words.collect::<Vec<_>>().join(" ")
        };
        text.lines().map(line).collect::<Vec<_>>().join("\n")
    };
    let managed = crate::role_skill_markdown(crate::WORKER_SKILL).unwrap();
    assert_eq!(
        unversioned(&shipped),
        unversioned(&managed),
        "regenerate {skill} from the corral-worker skill"
    );
}

#[test]
fn corral_gate_wakes_once_across_a_restart_and_done_moves_the_task_to_review() {
    // Given a bound worker whose corral_gate started:
    let w = worker(true);
    let started = w.op(None, "gate-1", "gate", json!({}));
    let receipt = committed(&started);
    let attempt = crate::agent::effects::own_attempt(&receipt.request_id);
    assert_eq!(receipt.result["outcome"], "started");
    assert_eq!(receipt.result["attempt"], attempt.as_str());
    assert_eq!(w.wake(&receipt.request_id), WakeState::Armed);
    assert!(w.messages("wait_wake").is_empty());

    // When the gate passes and a fresh corral process syncs twice:
    w.ctx()
        .store
        .update(&w.id, |t| {
            t.runs[0].gate.as_mut().unwrap().result = GateResult::Pass;
            Ok(())
        })
        .unwrap();
    let fresh = ctx_at(w.ctx().store.root());
    for _ in 0..2 {
        crate::agent::hooks::sync::task(&fresh, &w.task(), Timestamp::now()).unwrap();
    }

    // Then exactly one wait_wake keyed by attempt and state carries the gate_ref:
    let wakes = w.messages("wait_wake");
    assert_eq!(wakes.len(), 1, "{wakes:?}");
    let key = format!("wake:{}/R1/{attempt}/pass", w.id);
    assert_eq!(wakes[0].0.subject.as_ref().unwrap()["key"], key.as_str());
    assert_eq!(w.wake(&receipt.request_id), WakeState::Fired);
    let task = w.task();
    let gate_ref = crate::gate_tier::gate_readback(w.ctx(), &task)
        .unwrap()
        .gate_ref
        .unwrap();
    let section = crate::agent::hooks::surfaces::worker_gate_section(w.ctx(), &task, false)
        .unwrap()
        .unwrap();
    assert!(
        wakes[0]
            .1
            .contains(&serde_json::to_string(&gate_ref).unwrap())
    );
    assert!(wakes[0].1.contains(&section.contract_sha256));

    // When the resumed worker reports done with them in turn 2:
    let done = w.op(
        Some(2),
        "done-1",
        "done",
        json!({"summary": SUMMARY, "gate_ref": gate_ref, "contract_sha256": section.contract_sha256}),
    );

    // Then the task is in Review exactly as `corral done` leaves it, with AgentReport v1:
    committed(&done);
    let task = w.task();
    assert_eq!(task.status, Status::Review);
    assert_eq!(task.runs[0].outcome, Some(RunOutcome::Done));
    let report = &w.state().reports[0];
    let carried = report.gate_ref.as_ref().unwrap();
    assert_eq!(
        (carried.attempt.as_str(), carried.tier.as_str()),
        (gate_ref.attempt.as_str(), gate_ref.tier.as_str())
    );
    assert_eq!(carried.receipt_sha256, gate_ref.receipt_sha256);
    assert_eq!(
        (report.turn, report.contract_sha256.as_deref()),
        (Some(2), Some(section.contract_sha256.as_str()))
    );
}

#[test]
fn a_gate_refused_before_submission_is_rejected_and_arms_no_wake() {
    // Given a fence that commits between the gate's preparation and its submission:
    let w = worker(true);
    let root = w.ctx().store.root().to_path_buf();
    let (container, generation) = (w.container(), w.host.identity.generation);
    let fence = move |point: &str| {
        if point == "gate_submit_before_lock" {
            let store = Store::discover(&root).unwrap();
            crate::agent::fence_generation(&store, &container, generation, "resume").unwrap();
        }
    };
    let refused = with_failpoint(fence, || w.op(None, "gate-r", "gate", json!({})));

    // Then the call answers rejected submission_refused with the reason, and no wake is armed:
    let receipt = refused.receipt().unwrap();
    assert_eq!(receipt.state, ReceiptState::Rejected);
    assert_eq!(receipt.result["outcome"], "submission_refused");
    assert_eq!(receipt.result["code"], "agent_fenced");
    assert!(!receipt.result["reason"].as_str().unwrap().is_empty());
    assert_eq!(w.wake(&receipt.request_id), WakeState::Cancelled);
    let later = Timestamp::now() + SignedDuration::from_secs(3600);
    w.sync(later);
    assert!(w.messages("wait_wake").is_empty());
    assert_eq!(w.calls_of("pane run"), 0, "no gate started");
}

#[test]
fn a_gate_whose_pane_ipc_faulted_answers_uncertain_and_wakes_the_worker_once() {
    // Given an executor that died inside the pane IPC:
    let w = worker(true);
    let crash = |point: &str| {
        if point == "gate_submit_ipc" {
            panic!("injected crash in the pane IPC");
        }
    };
    let crashed = with_failpoint(crash, || {
        catch_unwind(AssertUnwindSafe(|| w.op(None, "gate-u", "gate", json!({})))).is_err()
    });
    assert!(crashed);
    let request = tool_request_id("s1", "gate-u");
    w.ctx()
        .store
        .update(&w.id, |t| {
            let state = core_agent::container_state_mut(t, &w.container()).unwrap();
            let reserved = state.receipts.iter_mut().find(|r| r.request_id == request);
            reserved.unwrap().executor.as_mut().unwrap().start_time = "0".into();
            Ok(())
        })
        .unwrap();

    // When the host retries the same request:
    let retried = w.op(None, "gate-u", "gate", json!({}));

    // Then it answers outcome unknown, and the worker is woken exactly once, by the gate's
    // terminal result or by gate_start_uncertain at the timeout, never by a started claim:
    let receipt = retried.receipt().unwrap();
    assert_eq!(receipt.state, ReceiptState::Uncertain);
    assert_eq!(receipt.result["outcome"], "uncertain");
    let timeout = w.ctx().store.config.agent.gate_start_uncertain_secs as i64;
    let later = Timestamp::now() + SignedDuration::from_secs(timeout + 1);
    for now in [Timestamp::now(), later, later] {
        w.sync(now);
    }
    let wakes = w.messages("wait_wake");
    assert_eq!(wakes.len(), 1, "{wakes:?}");
    let state = wakes[0].0.subject.as_ref().unwrap()["state"].clone();
    assert!(
        ["lost", "gate_start_uncertain"].contains(&state.as_str().unwrap()),
        "{state}"
    );
    let settled = w.state().receipt(&request).cloned().unwrap();
    assert_ne!(settled.state, ReceiptState::Committed, "{settled:?}");
    assert_eq!(w.calls_of("pane run"), 0, "the killed IPC never ran");
}

#[test]
fn done_with_a_pending_question_is_refused_with_the_key_and_the_turn_awaits_the_answer() {
    let mut w = worker(false);
    w.event("turn_started", 1, Some(bridge("k1")));
    let ask = json!({"kind": "clarification", "text": "Which file?", "blocking": true});
    let asked = committed(&w.op(Some(1), "ask-1", "ask", ask));
    let key = asked.result["key"].as_str().unwrap().to_owned();

    let refused = w.op(Some(1), "done-1", "done", json!({"summary": SUMMARY}));

    let (code, reason) = rejection(&refused);
    assert_eq!(code, "question_state");
    assert!(
        reason.starts_with("open_questions:") && reason.contains(&key),
        "{reason}"
    );
    let task = w.task();
    assert_eq!((task.status, task.runs[0].outcome), (Status::Running, None));
    assert!(w.state().questions[0].recorded_pending());
    w.event("turn_ended", 1, None);
    w.sync(Timestamp::now());
    assert_eq!(w.class(1), Some(TurnClass::AwaitingAnswer));
    assert!(w.messages("turn_nudge").is_empty());
}

#[test]
fn a_non_blocking_question_returns_at_once_and_independent_work_continues() {
    let mut w = worker(false);
    w.event("turn_started", 1, Some(bridge("k1")));
    let ask = json!({"kind": "technical_choice", "text": "Tabs or spaces?", "options": ["tabs", "spaces"]});
    let asked = committed(&w.op(Some(1), "ask-1", "ask", ask));
    assert_eq!(
        (
            asked.result["state"].as_str(),
            asked.result["blocking"].as_bool()
        ),
        (Some("open"), Some(false))
    );
    let spec = crate::progress_spec_revision(&w.task()).unwrap();
    let progress = json!({
        "task": w.id, "run": "R1", "spec_revision": spec, "expected_revision": 0,
        "phases": [{"id": "p", "label": "P", "steps": [
            {"id": "s", "label": "S", "status": {"state": "in_progress"}, "criteria": []}]}],
    });
    committed(&w.op(Some(1), "progress-1", "progress", progress));
    let synced = w.sync(Timestamp::now());
    assert_eq!(synced.attention, None);
    assert_eq!(w.task().status, Status::Running);
}

#[test]
fn a_silent_turn_after_done_and_changes_is_no_report_and_spends_no_refunded_nudge() {
    let mut w = worker(false);
    let now = Timestamp::now();
    // Given a silent first turn that spent this generation's nudge:
    w.turn(1, bridge("k1"));
    w.sync(now);
    assert_eq!(w.class(1), Some(TurnClass::NoReport));
    assert_eq!(w.messages("turn_nudge").len(), 1);
    // And a turn that reported done:
    w.event("turn_started", 2, Some(bridge("k2")));
    committed(&w.op(Some(2), "done-1", "done", json!({"summary": SUMMARY})));
    w.event("turn_ended", 2, None);
    w.sync(now);
    assert_eq!(w.class(2), Some(TurnClass::Completed));
    assert_eq!(w.task().status, Status::Review);

    // When corral changes reopens the run and the next turn ends without a report:
    w.fx.ctx.store.config.dispatch.paused = false;
    crate::request_changes(w.ctx(), &w.id, "Handle the empty input too.").unwrap();
    assert_eq!(w.task().status, Status::Running);
    w.turn(3, bridge("k3"));
    let synced = w.sync(now);

    // Then the earlier report does not count, no second nudge is posted, and Needs me holds:
    assert_eq!(w.class(3), Some(TurnClass::NoReport));
    assert_eq!(
        w.messages("turn_nudge").len(),
        1,
        "a spent nudge is not refunded"
    );
    let attention = synced.attention.unwrap();
    assert!(
        attention.starts_with(crate::agent::turns::HOLD_PREFIX),
        "{attention}"
    );
    crate::sync_at(w.ctx(), Timestamp::now()).unwrap();
    let task = w.task();
    assert_eq!(task.status, Status::NeedsMe);
    let excerpt = task.runs[0].blocked_excerpt.clone().unwrap_or_default();
    assert!(
        excerpt.starts_with("turn_ended_without_report: "),
        "{excerpt}"
    );

    // And the owner's next turn settles the hold:
    w.event("turn_started", 4, Some(json!({"kind": "owner_terminal"})));
    assert_eq!(w.sync(Timestamp::now()).attention, None);
    assert_eq!(w.task().status, Status::Running);
}

#[test]
fn an_owner_typed_turn_is_owner_engaged_by_turn_id_and_gets_no_nudge() {
    let mut w = worker(false);
    w.turn(1, json!({"kind": "owner_terminal"}));
    w.sync(Timestamp::now());
    assert_eq!(w.class(1), Some(TurnClass::OwnerEngaged));
    assert!(w.messages("turn_nudge").is_empty());

    w.turn(2, bridge("k2"));
    let synced = w.sync(Timestamp::now());
    assert_eq!(
        (w.class(1), w.class(2)),
        (Some(TurnClass::OwnerEngaged), Some(TurnClass::NoReport))
    );
    let nudges = w.messages("turn_nudge");
    assert_eq!(nudges.len(), 1);
    let key = format!("nudge:R1:{}", w.host.identity.generation);
    assert_eq!(nudges[0].0.subject.as_ref().unwrap()["key"], key.as_str());
    assert_eq!(
        synced.attention, None,
        "the nudge is still pending its answer"
    );
}

#[test]
fn a_bound_worker_gets_its_run_prompt_once_with_the_gate_section_from_the_readback() {
    let w = worker(true);
    w.sync(Timestamp::now());
    assert!(w.messages("run_prompt").is_empty(), "no prompt file yet");
    let prompt = w.ctx().store.prompts_dir().join(format!("{}-R1.md", w.id));
    std::fs::create_dir_all(prompt.parent().unwrap()).unwrap();
    std::fs::write(&prompt, "# Task\n").unwrap();
    w.sync(Timestamp::now());
    w.sync(Timestamp::now());
    let prompts = w.messages("run_prompt");
    assert_eq!(prompts.len(), 1);
    let body = &prompts[0].1;
    let section = crate::agent::hooks::surfaces::worker_gate_section(w.ctx(), &w.task(), true)
        .unwrap()
        .unwrap();
    assert!(body.contains(&format!("Read {} in full", prompt.display())));
    assert!(body.contains(&section.text), "{body}");
    assert!(body.contains(&format!("contract_sha256: {}", section.contract_sha256)));
    assert_eq!(crate::scan_gate_instructions(body, &[]), None, "{body}");
}

#[test]
fn a_wait_needs_a_deadline_and_wakes_once_when_it_leaves_pending() {
    let w = worker(false);
    let channel = json!({"kind": "channel", "channel_id": "ci"});
    let unobservable = json!({"id": "w1", "condition": "CI green", "handle": channel});
    let (code, reason) = rejection(&w.op(None, "wait-0", "wait", unobservable));
    assert_eq!(code, "bad_request");
    assert!(reason.starts_with("unobservable_wait"), "{reason}");

    let deadline = Timestamp::now() + SignedDuration::from_secs(3600);
    let wait =
        json!({"id": "w1", "condition": "CI green", "deadline": deadline, "handle": channel});
    let registered = committed(&w.op(None, "wait-1", "wait", wait));
    assert_eq!(w.wake(&registered.request_id), WakeState::Armed);
    w.ctx()
        .store
        .update(&w.id, |t| {
            t.runs[0].obligations.waits[0].observation.state = WaitState::Ready;
            Ok(())
        })
        .unwrap();
    w.sync(Timestamp::now());
    w.sync(Timestamp::now());
    let state = w.task().runs[0].obligations.waits[0].observation.state;
    let wakes = w.messages("wait_wake");
    assert_eq!(wakes.len(), 1, "{wakes:?}");
    let key = format!("wake:{}/R1/w1/{}", w.id, state.as_str());
    assert_eq!(wakes[0].0.subject.as_ref().unwrap()["key"], key.as_str());
    assert_eq!(w.wake(&registered.request_id), WakeState::Fired);
}

#[test]
fn a_pi_native_dispatch_with_an_anthropic_model_is_refused_and_openai_passes() {
    let w = worker(false);
    let mut ctx = ctx_at(w.ctx().store.root());
    let package = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../packages/corral-pi");
    ctx.store.config.agent.assets_dir = Some(package);
    for vendor in ["anthropic", "openai"] {
        let profile = serde_json::from_value(json!({
            "harness": "pi-native", "model": "gpt-6.1-sol", "vendor": vendor,
        }))
        .unwrap();
        ctx.store.config.profiles.insert(vendor.into(), profile);
    }
    let resolve = |name: &str| {
        let choice = corral_core::ProfileChoice {
            profile: Some(name.into()),
            ..Default::default()
        };
        ctx.store
            .config
            .resolve_profile(&choice, None, None)
            .unwrap()
    };
    let check =
        |name| crate::agent::hooks::preflight::check(&ctx, AgentRole::Worker, &resolve(name));
    let refused = format!("{:#}", check("anthropic").unwrap_err());
    assert!(refused.starts_with("vendor_refused"), "{refused}");
    check("openai").unwrap();
}
