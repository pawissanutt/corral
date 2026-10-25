//! Design 4 retry eligibility over a message's complete envelope and attempt evidence.
use std::fs;

use serde_json::json;

use super::*;
use crate::Store;

fn fixture() -> (tempfile::TempDir, Store) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    (root, store)
}

fn post<'a>(inbox: &'a Inbox<'a>, body: &str) -> Message {
    inbox
        .post(Post {
            kind: "owner_reply",
            urgency: Urgency::Wake,
            author: "owner",
            body,
            binding: MessageBinding::default(),
            subject: None,
            max_payload: MAX_PAYLOAD,
        })
        .unwrap()
}

fn rejected(retryable_after_fix: bool) -> Option<SendOutcome> {
    Some(SendOutcome::Rejected {
        reason: "fixture rejection".into(),
        retryable_after_fix,
    })
}

/// Publishes one envelope for every unread message and records `attempts` on it, the last
/// outcome becoming the envelope's; `None` leaves that attempt unresolved.
fn attempts(inbox: &Inbox<'_>, attempts: &[Option<SendOutcome>]) -> Envelope {
    let _deliver = inbox.delivery_lock().unwrap();
    let binding = SessionBinding {
        session: "fixture".into(),
        endpoint: None,
        transcript: None,
    };
    let mut envelope = inbox
        .publish(&binding, "fixture", MAX_PAYLOAD, true)
        .unwrap()
        .unwrap();
    for outcome in attempts {
        inbox
            .begin_attempt(&mut envelope, Timestamp::now())
            .unwrap();
        if let Some(outcome) = outcome {
            inbox
                .finish_attempt(&mut envelope, outcome.clone())
                .unwrap();
        }
    }
    envelope
}

/// The delivery pass's `fail`: every member becomes Failed and the envelope closes.
fn fail(inbox: &Inbox<'_>, envelope: &Envelope) {
    for seq in &envelope.members {
        inbox
            .mark_terminal(*seq, TerminalState::Failed, "rejected", "fixture")
            .unwrap();
    }
    inbox.close_settled(envelope).unwrap();
}

fn events(store: &Store, kind: &str) -> usize {
    fs::read_to_string(store.dir().join("events.jsonl"))
        .unwrap()
        .lines()
        .filter(|line| line.contains(&format!("\"kind\":\"{kind}\"")))
        .count()
}

/// Refuses with `code` and changes nothing: no sequence, message or retried event.
fn refuses(inbox: &Inbox<'_>, seq: u64, code: &str) {
    let state = inbox.state().unwrap();
    let messages = inbox.messages().unwrap();
    let error = format!("{:#}", inbox.retry(seq).unwrap_err());
    assert!(
        error.starts_with(&format!("retry_refused: {code}:")),
        "{code}: {error}"
    );
    assert!(!error.contains("inbox recover"), "{error}");
    assert_eq!(inbox.state().unwrap(), state);
    assert_eq!(inbox.messages().unwrap(), messages);
    assert_eq!(events(inbox.store, "message_retried"), 0);
}

fn succeeds(inbox: &Inbox<'_>, original: &Message) {
    let retried = inbox.retry(original.seq).unwrap();
    assert_ne!(retried.id, original.id);
    assert_eq!(retried.recipient, original.recipient);
    assert_eq!(retried.kind, original.kind);
    assert_eq!(retried.binding, original.binding);
    assert_eq!(
        retried.body.split_once('\n').unwrap().1,
        original.body.split_once('\n').unwrap().1
    );
    assert_eq!(inbox.terminal(retried.seq).unwrap(), None);
    let log = fs::read_to_string(inbox.store.dir().join("events.jsonl")).unwrap();
    let event: Value = log
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .rfind(|event| event["kind"] == "message_retried")
        .unwrap();
    assert_eq!(event["detail"]["from"], original.id.as_str());
    assert_eq!(event["detail"]["to"], retried.id.as_str());
}

#[test]
fn never_attempted_terminal_messages_are_retried() {
    for state in [
        TerminalState::Failed,
        TerminalState::Cancelled,
        TerminalState::Abandoned,
    ] {
        // Given a terminal message that no envelope ever attempted, one in an open publication.
        let (_root, store) = fixture();
        let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
        let message = post(&inbox, "never sent");
        attempts(&inbox, &[]);
        inbox
            .mark_terminal(message.seq, state, "fixture", "test")
            .unwrap();

        // When and then the owner retries it with a new identity.
        succeeds(&inbox, &message);
    }
}

#[test]
fn a_definitive_retryable_rejection_is_retried_and_a_final_one_is_not() {
    for retryable in [true, false] {
        // Given begin_attempt -> Rejected -> finish_attempt -> fail.
        let (_root, store) = fixture();
        let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
        let message = post(&inbox, "rejected");
        let envelope = attempts(&inbox, &[rejected(retryable)]);
        fail(&inbox, &envelope);

        // When retrying, then the nonempty attempt list alone is not uncertainty.
        if retryable {
            succeeds(&inbox, &message);
        } else {
            refuses(&inbox, message.seq, "not_retryable");
        }
    }
}

#[test]
fn any_earlier_unresolved_or_positive_attempt_bars_a_later_rejection() {
    let earlier = [
        None,
        Some(SendOutcome::Uncertain {
            reason: "timeout".into(),
        }),
        Some(SendOutcome::Conflicted {
            reason: "key bound".into(),
        }),
        Some(SendOutcome::Accepted),
        Some(SendOutcome::Admitted { detail: json!({}) }),
        Some(SendOutcome::Held {
            reason: "inbound".into(),
        }),
    ];
    for outcome in earlier {
        // Given an earlier ambiguous attempt followed by a retryable rejection, then Failed.
        let (_root, store) = fixture();
        let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
        let message = post(&inbox, "maybe delivered");
        let envelope = attempts(&inbox, &[outcome, rejected(true)]);
        fail(&inbox, &envelope);

        // When retrying, then the terminal label never makes the history safe.
        refuses(&inbox, message.seq, "uncertain_prior_attempt");
    }
}

#[test]
fn an_owner_resend_after_uncertainty_never_makes_a_later_rejection_safe() {
    // Given E1 uncertain, owner resend, then E2 retryable rejection -> Failed.
    let (_root, store) = fixture();
    let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
    let message = post(&inbox, "maybe delivered");
    attempts(
        &inbox,
        &[Some(SendOutcome::Uncertain {
            reason: "timeout".into(),
        })],
    );
    inbox.recover(Recovery::Resend).unwrap();
    let second = attempts(&inbox, &[rejected(true)]);
    assert_eq!(second.number, 2);
    fail(&inbox, &second);

    // When retrying, then E1's uncertainty still refuses.
    refuses(&inbox, message.seq, "uncertain_prior_attempt");
}

#[test]
fn attempted_cancelled_or_abandoned_messages_are_not_widened() {
    for (state, code) in [
        (TerminalState::Cancelled, "attempted_cancelled"),
        (TerminalState::Abandoned, "attempted_abandoned"),
    ] {
        // Given a definitively rejected attempt whose message was then cancelled or abandoned.
        let (_root, store) = fixture();
        let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
        let message = post(&inbox, "attempted");
        attempts(&inbox, &[rejected(true)]);
        inbox
            .mark_terminal(message.seq, state, "fixture", "test")
            .unwrap();

        // When retrying, then the Failed-only rejection exception does not generalize.
        refuses(&inbox, message.seq, code);
    }
}

#[test]
fn pending_observed_admitted_question_bound_and_unknown_evidence_refuse() {
    // Given one message per refusing state.
    let (_root, store) = fixture();
    let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
    let pending = post(&inbox, "pending");
    refuses(&inbox, pending.seq, "not_terminal");
    let receipt = Receipt {
        envelope: None,
        binding: None,
        observed_at: Timestamp::now(),
        via: "fixture".into(),
        detail: json!({}),
    };
    inbox
        .mark_terminal(pending.seq, TerminalState::Failed, "fixture", "test")
        .unwrap();
    inbox
        .record_receipt(pending.seq, ReceiptLevel::Admitted, &receipt)
        .unwrap();
    refuses(&inbox, pending.seq, "admitted");
    inbox
        .record_receipt(pending.seq, ReceiptLevel::Observed, &receipt)
        .unwrap();
    refuses(&inbox, pending.seq, "observed");

    let native = inbox
        .post(Post {
            kind: "question_answer",
            urgency: Urgency::Wake,
            author: "owner",
            body: "native answer",
            binding: MessageBinding {
                question_request: Some("q1".into()),
                ..Default::default()
            },
            subject: None,
            max_payload: MAX_PAYLOAD,
        })
        .unwrap();
    inbox
        .mark_terminal(native.seq, TerminalState::Cancelled, "fixture", "test")
        .unwrap();
    refuses(&inbox, native.seq, "alternate_route");

    // A missing envelope record leaves membership unknown, never safe.
    let lost = post(&inbox, "lost history");
    let envelope = attempts(&inbox, &[rejected(true)]);
    fail(&inbox, &envelope);
    fs::remove_file(inbox.envelope_path(envelope.number)).unwrap();
    refuses(&inbox, lost.seq, "evidence_incomplete");
}

#[test]
fn unavailable_terminal_recovery_refuses_before_any_repair_or_write() {
    for recovery in [Recovery::Submit, Recovery::Discard] {
        // Given an open attempted envelope.
        let (_root, store) = fixture();
        let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
        post(&inbox, "draft");
        attempts(
            &inbox,
            &[Some(SendOutcome::Uncertain {
                reason: "timeout".into(),
            })],
        );
        let state = inbox.state().unwrap();
        let log = fs::read(store.dir().join("events.jsonl")).unwrap();

        // When requesting the uninstalled terminal adapter's recovery.
        let error = inbox.recover(recovery).unwrap_err().to_string();

        // Then it names the missing capability and changes nothing.
        assert!(
            error.starts_with("terminal_adapter_unavailable:"),
            "{error}"
        );
        assert_eq!(inbox.state().unwrap(), state);
        assert_eq!(fs::read(store.dir().join("events.jsonl")).unwrap(), log);
        assert!(!inbox.dir().join("envelopes/000001.settled.json").exists());
    }
}

fn publication_fixture(task_recipient: bool) -> (tempfile::TempDir, Store, Recipient) {
    let (root, store) = fixture();
    let recipient = if task_recipient {
        let task = store
            .create_initialized(
                "retry publication",
                None,
                crate::Status::Running,
                "",
                |task| {
                    task.runs.push(serde_json::from_value(json!({
                        "id":"R1","agent":"fixture","worktree":root.path(),
                        "branch":"fixture","base":"main","base_sha":"0",
                        "started":"2026-10-19T00:00:00Z"
                    }))?);
                    Ok(())
                },
            )
            .unwrap();
        Recipient::Run {
            task: task.id,
            run: "R1".into(),
        }
    } else {
        Recipient::Pm
    };
    (root, store, recipient)
}

#[test]
fn retry_retains_exact_replacement_when_publication_announcement_fails() {
    for task_recipient in [false, true] {
        // Given a never-attempted terminal message and a deterministically broken event sink.
        let (_root, store, recipient) = publication_fixture(task_recipient);
        let inbox = Inbox::new(&store, recipient).unwrap();
        let original = post(&inbox, "retry once");
        inbox
            .mark_terminal(original.seq, TerminalState::Failed, "fixture", "test")
            .unwrap();
        let sink = store.dir().join("events.jsonl");
        fs::remove_file(&sink).unwrap();
        fs::create_dir(&sink).unwrap();

        // When the replacement commits but its announcement fails, then retain its claim.
        assert!(inbox.retry(original.seq).is_err());
        let replacement = inbox.message(2).unwrap();
        assert_eq!(replacement.id, format!("{}#2", inbox.key()));
        assert_eq!(replacement.body.split_once('\n').unwrap().1, "retry once");
        assert_eq!(inbox.messages().unwrap().len(), 2);
        assert_eq!(inbox.state().unwrap().next_seq, 3);
        assert_eq!(inbox.terminal(2).unwrap(), None);
        let claim: Value =
            super::storage::read_optional(&inbox.record_path(original.seq, "retried"))
                .unwrap()
                .expect("retry claim must survive publication failure");
        assert_eq!(claim["from"], original.id);
        assert_eq!(claim["to"], replacement.id);

        // Restoring announcements must not permit a second pending replacement.
        fs::remove_dir(&sink).unwrap();
        fs::write(&sink, "").unwrap();
        refuses(&inbox, original.seq, "already_retried");
        assert_eq!(inbox.messages().unwrap(), vec![original, replacement]);
    }
}

#[test]
fn retry_releases_claim_after_definitive_admission_refusal() {
    // Given a task message eligible for retry before the live admission callback refuses.
    let (_root, store, recipient) = publication_fixture(true);
    let inbox = Inbox::new(&store, recipient).unwrap();
    let original = post(&inbox, "not yet published");
    inbox
        .mark_terminal(original.seq, TerminalState::Failed, "fixture", "test")
        .unwrap();
    let state = inbox.state().unwrap();
    let log = fs::read(store.dir().join("events.jsonl")).unwrap();

    // When admission definitively refuses before publication.
    let error = inbox
        .retry_with(original.seq, |_| anyhow::bail!("fixture admission refusal"))
        .unwrap_err();

    // Then no delivery evidence is lost, and a later corrected admission can retry.
    assert_eq!(error.to_string(), "fixture admission refusal");
    assert!(!inbox.record_path(original.seq, "retried").exists());
    assert_eq!(inbox.state().unwrap(), state);
    assert_eq!(inbox.messages().unwrap(), vec![original.clone()]);
    assert_eq!(fs::read(store.dir().join("events.jsonl")).unwrap(), log);
    succeeds(&inbox, &original);
}

#[test]
fn retry_retains_identity_when_reservation_persistence_fails() {
    for task_recipient in [false, true] {
        // Given a terminal message and a state writer that will fail after reservation starts.
        let (_root, store, recipient) = publication_fixture(task_recipient);
        let inbox = Inbox::new(&store, recipient.clone()).unwrap();
        let original = post(&inbox, "uncertain reservation");
        inbox
            .mark_terminal(original.seq, TerminalState::Failed, "fixture", "test")
            .unwrap();
        let state_path = match recipient.task() {
            Some(task) => store.get(task).unwrap().path,
            None => store.state_dir().join("pm-inbox.json"),
        };
        let mut temporary = state_path.as_os_str().to_owned();
        temporary.push(".tmp");
        let temporary = std::path::PathBuf::from(temporary);
        fs::create_dir(&temporary).unwrap();

        // When persistence fails, then the conservative claim retains the reserved identity.
        assert!(inbox.retry(original.seq).is_err());
        let claim: Value =
            super::storage::read_optional(&inbox.record_path(original.seq, "retried"))
                .unwrap()
                .unwrap();
        assert_eq!(claim["from"], original.id);
        assert_eq!(claim["to"], format!("{}#2", inbox.key()));
        assert_eq!(inbox.messages().unwrap(), vec![original.clone()]);
        assert_eq!(inbox.state().unwrap().next_seq, 2);
        fs::remove_dir(&temporary).unwrap();
        refuses(&inbox, original.seq, "already_retried");
    }
}

#[test]
fn retry_retains_identity_when_message_body_cannot_be_written() {
    for task_recipient in [false, true] {
        // Given an eligible original and a directory blocking the replacement body.
        let (_root, store, recipient) = publication_fixture(task_recipient);
        let inbox = Inbox::new(&store, recipient).unwrap();
        let original = post(&inbox, "reserved gap");
        inbox
            .mark_terminal(original.seq, TerminalState::Failed, "fixture", "test")
            .unwrap();
        fs::create_dir(inbox.message_path(2)).unwrap();

        // When reservation commits but writing its body fails, then keep identity and gap evidence.
        assert!(inbox.retry(original.seq).is_err());
        let claim: Value =
            super::storage::read_optional(&inbox.record_path(original.seq, "retried"))
                .unwrap()
                .unwrap();
        assert_eq!(claim["from"], original.id);
        assert_eq!(claim["to"], format!("{}#2", inbox.key()));
        assert_eq!(inbox.state().unwrap().next_seq, 3);
        assert_eq!(events(&store, "message_gap"), 1);
        fs::remove_dir(inbox.message_path(2)).unwrap();
        assert_eq!(inbox.messages().unwrap(), vec![original.clone()]);
        refuses(&inbox, original.seq, "already_retried");
    }
}

#[test]
fn observed_members_stay_settled_when_an_unattempted_publication_is_replaced() {
    // Given one observed member and one unsent survivor in an unattempted publication.
    let (_root, store) = fixture();
    let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
    let observed = post(&inbox, "already observed");
    let survivor = post(&inbox, "still unsent");
    let envelope = attempts(&inbox, &[]);
    inbox
        .record_receipt(
            observed.seq,
            ReceiptLevel::Observed,
            &Receipt {
                envelope: Some(envelope.number),
                binding: Some(envelope.binding.clone()),
                observed_at: "2026-10-25T00:00:00Z".parse().unwrap(),
                via: "manual".into(),
                detail: json!({}),
            },
        )
        .unwrap();
    // When delivery retires the unsent publication before repacking.
    inbox.retire_unattempted(&envelope).unwrap();
    let replacement = attempts(&inbox, &[]);
    // Then only the survivor is republished and original evidence is unchanged.
    assert_eq!(replacement.members, vec![survivor.seq]);
    assert_eq!(inbox.envelope(envelope.number).unwrap(), envelope);
    assert!(
        inbox
            .receipt(observed.seq, ReceiptLevel::Observed)
            .unwrap()
            .is_some()
    );
    assert!(!inbox.inspect_retry(observed.seq).unwrap().eligible);
}

#[test]
fn inspection_never_offers_another_retry_after_a_retained_publication_claim() {
    // Given a retry whose reservation identity committed but body publication failed.
    let (_root, store, recipient) = publication_fixture(true);
    let inbox = Inbox::new(&store, recipient).unwrap();
    let original = post(&inbox, "reserved replacement");
    inbox
        .mark_terminal(original.seq, TerminalState::Failed, "fixture", "test")
        .unwrap();
    fs::create_dir(inbox.message_path(2)).unwrap();
    assert!(inbox.retry(original.seq).is_err());
    let state = inbox.state().unwrap();
    // When explicit inspection examines that original and its missing replacement.
    let original_inspection = inbox.inspect_retry(original.seq).unwrap();
    let replacement_inspection = inbox.inspect_retry(2).unwrap();
    // Then the original cannot be replayed and its replacement remains identified but incomplete.
    assert!(!original_inspection.eligible);
    assert_eq!(original_inspection.reason_code, "already_retried");
    assert!(!replacement_inspection.eligible);
    assert!(!replacement_inspection.evidence_complete);
    assert_eq!(inbox.state().unwrap(), state);
}
