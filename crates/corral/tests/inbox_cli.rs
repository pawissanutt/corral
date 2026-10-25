use std::path::Path;
use std::process::{Command, Output, Stdio};

use corral_core::Store;
use corral_core::inbox::{
    Inbox, MAX_PAYLOAD, Message, MessageBinding, Post, Receipt, ReceiptLevel, Recipient,
    SendOutcome, SessionBinding, TerminalState, Urgency,
};
use corral_ops::Ctx;
use corral_ops::delivery::{
    BindOutcome, Capabilities, DeliveryAdapter, DeliveryStatus, Observation, Readiness,
    RecipientRef, deliver,
};
use serde_json::{Value, json};

#[cfg(target_os = "linux")]
#[path = "inbox_cli/cancellation.rs"]
mod cancellation;
#[path = "inbox_cli/partial_cancellation.rs"]
mod partial_cancellation;

fn invoke(root: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_corral"));
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if name.starts_with("CORRAL_") || name.starts_with("HERDR_") {
            command.env_remove(key);
        }
    }
    command
        .env_remove("AI_AGENT")
        .env_remove("PI_CODING_AGENT")
        .current_dir(root)
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("CORRAL_ROOT", root)
        .env("HERDR_BIN_PATH", root.join("no-herdr"))
        .arg("--root")
        .arg(root)
        .arg("inbox")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn ok(root: &Path, args: &[&str]) -> String {
    let out = invoke(root, args);
    assert!(out.status.success(), "{args:?}: {out:?}");
    String::from_utf8(out.stdout).unwrap()
}

fn post(inbox: &Inbox<'_>, body: &str) -> Message {
    inbox
        .post(Post {
            kind: "owner_note",
            urgency: Urgency::Wake,
            author: "owner",
            body,
            binding: MessageBinding::default(),
            subject: None,
            max_payload: MAX_PAYLOAD,
        })
        .unwrap()
}

fn open_envelope(inbox: &Inbox<'_>) {
    let binding = SessionBinding {
        session: "fixture-session".into(),
        endpoint: None,
        transcript: None,
    };
    let mut envelope = inbox
        .publish(&binding, "fixture", MAX_PAYLOAD, false)
        .unwrap()
        .unwrap();
    inbox
        .begin_attempt(&mut envelope, jiff::Timestamp::now())
        .unwrap();
    inbox
        .finish_attempt(
            &mut envelope,
            SendOutcome::Uncertain {
                reason: "fixture interrupted".into(),
            },
        )
        .unwrap();
}

struct LateUncertainty;

impl DeliveryAdapter for LateUncertainty {
    fn harness(&self) -> &'static str {
        "fixture"
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            idempotent_retry: false,
            admission: true,
            non_waking: false,
            max_payload: MAX_PAYLOAD,
            send_budget: None,
            transcript_reader: true,
        }
    }
    fn readiness(&self, _: &Ctx, _: &RecipientRef) -> anyhow::Result<Readiness> {
        Ok(Readiness::Native)
    }
    fn bind(&self, _: &Ctx, _: &RecipientRef) -> anyhow::Result<BindOutcome> {
        Ok(BindOutcome::Bound(SessionBinding {
            session: "fixture".into(),
            endpoint: None,
            transcript: None,
        }))
    }
    fn send(&self, _: &Ctx, _: &SessionBinding, _: &corral_core::inbox::Envelope) -> SendOutcome {
        SendOutcome::Admitted {
            detail: json!({"claim":"original"}),
        }
    }
    fn observe(
        &self,
        _: &Ctx,
        _: &SessionBinding,
        _: &mut corral_core::inbox::ObserveCursor,
    ) -> anyhow::Result<Vec<Observation>> {
        Ok(vec![Observation::Uncertain {
            envelope: 1,
            reason: "claim expired".into(),
        }])
    }
}

#[test]
fn owner_list_shows_late_uncertainty_over_historical_admission() {
    // Given an admitted message whose immutable admission receipt must survive.
    let dir = tempfile::tempdir().unwrap();
    let ctx = Ctx::from_store(Store::init(dir.path()).unwrap());
    let inbox = Inbox::new(&ctx.store, Recipient::Pm).unwrap();
    let message = post(&inbox, "waiting on claim");
    assert_eq!(
        deliver(&ctx, &Recipient::Pm, &LateUncertainty, None).unwrap(),
        DeliveryStatus::Sent { envelope: 1 }
    );
    assert_eq!(ok(dir.path(), &[]).split('\t').nth(1), Some("admitted"));
    let admission = inbox.receipt(message.seq, ReceiptLevel::Admitted).unwrap();
    // When a real late observation changes the current unresolved envelope to uncertain.
    assert_eq!(
        deliver(&ctx, &Recipient::Pm, &LateUncertainty, None).unwrap(),
        DeliveryStatus::Uncertain
    );
    // Then the real text CLI surfaces recovery while JSON retains historical custody evidence.
    assert_eq!(ok(dir.path(), &[]).split('\t').nth(1), Some("uncertain"));
    let json: Value =
        serde_json::from_str(&ok(dir.path(), &["show", &message.id, "--json"])).unwrap();
    assert_eq!(json["envelope"]["outcome"]["status"], "uncertain");
    assert_eq!(
        inbox.receipt(message.seq, ReceiptLevel::Admitted).unwrap(),
        admission
    );
    assert!(json["admitted"].is_object());
}

#[test]
fn list_defaults_to_pending_including_admitted_messages() {
    // Given pending, admitted, observed and each terminal state in a temporary PM inbox.
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
    let pending = post(&inbox, "pending");
    let admitted = post(&inbox, "admitted");
    inbox
        .record_receipt(
            admitted.seq,
            ReceiptLevel::Admitted,
            &Receipt {
                envelope: None,
                binding: None,
                observed_at: jiff::Timestamp::now(),
                via: "fixture".into(),
                detail: json!({}),
            },
        )
        .unwrap();
    let observed = post(&inbox, "observed");
    inbox.mark_observed(observed.seq, "fixture").unwrap();
    for state in [
        TerminalState::Cancelled,
        TerminalState::Failed,
        TerminalState::Abandoned,
    ] {
        let message = post(&inbox, "terminal");
        inbox
            .mark_terminal(message.seq, state, "fixture", "manual")
            .unwrap();
    }

    // When listing without --all.
    let entries: Vec<Value> = serde_json::from_str(&ok(dir.path(), &["--json"])).unwrap();

    // Then only unread, nonterminal messages remain, in sequence order.
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["message"]["id"], pending.id);
    assert_eq!(entries[1]["message"]["id"], admitted.id);
    assert!(entries[1]["admitted"].is_object());
}

#[test]
fn all_includes_observed_and_terminal_messages_for_selected_recipient() {
    // Given settled messages in a temporary PM inbox.
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
    let observed = post(&inbox, "observed");
    inbox.mark_observed(observed.seq, "fixture").unwrap();
    let cancelled = post(&inbox, "cancelled");
    inbox
        .mark_terminal(cancelled.seq, TerminalState::Cancelled, "fixture", "manual")
        .unwrap();

    // When explicitly including all messages for pm.
    let entries: Vec<Value> =
        serde_json::from_str(&ok(dir.path(), &["--for", "pm", "--all", "--json"])).unwrap();

    // Then receipts and terminal state are included.
    assert_eq!(entries.len(), 2);
    assert!(entries[0]["observed"].is_object());
    assert_eq!(entries[1]["terminal"], "cancelled");
}

#[test]
fn show_json_includes_complete_body_and_metadata() {
    // Given a multiline message whose body is skipped by the core serde model.
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
    let message = post(&inbox, "first line\nsecond line");

    // When showing the message as JSON.
    let entry: Value =
        serde_json::from_str(&ok(dir.path(), &["show", &message.id, "--json"])).unwrap();

    // Then the full canonical body and identity are exposed.
    assert_eq!(entry["message"]["id"], message.id);
    assert_eq!(entry["message"]["body"], message.body);
    assert!(entry["observed"].is_null());
}

#[test]
fn show_text_includes_complete_body() {
    // Given a temporary PM message.
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
    let message = post(&inbox, "complete message\nwith a second line");

    // When showing without JSON.
    let output = ok(dir.path(), &["show", &message.id]);

    // Then stored content is not truncated.
    assert!(output.contains(&message.body));
}

#[test]
fn flush_only_persists_a_request_without_delivery() {
    // Given a temporary PM inbox with an unread message.
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
    let message = post(&inbox, "held info");

    // When requesting a flush with no herdr executable available.
    ok(dir.path(), &["flush", "pm"]);

    // Then only the durable request is written.
    assert!(inbox.flush_requested());
    assert!(inbox.envelopes().unwrap().is_empty());
    assert!(inbox.entry(message.seq).unwrap().observed.is_none());
}

#[test]
fn retry_reposts_failed_message_under_new_identity() {
    // Given a failed PM message.
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
    let message = post(&inbox, "retry this");
    inbox
        .mark_terminal(message.seq, TerminalState::Failed, "fixture", "fixture")
        .unwrap();

    // When retrying through the CLI.
    let id = ok(dir.path(), &["retry", &message.id]);

    // Then the new pending message has a new id and retains its content.
    let messages = inbox.messages().unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(id.trim(), messages[1].id);
    assert_ne!(messages[1].id, message.id);
    assert_eq!(
        messages[1].body.split_once('\n').unwrap().1,
        message.body.split_once('\n').unwrap().1
    );
    assert!(inbox.entry(messages[1].seq).unwrap().terminal.is_none());
}

#[test]
fn mark_records_manual_observation_and_cancellation_with_reason() {
    for action in ["--observed", "--cancel"] {
        // Given an unread PM message.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::init(dir.path()).unwrap();
        let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
        let message = post(&inbox, "mark this");

        // When recording an explicit owner decision.
        ok(
            dir.path(),
            &["mark", &message.id, action, "--reason", "owner evidence"],
        );

        // Then the chosen state and manual provenance persist.
        let entry = inbox.entry(message.seq).unwrap();
        let record = if action == "--observed" {
            let receipt = entry.observed.unwrap();
            assert_eq!(receipt.via, "manual");
            assert_eq!(receipt.detail["reason"], "owner evidence");
            inbox.record_path(message.seq, "receipt.observed")
        } else {
            assert_eq!(entry.terminal, Some(TerminalState::Cancelled));
            inbox.record_path(message.seq, "cancelled")
        };
        let value: Value = serde_json::from_str(&std::fs::read_to_string(record).unwrap()).unwrap();
        assert_eq!(value["via"], "manual");
    }
}

#[test]
fn recover_resend_or_abandon_records_owner_choice_without_delivery() {
    for action in ["--resend", "--abandon"] {
        // Given an uncertain open PM envelope.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::init(dir.path()).unwrap();
        let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
        let message = post(&inbox, "uncertain");
        open_envelope(&inbox);

        // When recording recovery.
        ok(dir.path(), &["recover", "pm", action]);

        // Then no new delivery is attempted, and the owner choice settles the envelope.
        assert!(inbox.state().unwrap().open_envelope.is_none());
        assert_eq!(inbox.envelopes().unwrap().len(), 1);
        assert_eq!(
            inbox.entry(message.seq).unwrap().terminal,
            if action == "--abandon" {
                Some(TerminalState::Abandoned)
            } else {
                None
            }
        );
    }
}

#[test]
fn terminal_recovery_flags_reach_core_without_enabling_an_adapter() {
    for action in ["--submit", "--discard"] {
        // Given an open PM envelope but no production adapter in phase 1.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::init(dir.path()).unwrap();
        let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
        post(&inbox, "draft");
        open_envelope(&inbox);

        // When requesting terminal recovery.
        let output = invoke(dir.path(), &["recover", "pm", action]);

        // Then parsing succeeds, the core rejects unsupported recovery, and state remains open.
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert!(inbox.state().unwrap().open_envelope.is_some());
        assert_eq!(inbox.envelopes().unwrap().len(), 1);
    }
}

#[test]
fn parser_rejects_invalid_ids_and_unsafe_recipient_keys() {
    for id in [
        "pm",
        "pm#",
        "pm#0",
        "pm#-1",
        "pm#+1",
        "pm#1#2",
        "pm#18446744073709551616",
        "../pm#1",
        "/tmp/pm#1",
        "pm/../../bad#1",
    ] {
        // Given an invalid id and a private empty root.
        let dir = tempfile::tempdir().unwrap();

        // When parsing a show command.
        let output = invoke(dir.path(), &["show", id]);

        // Then clap rejects it before any store access or writes.
        assert_eq!(output.status.code(), Some(2), "{id}: {output:?}");
        assert!(!dir.path().join(".corral").exists());
    }
    for args in [
        vec!["--for", "../pm"],
        vec!["flush", "../pm"],
        vec!["recover", "../pm", "--resend"],
    ] {
        let dir = tempfile::tempdir().unwrap();
        let output = invoke(dir.path(), &args);
        assert_eq!(output.status.code(), Some(2), "{args:?}: {output:?}");
    }
}

#[test]
fn parser_requires_exactly_one_recovery_or_mark_action_and_a_reason() {
    for args in [
        vec!["recover", "pm"],
        vec!["mark", "pm#1", "--reason", "evidence"],
        vec!["mark", "pm#1", "--observed"],
        vec!["mark", "pm#1", "--cancel"],
        vec![
            "mark",
            "pm#1",
            "--observed",
            "--cancel",
            "--reason",
            "evidence",
        ],
    ] {
        // Given missing or conflicting action arguments.
        let dir = tempfile::tempdir().unwrap();

        // When invoking the command.
        let output = invoke(dir.path(), &args);

        // Then the parser rejects it before opening the store.
        assert_eq!(output.status.code(), Some(2), "{args:?}: {output:?}");
    }
    let actions = ["--resend", "--abandon", "--submit", "--discard"];
    for (index, first) in actions.iter().enumerate() {
        for second in &actions[index + 1..] {
            let dir = tempfile::tempdir().unwrap();
            let output = invoke(dir.path(), &["recover", "pm", first, second]);
            assert_eq!(output.status.code(), Some(2), "{output:?}");
        }
    }
}

#[test]
fn blank_mark_reason_is_rejected_without_changing_state() {
    // Given an unread PM message.
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
    let message = post(&inbox, "pending");

    // When the owner supplies only whitespace as evidence.
    let output = invoke(
        dir.path(),
        &["mark", &message.id, "--cancel", "--reason", " \n "],
    );

    // Then cancellation fails without altering the message.
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let entry = inbox.entry(message.seq).unwrap();
    assert!(entry.observed.is_none());
    assert!(entry.terminal.is_none());
}
