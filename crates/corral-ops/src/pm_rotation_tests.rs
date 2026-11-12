use super::*;
use corral_core::agent::{AgentRunState, ContainerRef, paths};
use corral_core::inbox::{MAX_PAYLOAD, MessageBinding, Post, Urgency};
use serde_json::json;

use crate::agent::BindTarget;
use crate::agent::bridge::fixture::{self as f, Fx, HostProc, SESSION};
use crate::agent::launch::{BOOTSTRAP_FILE, generation_dir};
use crate::agent::recovery::PmRecoveryInProgress;
use crate::agent::test_support as ts;

fn lines(values: &[Value]) -> String {
    values.iter().map(|value| format!("{value}\n")).collect()
}

/// A Pi session whose one assistant response used `input` tokens.
fn usage_session(id: &str, response: &str, input: u64) -> String {
    let at = jiff::Timestamp::now().to_string();
    lines(&[
        json!({"type": "session", "version": 3, "id": id, "timestamp": at}),
        json!({"type": "message", "id": format!("m-{response}"), "timestamp": at,
            "message": {"role": "assistant", "provider": "openai", "model": "gpt-fixture",
                "responseId": response, "usage": {"input": input, "output": 1},
                "content": [], "stopReason": "stop"}}),
    ])
}

fn dialog(id: &str, session: &str, status: &str, expires: i64, message: Option<&str>) -> Value {
    json!({"type": "custom", "customType": DIALOG_ENTRY, "data": {
        "version": 1, "id": id, "session": session, "generation": 1, "key": id,
        "questions": [], "expires": expires, "status": status, "message": message,
    }})
}

/// A bound PM whose live host wrote one response to its session.
fn bound_pm() -> (Fx, ts::Host, HostProc) {
    let fx = crate::agent::hooks::pm::tests::fixture();
    let host = HostProc::spawn(true);
    let target = BindTarget::Pm {
        project: String::new(),
    };
    let bound = fx.bind(&target, "pm-pane", &host);
    f::write_session(&bound, &usage_session(SESSION, "r1", 10));
    (fx, bound, host)
}

fn session_path(fx: &Fx) -> PathBuf {
    paths::container_sessions_dir(&fx.ctx.store, &ContainerRef::Pm).join(format!("{SESSION}.jsonl"))
}

fn edit_pm(fx: &Fx, edit: impl FnOnce(&mut AgentRunState)) {
    pm_state::update(&fx.ctx.store, |pm| {
        edit(&mut pm.as_mut().unwrap().state);
        Ok(((), true))
    })
    .unwrap();
}

fn inbox_message(fx: &Fx) {
    Inbox::new(&fx.ctx.store, Recipient::Pm)
        .unwrap()
        .post(Post {
            kind: "owner_note",
            urgency: Urgency::Info,
            author: "owner",
            body: "keep the release frozen",
            binding: MessageBinding::default(),
            subject: None,
            max_payload: MAX_PAYLOAD,
        })
        .unwrap();
}

fn board_question(fx: &Fx) {
    let question = serde_json::from_value(json!({
        "version": 1, "key": "pm/Q1", "revision": 1,
        "asked": {"request_id": "rq-fixture", "payload_sha256": "fixture",
                  "session_id": SESSION, "generation": 1, "at": "2026-10-15T00:00:00Z"},
        "kind": "clarification", "blocking": true, "text": "Which release?",
        "state": "open", "wake": {"route": "none", "key": "fixture", "state": "posted"}
    }))
    .unwrap();
    edit_pm(fx, |state| state.questions.push(question));
}

fn job(fx: &Fx) {
    let job = serde_json::from_value(json!({
        "id": "J1", "generation": 1, "command": "sleep 600", "state": "running",
        "pid": 4242, "start_time": "1", "prepared_at": "2026-10-15T00:00:00Z",
    }))
    .unwrap();
    edit_pm(fx, |state| state.jobs.push(job));
}

fn plan_step(fx: &Fx) {
    let todo = serde_json::from_value(json!({
        "revision": 1, "spec_revision": "pm", "plan": "P1",
        "phases": [{"id": "ship", "label": "Ship", "steps": [
            {"id": "s1", "label": "Dispatch T-1", "status": {"state": "in_progress"}},
        ]}],
    }))
    .unwrap();
    edit_pm(fx, |state| state.todo = Some(todo));
}

fn owner_dialog(fx: &Fx) {
    let mut text = std::fs::read_to_string(session_path(fx)).unwrap();
    text.push_str(&lines(&[dialog("d1", SESSION, "pending", i64::MAX, None)]));
    std::fs::write(session_path(fx), text).unwrap();
}

fn dialog_journal_unreadable(fx: &Fx) {
    let path = session_path(fx);
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
}

/// A blocker's reason tag and the fixture step that opens it.
type Case = (&'static str, fn(&Fx));

#[test]
fn each_open_obligation_blocks_rotation_and_changes_nothing() {
    let cases: [Case; 6] = [
        ("inbox_message", inbox_message),
        ("board_question", board_question),
        ("job", job),
        ("plan_step", plan_step),
        ("owner_dialog", owner_dialog),
        ("dialog_journal_unreadable", dialog_journal_unreadable),
    ];
    for (name, setup) in cases {
        let (fx, _bound, mut host) = bound_pm();
        setup(&fx);
        let before = pm_state::load(&fx.ctx.store).unwrap();
        let inbox = Inbox::new(&fx.ctx.store, Recipient::Pm).unwrap();
        let pending = inbox.pending_messages().unwrap();

        let outcome = rotate(&fx.ctx, "context_growth", false).unwrap();

        let RotateOutcome::Blocked {
            generation,
            reasons,
        } = outcome
        else {
            panic!("{name}: rotated past an open obligation");
        };
        assert_eq!(generation, 1, "{name}");
        let named: Vec<Value> = reasons
            .iter()
            .map(|reason| serde_json::to_value(reason).unwrap()["reason"].clone())
            .collect();
        assert_eq!(named, vec![json!(name)], "{name}");
        assert!(!reasons[0].sentence.is_empty(), "{name}");
        assert_eq!(pm_state::load(&fx.ctx.store).unwrap(), before, "{name}");
        assert_eq!(inbox.pending_messages().unwrap(), pending, "{name}");
        assert!(fx.calls("pane run").is_empty(), "{name}");
        assert!(fx.calls("workspace create").is_empty(), "{name}");
        assert!(fx.events("agent_fenced").is_empty(), "{name}");
        assert!(!host.reaped(), "{name}: the live host was stopped");
    }
}

#[test]
fn a_clear_barrier_rotates_into_a_fresh_session_and_keeps_the_old_one_collectable() {
    let (fx, bound, mut host) = bound_pm();
    let store = &fx.ctx.store;
    assert_eq!(barrier(&fx.ctx).unwrap(), Barrier::Clear);

    let outcome = rotate(&fx.ctx, "context_growth", false).unwrap();

    let RotateOutcome::Rotated {
        pane,
        generation,
        fenced,
        previous_session,
        overridden,
    } = outcome
    else {
        panic!("a clear barrier did not rotate");
    };
    assert_eq!((pane.as_str(), generation, fenced), ("pm-pane", 2, 1));
    assert_eq!(previous_session.as_deref(), Some(SESSION));
    assert!(overridden.is_empty());
    assert!(host.reaped(), "the idle host was not stopped");
    assert!(fx.calls("pane focus").is_empty());
    assert_eq!(fx.calls("pane run").len(), 1);
    let dir = generation_dir(store, &Recipient::Pm, 2).unwrap();
    let bootstrap: Value =
        serde_json::from_slice(&std::fs::read(dir.join(BOOTSTRAP_FILE)).unwrap()).unwrap();
    assert_eq!(bootstrap["fresh_session"], true);
    assert!(bootstrap.get("resume_session").is_none_or(Value::is_null));
    let notice = Inbox::new(store, Recipient::Pm)
        .unwrap()
        .pending_messages()
        .unwrap()
        .into_iter()
        .find(|message| message.kind == "recovery_notice")
        .unwrap();
    assert!(notice.body.contains("with a fresh conversation"));
    assert!(notice.body.contains(&format!("(Pi session {SESSION})")));
    assert!(notice.body.contains("corral_notes and corral_todo"));
    assert_eq!(
        notice.subject.unwrap()["rotation"]["previous_session"],
        SESSION
    );

    // The successor's host starts a new Pi session, as a fresh bootstrap makes it.
    let mut next = bound.clone();
    next.identity.generation = 2;
    next.identity.project = bootstrap["project"].as_str().unwrap().into();
    next.identity.session_id = "rotated-session".into();
    next.host_cap = ts::secret("rotated-host", 2);
    next.ctl_cap = ts::secret("rotated-ctl", 2);
    next.nonce = bootstrap["bind_nonce"].as_str().unwrap().into();
    let file = bound.sessions.join("rotated-session.jsonl");
    let successor = HostProc::spawn(true);
    let payload = json!({
        "ctl_cap_sha256": paths::sha256_hex(next.ctl_cap.as_bytes()),
        "session_file": file, "pid": successor.pid(),
        "start_time": successor.start_time(), "pane": pane,
    });
    let request = crate::agent::event_request_id(2, 0);
    let input = ts::envelope(&next, "hello", &request, payload, true);
    ts::committed(&crate::agent::call(&fx.ctx, "hello", &input).unwrap());
    std::fs::write(&file, usage_session("rotated-session", "r2", 20)).unwrap();

    let pm = pm_state::load(store).unwrap().unwrap();
    let old = pm.state.generation(1).unwrap();
    assert_eq!(old.fenced.as_ref().unwrap().reason, "rotate");
    let old = old.bound.as_ref().unwrap();
    assert_eq!(old.session_id, SESSION);
    assert_eq!(old.session_file, f::session_file(&bound));
    let new = pm.state.generation(2).unwrap().bound.as_ref().unwrap();
    assert_eq!(new.session_id, "rotated-session");
    assert_eq!(new.session_file, file);
    let usage = crate::agent::usage::collect_container(&ContainerRef::Pm, store).unwrap();
    assert_eq!(usage.input, 30);
    assert_eq!(fx.events("pm_rotated").len(), 1);
    assert!(fx.events("pm_relaunched").is_empty());
}

#[test]
fn a_rotation_while_another_entry_holds_the_recovery_lease_is_refused_in_progress() {
    let (fx, _bound, mut host) = bound_pm();
    let claim = fx.ctx.store.try_lock("pm-recovery").unwrap().unwrap();
    let before = pm_state::load(&fx.ctx.store).unwrap();

    let error = rotate(&fx.ctx, "context_growth", false).unwrap_err();

    assert!(error.downcast_ref::<PmRecoveryInProgress>().is_some());
    let error = barrier(&fx.ctx).unwrap_err();
    assert!(error.downcast_ref::<PmRecoveryInProgress>().is_some());
    assert_eq!(pm_state::load(&fx.ctx.store).unwrap(), before);
    assert!(fx.calls("pane run").is_empty());
    assert!(!host.reaped());
    drop(claim);
}

#[test]
fn owner_dialogs_block_while_pending_or_answered_and_undelivered() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("session.jsonl");
    let now = 1_000_000;
    let answer = "[ask_user answer d-answered]\nRelease: hold";
    let text = lines(&[
        dialog("d-pending", SESSION, "pending", now + 1, None),
        dialog("d-expired", SESSION, "pending", now, None),
        dialog("d-answered", SESSION, "answered", now + 1, Some(answer)),
        json!({"type": "message", "id": "u1", "message": {"role": "user", "content": [
            {"type": "text", "text": "[ask_user answer d-answered]\n"},
            {"type": "text", "text": "Release: hold"},
        ]}}),
        dialog(
            "d-undelivered",
            SESSION,
            "answered",
            now + 1,
            Some("not yet sent"),
        ),
        dialog("d-unrecorded", SESSION, "answered", now + 1, None),
        dialog("d-withdrawn", SESSION, "pending", now + 1, None),
        dialog("d-withdrawn", SESSION, "withdrawn", now + 1, None),
        dialog("d-other", "another-session", "pending", now + 1, None),
    ]) + "not json\n";
    std::fs::write(&file, text).unwrap();

    let owed = open_dialogs(&file, SESSION, now).unwrap();

    let owner = |id: &str, state| Blocker::OwnerDialog {
        id: id.into(),
        state,
    };
    assert_eq!(
        owed,
        vec![
            owner("d-pending", DialogState::Pending),
            owner("d-undelivered", DialogState::AnsweredUndelivered),
            owner("d-unrecorded", DialogState::AnsweredUndelivered),
        ]
    );
}

#[test]
fn a_missing_journal_owes_no_dialog_and_an_unreadable_one_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let missing = open_dialogs(&dir.path().join("absent.jsonl"), SESSION, 0).unwrap();
    assert_eq!(missing, Vec::<Blocker>::new());
    assert!(open_dialogs(dir.path(), SESSION, 0).is_err());
}
