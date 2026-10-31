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
pub(super) struct Session {
    pub(super) fx: Fx,
    _host: HostProc,
    pub(super) bound: ts::Host,
    pub(super) id: String,
    seq: u64,
    log: PathBuf,
}

pub(super) fn session() -> Session {
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

    /// The host's journal entry for owner text typed into the pane, or the owner's answer to
    /// a confirmation of `command` shown for the tool call `call`.
    pub(super) fn journal(&mut self, text: &str, answer: Option<(&str, &str)>) -> String {
        let source = if answer.is_some() {
            "confirmation"
        } else {
            "pane"
        };
        let mut payload = json!({"kind": "owner_input", "source": source, "text": text,
            "sha256": paths::sha256_hex(text.as_bytes())});
        if let Some((command, call)) = answer {
            payload["command"] = json!(command);
            payload["answers"] = json!(self.request(call));
        }
        let receipt = ts::committed(&self.journal_raw(payload));
        receipt.result["input"].as_str().unwrap().to_owned()
    }

    pub(super) fn request(&self, call: &str) -> String {
        tool_request_id(f::SESSION, call)
    }

    pub(super) fn act(&self, call: &str, payload: &Value) -> CallResponse {
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
    pub(super) fn runs(&self) -> Vec<String> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    pub(super) fn task(&self, status: Status) -> String {
        self.fx
            .ctx
            .store
            .create("Owner action target", None, status, "")
            .unwrap()
            .id
    }

    pub(super) fn stored(&self, call: &str) -> Option<RequestReceipt> {
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

pub(super) fn executor_of(process: &HostProc) -> Executor {
    Executor {
        pid: process.pid(),
        start_time: process.start_time(),
    }
}

pub(super) fn dead_process() -> Executor {
    let mut process = HostProc::spawn(false);
    let executor = executor_of(&process);
    process.kill();
    executor
}

pub(super) fn receipt(response: &CallResponse) -> (RequestReceipt, bool) {
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
    let answer = s.journal("y", Some((&format!("corral discard {task}"), "x")));
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
    // And a journal entry must carry the digest of its text; a confirmation names its command
    // and the request it answers.
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
    let unbound = s.journal_raw(
        json!({"kind": "owner_input", "source": "confirmation", "text": "y",
        "sha256": paths::sha256_hex(b"y"), "command": "corral mode assist"}),
    );
    assert_eq!(ts::code(&unbound), Some("bad_request"));
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
        let no = format!("{name}-no");
        let asked = s.act(&no, &payload);
        assert_eq!(ts::code(&asked), Some("confirmation_required"), "{name}");
        assert_eq!(rejected_detail(&asked)["command"], command, "{name}");
        assert!(s.stored(&no).is_none());
        let before = s.runs().len();
        // When the owner answers N for this call and the host resends the identical call, a
        // declined receipt is recorded, nothing runs, and replaying the call returns it.
        s.journal("n", Some((&command, &no)));
        let (declined, _) = receipt(&s.act(&no, &payload));
        assert_eq!(declined.state, ReceiptState::Committed, "{name}");
        assert_eq!(declined.result["outcome"], "declined", "{name}");
        let (again, replayed) = receipt(&s.act(&no, &payload));
        assert!(replayed && again == declined, "{name}");
        assert_eq!(s.runs().len(), before, "{name}");
        // When the owner answers y, the command runs exactly once, and a replay of the identical
        // call returns that receipt without another question or effect.
        let yes = format!("{name}-yes");
        assert_eq!(
            ts::code(&s.act(&yes, &payload)),
            Some("confirmation_required")
        );
        s.journal("y", Some((&command, &yes)));
        let (ran, _) = receipt(&s.act(&yes, &payload));
        assert_eq!(ran.result["outcome"], "executed", "{name}: {ran:?}");
        assert_eq!(ran.result["confirmed"], true, "{name}");
        assert_eq!(ran.result["command"], command, "{name}");
        let (replay, replayed) = receipt(&s.act(&yes, &payload));
        assert!(replayed && replay == ran, "{name}");
        let runs = s.runs();
        assert_eq!(runs.len(), before + 1, "{name}");
        assert!(
            runs[before].ends_with(&command.replacen("corral ", "|", 1)),
            "{name}: {runs:?}"
        );
        // And an answer binds one call and one command: another call, or an answer shown for
        // another command, still waits for the owner.
        let other = format!("{name}-other");
        assert_eq!(
            ts::code(&s.act(&other, &payload)),
            Some("confirmation_required")
        );
        s.journal("y", Some(("corral config set x 1", &other)));
        assert_eq!(
            ts::code(&s.act(&other, &payload)),
            Some("confirmation_required")
        );
        assert_eq!(s.runs().len(), before + 1, "{name}");
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
        json!({"kind": "finish", "target": "spike", "args": ["--title", "Spike"]}),
    ];
    for (n, mut payload) in cases.into_iter().enumerate() {
        payload["input"] = json!(s.journal("do it", None));
        let (ran, _) = receipt(&s.act(&format!("ordinary-{n}"), &payload));
        assert_eq!(ran.result["outcome"], "executed", "{payload}: {ran:?}");
        assert_eq!(ran.result["class"], "ordinary", "{payload}");
    }
    assert_eq!(s.runs().len(), 11);
    assert!(s.runs()[5].ends_with(&format!("|verdict {task} approve --notes looks good")));
    // A finish names a scratch slug, anything outside the closed set is unsupported, a flag the
    // command does not take is refused, and an ordinary kind takes no confirmation.
    let input = s.journal("finish this", None);
    for (call, payload, code) in [
        (
            "finish",
            json!({"kind": "finish", "target": "Not A Slug", "input": input}),
            "bad_request",
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
    assert_eq!(s.runs().len(), 11);
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

/// A claimed reservation whose executor and command child both died: `finish` (when given)
/// ran first, as the child would after its command returned.
fn claimed_then_dead(
    s: &Session,
    call: &str,
    payload: &Value,
    argv: &[String],
    finish: Option<Option<&str>>,
) -> String {
    let mut executor = HostProc::spawn(false);
    let request = s.reserve(call, payload, &executor_of(&executor));
    let value = format!("{}:{request}", s.id);
    let store = &s.fx.ctx.store;
    let via = claim(store, &value, argv, executor.pid()).unwrap().unwrap();
    if let Some(error) = finish {
        crate::assistant::owner_action::finish_in(store, &via, error).unwrap();
    }
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
    request
}

#[test]
fn crashes_settle_from_completion_records_or_target_state_never_from_events() {
    let mut s = session();
    let task = s.task(Status::Review);
    let input = s.journal("land it", None);
    let approve = json!({"kind": "approve", "target": task, "input": input});
    let argv = ["approve".to_owned(), task.clone()];
    // A crash between the owner's answer and the call: no reservation, so nothing ran.
    assert!(s.stored("lost").is_none());
    assert_eq!(
        classify_corral("owner_action", None, false).0,
        corral_core::agent::ResumeClass::NotExecuted
    );
    // An executor that died after its reservation and before any command: not executed.
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
    // The claim refuses a different command, ignores another parent and runs once.
    let mut executor = HostProc::spawn(false);
    let request = s.reserve("claim", &approve, &executor_of(&executor));
    let value = format!("{}:{request}", s.id);
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
        (input.as_str(), "land it")
    );
    assert!(claim(store, &value, &argv, executor.pid()).is_err());
    executor.kill();
    // A child that died after preparation and failure events naming its request, with no
    // completion record: those events prove nothing, so the receipt is uncertain, not success.
    let request = claimed_then_dead(&s, "prepared", &approve, &argv, None);
    for kind in [
        "candidate_rebased",
        "verdict_carried",
        "landing_stale",
        "approved",
    ] {
        store
            .log_event(
                Some(&task),
                kind,
                json!({"owner_authority": {"request": request}}),
            )
            .unwrap();
    }
    let (prepared, _) = receipt(&s.act("prepared", &approve));
    assert_eq!(prepared.state, ReceiptState::Uncertain, "{prepared:?}");
    assert_eq!(prepared.result["outcome"], "outcome_unknown");
    // A child that recorded its command's success, or its failure, settles from that record.
    claimed_then_dead(&s, "recorded", &approve, &argv, Some(None));
    let (recorded, _) = receipt(&s.act("recorded", &approve));
    assert_eq!(recorded.state, ReceiptState::Committed, "{recorded:?}");
    assert_eq!(
        (&recorded.result["outcome"], &recorded.result["reconciled"]),
        (&json!("executed"), &json!("completion_record"))
    );
    let failure = "landing_stale: the base moved; rebase and run the gate once";
    claimed_then_dead(&s, "failed", &approve, &argv, Some(Some(failure)));
    let (failed, _) = receipt(&s.act("failed", &approve));
    assert_eq!(failed.state, ReceiptState::Rejected, "{failed:?}");
    assert_eq!(
        (
            failed.result["outcome"].as_str(),
            failed.result["reason"].as_str()
        ),
        (Some("refused"), Some(failure))
    );
    // Without a record, only a verified terminal state counts: the task Done and its approved
    // event carrying this request.
    let request = claimed_then_dead(&s, "landed", &approve, &argv, None);
    store
        .update(&task, |task| {
            task.status = Status::Done;
            Ok(())
        })
        .unwrap();
    store
        .log_event(
            Some(&task),
            "approved",
            json!({"owner_authority": {"request": request}}),
        )
        .unwrap();
    let (landed, _) = receipt(&s.act("landed", &approve));
    assert_eq!(landed.state, ReceiptState::Committed, "{landed:?}");
    assert_eq!(landed.result["reconciled"], "target_state");
    assert!(s.runs().is_empty(), "nothing re-executed: {:?}", s.runs());
}
