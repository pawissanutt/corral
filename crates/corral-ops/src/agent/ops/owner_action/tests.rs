use std::path::PathBuf;

use corral_core::Status;
use corral_core::agent::{Executor, ReceiptState, RequestReceipt, paths};
use serde_json::{Value, json};

use super::*;
use crate::agent::admission::BindTarget;
use crate::agent::bridge::fixture::{self as f, Fx, HostProc};
use crate::agent::bridge::interrupted::classify_corral;
use crate::agent::test_support as ts;
use crate::agent::{event_request_id, tool_request_id};
use crate::assistant::owner_action::claim;

const PANE: &str = "w1:p7";

/// A-1 bound by a live stand-in host, with a stand-in corral that logs each command it runs.
struct Session {
    fx: Fx,
    _host: HostProc,
    bound: ts::Host,
    id: String,
    seq: u64,
    log: PathBuf,
}

fn session() -> Session {
    let fx = f::fixture(false, 1);
    let host = HostProc::spawn(true);
    let session = fx
        .ctx
        .store
        .create_assistant("pm-pi", None, Some(PANE), None)
        .unwrap();
    let target = BindTarget::Assistant {
        session: session.id.clone(),
    };
    let bound = fx.bind(&target, PANE, &host);
    let log = fx.root().join("owner-action.log");
    let fail = fx.root().join("owner-action.fail");
    let program = fx.root().join("fake-corral");
    ts::write_executable(
        &program,
        &format!(
            "#!/bin/sh\nprintf '%s|%s\\n' \"$CORRAL_OWNER_ACTION\" \"$*\" >> '{}'\n\
             if [ -f '{}' ]; then cat '{}' >&2; exit 1; fi\nprintf 'ran %s\\n' \"$*\"\n",
            log.display(),
            fail.display(),
            fail.display()
        ),
    );
    child::EXECUTABLE.with(|path| *path.borrow_mut() = Some(program));
    Session {
        fx,
        _host: host,
        bound,
        id: session.id,
        seq: 1,
        log,
    }
}

impl Session {
    fn ts(&self) -> ts::Fx {
        ts::Fx {
            _tmp: tempfile::tempdir().unwrap(),
            ctx: self.fx.ctx.clone(),
        }
    }

    fn journal_raw(&mut self, payload: Value) -> CallResponse {
        let request = event_request_id(self.bound.identity.generation, self.seq);
        self.seq += 1;
        let input = ts::envelope(&self.bound, "event", &request, payload, false);
        ts::send(&self.ts(), &input, "event")
    }

    /// The host's journal entry for owner text typed into the pane, or a confirmation answer.
    fn journal(&mut self, text: &str, command: Option<&str>) -> String {
        let source = if command.is_some() {
            "confirmation"
        } else {
            "pane"
        };
        let mut payload = json!({"kind": "owner_input", "source": source, "text": text,
            "sha256": paths::sha256_hex(text.as_bytes())});
        if let Some(command) = command {
            payload["command"] = json!(command);
        }
        let receipt = ts::committed(&self.journal_raw(payload));
        receipt.result["input"].as_str().unwrap().to_owned()
    }

    fn request(&self, call: &str) -> String {
        tool_request_id(f::SESSION, call)
    }

    fn act(&self, call: &str, payload: &Value) -> CallResponse {
        let input = ts::envelope(
            &self.bound,
            "owner_action",
            &self.request(call),
            payload.clone(),
            false,
        );
        ts::send(&self.ts(), &input, "owner_action")
    }

    /// Every command the stand-in corral ran: `<CORRAL_OWNER_ACTION>|<argv>`.
    fn runs(&self) -> Vec<String> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn task(&self, status: Status) -> String {
        self.fx
            .ctx
            .store
            .create("Owner action target", None, status, "")
            .unwrap()
            .id
    }

    fn stored(&self, call: &str) -> Option<RequestReceipt> {
        let state = self
            .fx
            .ctx
            .store
            .get_assistant_container(&self.id)
            .unwrap()
            .state;
        state.receipt(&self.request(call)).cloned()
    }

    /// A reservation made by `executor`, as an executor that stops right after it would leave.
    fn reserve(&self, call: &str, payload: &Value, executor: &Executor) -> String {
        let request = self.request(call);
        let input = ts::envelope(
            &self.bound,
            "owner_action",
            &request,
            payload.clone(),
            false,
        );
        let (envelope, _) = crate::agent::envelope::parse("owner_action", &input).unwrap();
        let mode = Mode::Mutation { settles_turn: None };
        let Admission::New(admitted) = admission::admit(&self.fx.ctx, &envelope, mode).unwrap()
        else {
            panic!("a fresh request is new");
        };
        let p: Payload = serde_json::from_value(payload.clone()).unwrap();
        let action =
            kinds::resolve(&p.kind, p.target.as_deref(), &p.args, p.expected_revision).unwrap();
        let (_, replayed) = reserve(&self.fx.ctx, &admitted, &p, &action, executor).unwrap();
        assert!(!replayed);
        request
    }
}

fn executor_of(process: &HostProc) -> Executor {
    Executor {
        pid: process.pid(),
        start_time: process.start_time(),
    }
}

fn dead_process() -> Executor {
    let mut process = HostProc::spawn(false);
    let executor = executor_of(&process);
    process.kill();
    executor
}

fn receipt(response: &CallResponse) -> (RequestReceipt, bool) {
    match response {
        CallResponse::Receipt { receipt, replayed } => (receipt.clone(), *replayed),
        other => panic!("expected a receipt, got {other:?}"),
    }
}

fn rejected_detail(response: &CallResponse) -> Value {
    response.to_json().unwrap()["result"].clone()
}

#[test]
fn a_cited_pane_instruction_runs_once_and_a_replay_returns_its_first_receipt() {
    // Given A-1 journaled the owner's pane message and a task in Review.
    let mut s = session();
    let task = s.task(Status::Review);
    let input = s.journal("T-5 looks good, please land it", None);
    assert_eq!(input, format!("{}/g1/i1", s.id));
    // When the model calls owner_action approve citing that input.
    let payload = json!({"kind": "approve", "target": task, "input": input});
    let first = s.act("approve-1", &payload);
    // Then the approve command ran once, as the reservation's child, with no confirmation.
    let (stored, replayed) = receipt(&first);
    assert!(!replayed);
    assert_eq!(stored.state, ReceiptState::Committed, "{stored:?}");
    let result = &stored.result;
    assert_eq!(result["outcome"], "executed");
    assert_eq!(
        (result["input"].as_str(), result["confirmed"].as_bool()),
        (Some(input.as_str()), Some(false))
    );
    assert_eq!(result["command"], format!("corral approve {task}"));
    assert_eq!(result["output"], format!("ran approve {task}"));
    assert!(result.get("argv").is_none());
    let request = s.request("approve-1");
    assert_eq!(s.runs(), vec![format!("{}:{request}|approve {task}", s.id)]);
    // When the host replays the same call after a crash, it gets the first receipt and nothing
    // runs again.
    let (again, replayed) = receipt(&s.act("approve-1", &payload));
    assert!(replayed);
    assert_eq!(again, stored);
    assert_eq!(s.runs().len(), 1);
}

#[test]
fn synthetic_inputs_never_authorize_an_owner_action() {
    // Given A-1 with one journaled pane input and one confirmation answer.
    let mut s = session();
    let task = s.task(Status::Review);
    s.journal("hello", None);
    let answer = s.journal("y", Some(&format!("corral discard {task}")));
    // When the model cites a corral wake, a PM outcome notice, a recovery notice, an input the
    // host never journaled, or the confirmation itself as the owner's instruction.
    for (call, cited) in [
        ("wake", "qwake:T-5:Q1:1"),
        ("pm", "pmi:PMI-1:2"),
        ("recovery", "recovery_notice:g1"),
        ("unjournaled", "A-1/g1/i9"),
        ("confirmation", answer.as_str()),
    ] {
        let payload = json!({"kind": "approve", "target": task, "input": cited});
        let refused = s.act(call, &payload);
        // Then each refuses not_owner_input, stores nothing and runs nothing.
        assert_eq!(ts::code(&refused), Some("not_owner_input"), "{call}");
        assert!(s.stored(call).is_none(), "{call}");
    }
    assert!(s.runs().is_empty());
    // And a journal entry must carry the digest of its text; a confirmation names its command.
    let forged = s.journal_raw(
        json!({"kind": "owner_input", "source": "pane", "text": "do it",
        "sha256": "0".repeat(64)}),
    );
    assert_eq!(ts::code(&forged), Some("bad_request"));
    let bare = s.journal_raw(
        json!({"kind": "owner_input", "source": "confirmation", "text": "y",
        "sha256": paths::sha256_hex(b"y")}),
    );
    assert_eq!(ts::code(&bare), Some("bad_request"));
}

#[test]
fn every_confirm_class_kind_waits_for_the_owner_and_runs_only_on_yes() {
    let mut s = session();
    let task = s.task(Status::Review);
    let cases: [(&str, Value, String); 6] = [
        (
            "drop",
            json!({"kind": "move", "target": task, "args": ["dropped"]}),
            format!("corral move {task} dropped"),
        ),
        (
            "force",
            json!({"kind": "move", "target": task, "args": ["ready", "--force"]}),
            format!("corral move {task} ready --force"),
        ),
        (
            "discard",
            json!({"kind": "discard", "target": task}),
            format!("corral discard {task}"),
        ),
        (
            "keep",
            json!({"kind": "discard", "target": task, "args": ["--keep-branch"]}),
            format!("corral discard {task} --keep-branch"),
        ),
        (
            "config",
            json!({"kind": "config_set", "target": "limits.running", "args": ["3"]}),
            "corral config set limits.running 3".into(),
        ),
        (
            "grant",
            json!({"kind": "mode", "target": "assist"}),
            "corral mode assist".into(),
        ),
    ];
    for (name, mut payload, command) in cases {
        // Given the owner's instruction for a confirm-class action.
        payload["input"] = json!(s.journal(&format!("please {name}"), None));
        // When the model calls it without the owner's answer, it waits: the refusal names the
        // command the host shows, and nothing runs or is stored.
        let asked = s.act(&format!("{name}-ask"), &payload);
        assert_eq!(ts::code(&asked), Some("confirmation_required"), "{name}");
        assert_eq!(rejected_detail(&asked)["command"], command, "{name}");
        assert!(s.stored(&format!("{name}-ask")).is_none());
        // When the owner answers N, a declined receipt is recorded and nothing runs.
        let mut no = payload.clone();
        no["confirmation"] = json!(s.journal("n", Some(&command)));
        let (declined, _) = receipt(&s.act(&format!("{name}-no"), &no));
        assert_eq!(declined.state, ReceiptState::Committed, "{name}");
        assert_eq!(declined.result["outcome"], "declined", "{name}");
        assert!(
            s.runs()
                .iter()
                .all(|run| !run.contains(&format!("{name}-yes")))
        );
        let before = s.runs().len();
        // When the owner answers y, the command runs exactly once, and a replay runs nothing.
        let mut yes = payload.clone();
        yes["confirmation"] = json!(s.journal("y", Some(&command)));
        let (ran, _) = receipt(&s.act(&format!("{name}-yes"), &yes));
        assert_eq!(ran.result["outcome"], "executed", "{name}: {ran:?}");
        assert_eq!(ran.result["confirmed"], true, "{name}");
        assert_eq!(ran.result["command"], command, "{name}");
        receipt(&s.act(&format!("{name}-yes"), &yes));
        let runs = s.runs();
        assert_eq!(runs.len(), before + 1, "{name}");
        assert!(
            runs[before].ends_with(&command.replacen("corral ", "|", 1)),
            "{name}: {runs:?}"
        );
        // And an answer cannot be reused or shown for another command.
        let reused = s.act(&format!("{name}-again"), &yes);
        assert_eq!(ts::code(&reused), Some("bad_request"), "{name}");
        let mut other = payload.clone();
        other["confirmation"] = json!(s.journal("y", Some("corral config set x 1")));
        let mismatch = s.act(&format!("{name}-other"), &other);
        assert_eq!(ts::code(&mismatch), Some("not_owner_input"), "{name}");
    }
}

#[test]
fn ordinary_kinds_run_on_the_instruction_without_a_confirmation() {
    let mut s = session();
    let task = s.task(Status::Review);
    let cases = [
        json!({"kind": "approve", "target": task}),
        json!({"kind": "dispatch", "target": task, "args": ["--plan"]}),
        json!({"kind": "queue", "target": task}),
        json!({"kind": "unqueue", "target": task}),
        json!({"kind": "move", "target": task, "args": ["ready"]}),
        json!({"kind": "verdict", "target": task, "args": ["approve", "--notes", "looks good"]}),
        json!({"kind": "review_run", "target": task}),
        json!({"kind": "plan_accept", "target": task, "args": ["--attempt", "pa-1"]}),
        json!({"kind": "park", "target": task, "args": ["--reason", "waiting"]}),
        json!({"kind": "revert", "target": task}),
    ];
    for (n, mut payload) in cases.into_iter().enumerate() {
        payload["input"] = json!(s.journal("do it", None));
        let (ran, _) = receipt(&s.act(&format!("ordinary-{n}"), &payload));
        assert_eq!(ran.result["outcome"], "executed", "{payload}: {ran:?}");
        assert_eq!(ran.result["class"], "ordinary", "{payload}");
    }
    assert_eq!(s.runs().len(), 10);
    assert!(s.runs()[5].ends_with(&format!("|verdict {task} approve --notes looks good")));
    // Finish has not landed, anything outside the closed set is unsupported, a flag the
    // command does not take is refused, and an ordinary kind takes no confirmation.
    let input = s.journal("finish this", None);
    for (call, payload, code) in [
        (
            "finish",
            json!({"kind": "finish", "target": "spike", "input": input}),
            "unsupported_action",
        ),
        (
            "push",
            json!({"kind": "push", "target": task, "input": input}),
            "unsupported_action",
        ),
        (
            "root",
            json!({"kind": "approve", "target": task, "args": ["--root", "/"], "input": input}),
            "bad_request",
        ),
        (
            "author",
            json!({"kind": "move", "target": task, "args": ["ready", "--author", "pm"],
                "input": input}),
            "bad_request",
        ),
        (
            "confirmed",
            json!({"kind": "approve", "target": task, "input": input, "confirmation": input}),
            "bad_request",
        ),
    ] {
        assert_eq!(ts::code(&s.act(call, &payload)), Some(code), "{call}");
    }
    assert_eq!(s.runs().len(), 10);
}

#[test]
fn a_target_held_by_autopilot_refuses_conflict_in_flight_naming_the_lock() {
    // Given autopilot holds T's approval lock.
    let mut s = session();
    let task = s.task(Status::Review);
    let input = s.journal("approve it", None);
    let payload = json!({"kind": "approve", "target": task, "input": input});
    let held =
        s.fx.ctx
            .store
            .try_lock(&format!("autopilot-{task}"))
            .unwrap();
    // When the owner's approve runs, it refuses naming the lock and runs nothing.
    let refused = s.act("held", &payload);
    assert_eq!(ts::code(&refused), Some("conflict_in_flight"));
    assert_eq!(
        rejected_detail(&refused)["holder"],
        format!("autopilot-{task}")
    );
    assert!(s.runs().is_empty() && s.stored("held").is_none());
    drop(held);
    // And when autopilot takes the lock after the probe, the command's own refusal is recorded
    // as conflict_in_flight too, once.
    std::fs::write(
        s.fx.root().join("owner-action.fail"),
        format!("Error: approval already in progress; run `corral approve {task}` after it"),
    )
    .unwrap();
    let (raced, _) = receipt(&s.act("raced", &payload));
    assert_eq!(raced.state, ReceiptState::Rejected);
    assert_eq!(raced.result["code"], "conflict_in_flight");
    assert_eq!(s.runs().len(), 1);
}

#[test]
fn crashes_leave_each_receipt_executed_or_not_executed_never_guessed() {
    let mut s = session();
    let task = s.task(Status::Review);
    let input = s.journal("drop it", None);
    let command = format!("corral discard {task}");
    // A crash between the owner's confirmation and the call: no reservation, so nothing ran.
    s.journal("y", Some(&command));
    assert!(s.stored("lost").is_none());
    assert_eq!(
        classify_corral("owner_action", None, false).0,
        corral_core::agent::ResumeClass::NotExecuted
    );
    // An executor that died after its reservation and before any command: replay settles it
    // not executed and runs nothing.
    let approve = json!({"kind": "approve", "target": task, "input": input});
    s.reserve("early", &approve, &dead_process());
    let (early, replayed) = receipt(&s.act("early", &approve));
    assert!(replayed);
    assert_eq!(early.state, ReceiptState::Abandoned, "{early:?}");
    assert_eq!(early.result["outcome"], "not_executed");
    // While an executor still runs, a replay reports the reservation and runs nothing.
    let live = HostProc::spawn(false);
    s.reserve("running", &approve, &executor_of(&live));
    let (running, _) = receipt(&s.act("running", &approve));
    assert_eq!(running.state, ReceiptState::Reserved);
    // A command child that claimed its reservation, then died with its executor: the event log
    // decides. Its approved event names the request, so the receipt settles executed.
    let mut executor = HostProc::spawn(false);
    let request = s.reserve("late", &approve, &executor_of(&executor));
    let value = format!("{}:{request}", s.id);
    let argv = ["approve".to_owned(), task.clone()];
    let store = &s.fx.ctx.store;
    assert!(claim(store, &value, &["approve".into()], executor.pid()).is_err());
    assert!(
        claim(store, &value, &argv, std::process::id())
            .unwrap()
            .is_none()
    );
    let via = claim(store, &value, &argv, executor.pid())
        .unwrap()
        .unwrap();
    assert_eq!(
        (via.input.as_str(), via.owner_message.as_str()),
        (input.as_str(), "drop it")
    );
    assert!(claim(store, &value, &argv, executor.pid()).is_err());
    let dead = dead_process();
    store
        .settle_assistant_container(&s.id, |assistant, _| {
            let stored = assistant
                .state
                .receipts
                .iter_mut()
                .find(|r| r.request_id == request)
                .unwrap();
            stored.evidence[0].detail = Some(format!("{} {}", dead.pid, dead.start_time));
            Ok(())
        })
        .unwrap();
    executor.kill();
    store
        .log_event(
            Some(&task),
            "approved",
            json!({"owner_authority": {"request": request}}),
        )
        .unwrap();
    let (late, _) = receipt(&s.act("late", &approve));
    assert_eq!(late.state, ReceiptState::Committed, "{late:?}");
    assert_eq!(
        (
            late.result["outcome"].clone(),
            late.result["events"].clone()
        ),
        (json!("executed"), json!(["approved"]))
    );
    // The same crash with no event names no outcome: uncertain, reported, never re-executed.
    let mut executor = HostProc::spawn(false);
    let request = s.reserve("unknown", &approve, &executor_of(&executor));
    let value = format!("{}:{request}", s.id);
    claim(store, &value, &argv, executor.pid())
        .unwrap()
        .unwrap();
    store
        .settle_assistant_container(&s.id, |assistant, _| {
            let stored = assistant
                .state
                .receipts
                .iter_mut()
                .find(|r| r.request_id == request)
                .unwrap();
            stored.evidence[0].detail = Some(format!("{} {}", dead.pid, dead.start_time));
            Ok(())
        })
        .unwrap();
    executor.kill();
    let (unknown, _) = receipt(&s.act("unknown", &approve));
    assert_eq!(unknown.state, ReceiptState::Uncertain, "{unknown:?}");
    assert_eq!(unknown.result["outcome"], "outcome_unknown");
    assert!(s.runs().is_empty(), "nothing re-executed: {:?}", s.runs());
}
