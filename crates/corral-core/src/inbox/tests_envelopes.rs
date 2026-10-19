use std::cell::Cell;

use sha2::{Digest, Sha256};

use super::*;
use crate::{Run, Status};

#[test]
fn oversize_pm_posts_leave_sequences_files_and_events_unchanged() {
    // Given a pristine PM inbox and both adapter and global payload limits.
    let (_root, store) = fixture();
    let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
    let before = inbox.state().unwrap();
    let events = fs::read(store.dir().join("events.jsonl")).unwrap();
    for cap in [128, MAX_PAYLOAD + 1024] {
        let body = "\u{1f980}".repeat(cap.min(MAX_PAYLOAD) / 4 + 1);
        let mut request = post(&body, Urgency::Wake);
        request.max_payload = cap;

        // When a UTF-8 body alone exceeds the effective byte cap.
        let error = inbox.post(request).unwrap_err().to_string();

        // Then rejection names its byte size and has no persistent side effects.
        assert!(error.contains(&body.len().to_string()));
        assert!(error.contains(&cap.min(MAX_PAYLOAD).to_string()));
        assert_eq!(inbox.state().unwrap(), before);
        assert!(!inbox.dir().exists());
        assert!(!store.state_dir().join("pm-inbox.json").exists());
        assert_eq!(fs::read(store.dir().join("events.jsonl")).unwrap(), events);
    }
}

#[test]
fn oversize_task_post_never_invokes_admission_or_mutates_task() {
    // Given a real task recipient whose admission closure would change the task.
    let (_root, store) = fixture();
    let task = store
        .create_initialized("Inbox admission", None, Status::Idea, "scope", |task| {
            let run: Run = serde_json::from_value(json!({
                "id": "R1", "agent": "fixture", "worktree": "/fixture",
                "branch": "fixture", "base": "main", "base_sha": "abc",
                "started": "2026-10-19T00:00:00Z"
            }))?;
            task.runs.push(run);
            Ok(())
        })
        .unwrap();
    let inbox = Inbox::new(
        &store,
        Recipient::Run {
            task: task.id.clone(),
            run: "R1".into(),
        },
    )
    .unwrap();
    let before = fs::read(&task.path).unwrap();
    let events = fs::read(store.dir().join("events.jsonl")).unwrap();
    let state = inbox.state().unwrap();
    let called = Cell::new(false);
    let body = "x".repeat(MAX_PAYLOAD + 1);

    // When size validation rejects the post before caller admission.
    let result = inbox.post_with(post(&body, Urgency::Wake), |task, _| {
        called.set(true);
        task.title = "must not be saved".into();
        Ok(())
    });

    // Then the closure, task file, sequence, message files and event log are untouched.
    assert!(result.is_err());
    assert!(!called.get());
    assert_eq!(inbox.state().unwrap(), state);
    assert_eq!(fs::read(&task.path).unwrap(), before);
    assert!(!inbox.dir().exists());
    assert_eq!(fs::read(store.dir().join("events.jsonl")).unwrap(), events);
}

#[test]
fn header_overhead_is_included_before_reserving_a_sequence() {
    // Given a body below the adapter cap, but too large once headers are included.
    let (_root, store) = fixture();
    let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
    let body = "x".repeat(120);
    let mut request = post(&body, Urgency::Wake);
    request.max_payload = 128;

    // When admission computes the complete bounded payload size.
    let result = inbox.post(request);

    // Then no sequence is reserved for a message that cannot fit in an envelope.
    assert!(result.is_err());
    assert_eq!(inbox.state().unwrap(), InboxState::default());
    assert!(!inbox.dir().exists());
}

#[test]
fn held_info_joins_the_next_wake_in_sequence_order() {
    // Given informational messages and no wake.
    let (_root, store) = fixture();
    let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
    let info = inbox.post(post("held information", Urgency::Info)).unwrap();
    let before = inbox.state().unwrap();
    assert!(publish(&inbox, "original", false).is_none());
    assert_eq!(inbox.state().unwrap(), before);
    assert!(inbox.envelopes().unwrap().is_empty());
    let wake = inbox.post(post("act now", Urgency::Wake)).unwrap();

    // When the next wake is published.
    let envelope = publish(&inbox, "original", false).unwrap();

    // Then held info is included before the wake, with the complete canonical payload.
    assert_eq!(envelope.members, vec![info.seq, wake.seq]);
    assert_eq!(
        inbox.payload(&envelope).unwrap(),
        format!(
            "[corral envelope pm@env1]\n\n{}\n\n{}",
            info.body, wake.body
        )
    );
    assert_eq!(inbox.state().unwrap().open_envelope, Some(envelope.number));
}

#[test]
fn owner_flush_publishes_held_info_without_a_wake_message() {
    // Given info that ordinary delivery holds.
    let (_root, store) = fixture();
    let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
    let info = inbox.post(post("held information", Urgency::Info)).unwrap();
    assert!(!inbox.flush_requested());
    assert!(publish(&inbox, "original", false).is_none());

    // When the owner requests a flush and delivery consumes it.
    inbox.request_flush().unwrap();
    assert!(inbox.flush_requested());
    let envelope = publish(&inbox, "original", inbox.flush_requested()).unwrap();
    inbox.consume_flush().unwrap();
    inbox.consume_flush().unwrap();

    // Then the held message is frozen once and the durable flush request is cleared.
    assert_eq!(envelope.members, vec![info.seq]);
    assert!(!inbox.flush_requested());
    assert!(inbox.payload(&envelope).unwrap().ends_with(&info.body));
}

#[test]
fn question_bound_wake_is_never_packed_or_used_to_release_info() {
    // Given an unanswered native question reply and held info.
    let (_root, store) = fixture();
    let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
    let mut question = post("native answer", Urgency::Wake);
    question.binding.question_request = Some("request-1".into());
    let question = inbox.post(question).unwrap();
    let info = inbox.post(post("held", Urgency::Info)).unwrap();
    assert!(publish(&inbox, "original", false).is_none());
    let wake = inbox.post(post("ordinary wake", Urgency::Wake)).unwrap();

    // When ordinary delivery publishes unread messages.
    let envelope = publish(&inbox, "original", false).unwrap();

    // Then the native reply stays durable but cannot leak into the envelope.
    assert_eq!(envelope.members, vec![info.seq, wake.seq]);
    assert!(!inbox.payload(&envelope).unwrap().contains(&question.body));
    assert_eq!(inbox.message(question.seq).unwrap(), question);
    assert!(inbox.entry(question.seq).unwrap().observed.is_none());
}

#[test]
fn published_envelope_rejects_all_frozen_field_changes() {
    // Given one frozen publication and a later message that must wait.
    let (_root, store) = fixture();
    let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
    inbox.post(post("frozen", Urgency::Wake)).unwrap();
    let original = publish(&inbox, "original", false).unwrap();
    let payload = inbox.payload(&original).unwrap();
    let record = fs::read(inbox.envelope_path(original.number)).unwrap();
    inbox.post(post("later", Urgency::Wake)).unwrap();
    let before = inbox.state().unwrap();
    let mut mutations = Vec::new();
    let mut changed = original.clone();
    changed.members.push(2);
    mutations.push(changed);
    let mut changed = original.clone();
    changed.binding.session = "replacement".into();
    mutations.push(changed);
    let mut changed = original.clone();
    changed.binding.endpoint = None;
    mutations.push(changed);
    let mut changed = original.clone();
    changed.binding.transcript = None;
    mutations.push(changed);
    let mut changed = original.clone();
    changed.adapter = "other".into();
    mutations.push(changed);
    let mut changed = original.clone();
    changed.mode = "follow_up".into();
    mutations.push(changed);
    let mut changed = original.clone();
    changed.payload_sha256 = "forged".into();
    mutations.push(changed);
    let mut changed = original.clone();
    changed.payload_path = inbox.dir().join("other.payload");
    mutations.push(changed);
    let mut changed = original.clone();
    changed.idempotency_key = "pm@env999".into();
    mutations.push(changed);
    let mut changed = original.clone();
    changed.number = 999;
    mutations.push(changed);

    // When callers try to alter a frozen field or repack an open envelope.
    let _deliver = inbox.delivery_lock().unwrap();
    for changed in mutations {
        assert!(inbox.save_envelope(&changed).is_err(), "{changed:?}");
        assert_eq!(
            fs::read(inbox.envelope_path(original.number)).unwrap(),
            record
        );
    }
    assert!(
        inbox
            .publish(&binding("replacement"), "other", MAX_PAYLOAD, false)
            .is_err()
    );

    // Then every frozen byte, hash, identity and member remains original.
    assert_eq!(inbox.envelope(original.number).unwrap(), original);
    assert_eq!(inbox.payload(&original).unwrap(), payload);
    assert_eq!(
        original.payload_sha256,
        format!("{:x}", Sha256::digest(payload.as_bytes()))
    );
    assert_eq!(inbox.state().unwrap(), before);
}

#[test]
fn attempts_persist_without_changing_frozen_delivery_identity() {
    // Given an envelope whose first send has an uncertain outcome.
    let (_root, store) = fixture();
    let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
    inbox.post(post("send once", Urgency::Wake)).unwrap();
    let mut envelope = publish(&inbox, "original", false).unwrap();
    let original = envelope.clone();
    let payload = inbox.payload(&envelope).unwrap();
    let at = receipt("clock").observed_at;
    let _deliver = inbox.delivery_lock().unwrap();
    inbox.begin_attempt(&mut envelope, at).unwrap();
    assert_eq!(
        inbox.envelope(envelope.number).unwrap().attempts,
        vec![Attempt { at, outcome: None }]
    );
    inbox
        .finish_attempt(
            &mut envelope,
            SendOutcome::Uncertain {
                reason: "lost response".into(),
            },
        )
        .unwrap();

    // When an idempotent sender records a second attempt on the same envelope.
    inbox.begin_attempt(&mut envelope, at).unwrap();
    inbox
        .finish_attempt(
            &mut envelope,
            SendOutcome::Admitted {
                detail: json!({"delivery_id":"d1"}),
            },
        )
        .unwrap();

    // Then only the permitted journal fields have changed.
    assert_eq!(inbox.envelope(envelope.number).unwrap(), envelope);
    assert_eq!(envelope.attempts.len(), 2);
    assert!(matches!(
        envelope.attempts[0].outcome,
        Some(SendOutcome::Uncertain { .. })
    ));
    assert!(matches!(
        envelope.attempts[1].outcome,
        Some(SendOutcome::Admitted { .. })
    ));
    envelope.outcome = original.outcome.clone();
    envelope.stages = original.stages.clone();
    envelope.attempts = original.attempts.clone();
    assert_eq!(envelope, original);
    assert_eq!(inbox.payload(&envelope).unwrap(), payload);
}

#[test]
fn owner_resend_preserves_old_evidence_and_creates_new_envelope_identity() {
    // Given an uncertain attempt in the old session.
    let (root, store) = fixture();
    let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
    let message = inbox.post(post("uncertain", Urgency::Wake)).unwrap();
    let mut old = publish(&inbox, "original", false).unwrap();
    {
        let _deliver = inbox.delivery_lock().unwrap();
        inbox
            .begin_attempt(&mut old, receipt("clock").observed_at)
            .unwrap();
        inbox
            .finish_attempt(
                &mut old,
                SendOutcome::Uncertain {
                    reason: "disconnected".into(),
                },
            )
            .unwrap();
    }
    let old_bytes = fs::read(inbox.envelope_path(old.number)).unwrap();
    let old_payload = inbox.payload(&old).unwrap();

    // When the owner accepts a possible duplicate and binds the next publication elsewhere.
    inbox.recover(Recovery::Resend).unwrap();
    let reopened = Store::open(root.path()).unwrap();
    let inbox = Inbox::new(&reopened, Recipient::Pm).unwrap();
    inbox.recover_publication().unwrap();
    assert_eq!(inbox.state().unwrap().open_envelope, None);
    let new = publish(&inbox, "replacement", false).unwrap();

    // Then resend is a new frozen envelope; original uncertainty and payload survive.
    assert_eq!(new.number, old.number + 1);
    assert_eq!(new.members, vec![message.seq]);
    assert_eq!(new.binding, binding("replacement"));
    assert_ne!(new.idempotency_key, old.idempotency_key);
    assert_ne!(new.payload_sha256, old.payload_sha256);
    assert_eq!(inbox.envelope(old.number).unwrap(), old);
    assert_eq!(
        fs::read(inbox.envelope_path(old.number)).unwrap(),
        old_bytes
    );
    assert_eq!(inbox.payload(&old).unwrap(), old_payload);
    assert!(inbox.settled(&old).unwrap());
    assert!(
        inbox
            .receipt(message.seq, ReceiptLevel::Observed)
            .unwrap()
            .is_none()
    );
    assert_eq!(inbox.terminal(message.seq).unwrap(), None);
}

#[test]
fn owner_resend_rejects_unattempted_envelopes_without_settling_them() {
    // Given a publication that has never started a send.
    let (_root, store) = fixture();
    let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
    inbox.post(post("unattempted", Urgency::Wake)).unwrap();
    let envelope = publish(&inbox, "original", false).unwrap();
    let before = inbox.state().unwrap();

    // When the owner asks to recover it as an uncertain resend.
    assert!(inbox.recover(Recovery::Resend).is_err());

    // Then no recovery marker or state change is manufactured.
    assert_eq!(inbox.state().unwrap(), before);
    assert_eq!(inbox.envelope(envelope.number).unwrap(), envelope);
    assert!(!inbox.settled(&envelope).unwrap());
}

#[test]
fn publication_recovery_reattaches_original_session_and_members() {
    // Given a published record with its open pointer lost before persistence.
    let (root, store) = fixture();
    let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
    inbox.post(post("published", Urgency::Wake)).unwrap();
    let envelope = publish(&inbox, "original", false).unwrap();
    {
        let _deliver = inbox.delivery_lock().unwrap();
        let _lock = inbox.lock().unwrap();
        inbox
            .update_state(|state| {
                state.open_envelope = None;
                state.binding = None;
                Ok(())
            })
            .unwrap();
    }
    let reopened = Store::open(root.path()).unwrap();
    let inbox = Inbox::new(&reopened, Recipient::Pm).unwrap();

    // When recovery scans the durable publication, including a repeated recovery.
    {
        let _deliver = inbox.delivery_lock().unwrap();
        inbox.recover_publication().unwrap();
        inbox.recover_publication().unwrap();
    }

    // Then it reattaches the existing envelope without reserving another identity.
    let state = inbox.state().unwrap();
    assert_eq!(state.open_envelope, Some(envelope.number));
    assert_eq!(state.binding, Some(envelope.binding.clone()));
    assert_eq!(state.next_envelope, envelope.number + 1);
    assert_eq!(inbox.envelopes().unwrap(), vec![envelope]);
}

#[test]
fn unattempted_retirement_preserves_flush_across_another_interruption() {
    // Given an info-only publication interrupted after its flush was consumed.
    let (root, store) = fixture();
    let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
    let info = inbox.post(post("held information", Urgency::Info)).unwrap();
    inbox.request_flush().unwrap();
    let original = publish(&inbox, "original", true).unwrap();
    inbox.consume_flush().unwrap();

    // When rebinding retires it, but the process stops before publishing a replacement.
    {
        let _deliver = inbox.delivery_lock().unwrap();
        inbox.retire_unattempted(&original).unwrap();
    }
    let reopened = Store::open(root.path()).unwrap();
    let inbox = Inbox::new(&reopened, Recipient::Pm).unwrap();
    inbox.recover_publication().unwrap();

    // Then persisted eligibility survives and the old publication cannot be reattached.
    assert!(inbox.flush_requested());
    assert_eq!(inbox.state().unwrap().open_envelope, None);
    assert!(inbox.has_pending_delivery(inbox.flush_requested()).unwrap());
    let replacement = publish(&inbox, "replacement", inbox.flush_requested()).unwrap();
    assert_eq!(replacement.members, vec![info.seq]);
    assert_eq!(replacement.binding, binding("replacement"));
    assert_eq!(inbox.envelope(original.number).unwrap(), original);
}

#[test]
fn publication_recovery_keeps_cursor_only_for_the_same_binding() {
    for same_binding in [false, true] {
        // Given the publication crash boundary with a nondefault previous-session cursor.
        let (root, store) = fixture();
        let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
        inbox.post(post("published", Urgency::Wake)).unwrap();
        let envelope = publish(&inbox, "replacement", false).unwrap();
        let cursor = ObserveCursor {
            offset: 12345,
            last_complete_line: 77,
            coverage: Coverage::Truncated,
        };
        {
            let _deliver = inbox.delivery_lock().unwrap();
            let _lock = inbox.lock().unwrap();
            inbox
                .update_state(|state| {
                    state.open_envelope = None;
                    state.binding = Some(binding(if same_binding {
                        "replacement"
                    } else {
                        "original"
                    }));
                    state.observe_cursor = cursor.clone();
                    Ok(())
                })
                .unwrap();
        }
        let reopened = Store::open(root.path()).unwrap();
        let inbox = Inbox::new(&reopened, Recipient::Pm).unwrap();

        // When recovery attaches the durable envelope, including a repeated pass.
        {
            let _deliver = inbox.delivery_lock().unwrap();
            inbox.recover_publication().unwrap();
            inbox.recover_publication().unwrap();
        }

        // Then its cursor belongs to the recovered binding, never to another transcript.
        let state = inbox.state().unwrap();
        assert_eq!(state.binding, Some(envelope.binding.clone()));
        assert_eq!(state.open_envelope, Some(envelope.number));
        assert_eq!(
            state.observe_cursor,
            if same_binding {
                cursor
            } else {
                ObserveCursor::default()
            }
        );
        assert_eq!(inbox.envelope(envelope.number).unwrap(), envelope);
    }
}

#[test]
fn owner_abandon_preserves_observed_members_and_releases_the_open_envelope() {
    // Given a two-member envelope, only one of which was observed.
    let (_root, store) = fixture();
    let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
    let first = inbox.post(post("observed", Urgency::Wake)).unwrap();
    let second = inbox.post(post("pending", Urgency::Wake)).unwrap();
    let envelope = publish(&inbox, "original", false).unwrap();
    let evidence = receipt("transcript");
    inbox
        .record_receipt(first.seq, ReceiptLevel::Observed, &evidence)
        .unwrap();

    // When the owner abandons the remaining delivery.
    inbox.recover(Recovery::Abandon).unwrap();

    // Then observation wins, the other member is abandoned and nothing is resent.
    assert_eq!(
        inbox.receipt(first.seq, ReceiptLevel::Observed).unwrap(),
        Some(evidence)
    );
    assert_eq!(inbox.terminal(first.seq).unwrap(), None);
    assert_eq!(
        inbox.terminal(second.seq).unwrap(),
        Some(TerminalState::Abandoned)
    );
    assert_eq!(inbox.state().unwrap().open_envelope, None);
    assert!(inbox.settled(&envelope).unwrap());
    assert!(publish(&inbox, "replacement", false).is_none());
}

#[test]
fn owner_retry_reposts_failed_message_with_new_identity_and_original_metadata() {
    // Given a failed message with subject and caller binding metadata.
    let (_root, store) = fixture();
    let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
    let mut request = post("keep this exact body\nand line", Urgency::Info);
    request.binding.run = Some("R1".into());
    request.binding.decision = Some("D1".into());
    request.binding.review_round = Some(3);
    request.subject = Some(json!({"task":"T-001"}));
    let original = inbox.post(request).unwrap();
    inbox
        .mark_terminal(original.seq, TerminalState::Failed, "rejected", "fixture")
        .unwrap();

    // When the owner retries after fixing the rejection.
    let retry = inbox.retry(original.seq).unwrap();

    // Then a new durable message preserves the content, not the failed identity.
    assert_eq!(retry.seq, original.seq + 1);
    assert_ne!(retry.id, original.id);
    assert_eq!(retry.kind, original.kind);
    assert_eq!(retry.urgency, original.urgency);
    assert_eq!(retry.author, original.author);
    assert_eq!(retry.binding, original.binding);
    assert_eq!(retry.subject, original.subject);
    assert_eq!(
        retry.body.split_once('\n').unwrap().1,
        original.body.split_once('\n').unwrap().1
    );
    assert_eq!(inbox.message(retry.seq).unwrap(), retry);
    assert_eq!(
        inbox.terminal(original.seq).unwrap(),
        Some(TerminalState::Failed)
    );
    assert_eq!(inbox.terminal(retry.seq).unwrap(), None);
}

#[test]
fn owner_retry_refuses_pending_cancelled_abandoned_and_observed_messages() {
    // Given messages that are not failed.
    let (_root, store) = fixture();
    let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
    let pending = inbox.post(post("pending", Urgency::Wake)).unwrap();
    let cancelled = inbox.post(post("cancelled", Urgency::Wake)).unwrap();
    let abandoned = inbox.post(post("abandoned", Urgency::Wake)).unwrap();
    let observed = inbox.post(post("observed", Urgency::Wake)).unwrap();
    inbox
        .mark_terminal(cancelled.seq, TerminalState::Cancelled, "cancel", "test")
        .unwrap();
    inbox
        .mark_terminal(abandoned.seq, TerminalState::Abandoned, "abandon", "test")
        .unwrap();
    inbox
        .record_receipt(observed.seq, ReceiptLevel::Observed, &receipt("transcript"))
        .unwrap();
    let before = inbox.state().unwrap();
    let messages = inbox.messages().unwrap();

    // When retries are requested for each non-failure state.
    for message in [pending, cancelled, abandoned, observed] {
        assert!(inbox.retry(message.seq).is_err());
    }

    // Then no new sequence or message was created.
    assert_eq!(inbox.state().unwrap(), before);
    assert_eq!(inbox.messages().unwrap(), messages);
}

#[test]
fn effect_journal_reuses_exact_observed_receipt_across_reopen_and_replay() {
    // Given a started effect journal with original recipient-side evidence.
    let (root, store) = fixture();
    let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
    let message = inbox.post(post("apply once", Urgency::Wake)).unwrap();
    let evidence = receipt("transcript");
    inbox
        .record_receipt(message.seq, ReceiptLevel::Observed, &evidence)
        .unwrap();
    let started = inbox.start_effect(message.seq, false, None).unwrap();
    assert_eq!(started.status, EffectStatus::Started);
    assert_eq!(started.receipt, evidence);
    assert!(inbox.state().unwrap().effects_applied.is_empty());
    let reopened = Store::open(root.path()).unwrap();
    let inbox = Inbox::new(&reopened, Recipient::Pm).unwrap();

    // When crash replay finds the journal, even with replacement receipt input.
    assert!(
        !inbox
            .record_receipt(message.seq, ReceiptLevel::Observed, &receipt("replacement"))
            .unwrap()
    );
    assert_eq!(
        inbox.start_effect(message.seq, false, None).unwrap(),
        started
    );
    inbox.finish_effect(message.seq).unwrap();
    let applied = inbox.entry(message.seq).unwrap().effect.unwrap();
    let bytes = fs::read(inbox.record_path(message.seq, "effect")).unwrap();
    inbox.finish_effect(message.seq).unwrap();

    // Then application is journaled once using exactly the original receipt.
    assert_eq!(applied.status, EffectStatus::Applied);
    assert_eq!(applied.receipt, evidence);
    assert_eq!(
        inbox.start_effect(message.seq, false, None).unwrap(),
        applied
    );
    assert_eq!(
        inbox.state().unwrap().effects_applied,
        BTreeSet::from([message.seq])
    );
    assert_eq!(
        fs::read(inbox.record_path(message.seq, "effect")).unwrap(),
        bytes
    );
}

#[test]
fn effects_require_observation_and_stale_effects_never_become_applied() {
    // Given admission evidence without an observed receipt.
    let (_root, store) = fixture();
    let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
    let message = inbox.post(post("stale", Urgency::Wake)).unwrap();
    inbox
        .record_receipt(message.seq, ReceiptLevel::Admitted, &receipt("custody"))
        .unwrap();
    assert!(inbox.start_effect(message.seq, false, None).is_err());
    assert!(!inbox.record_path(message.seq, "effect").exists());
    let evidence = receipt("transcript");
    inbox
        .record_receipt(message.seq, ReceiptLevel::Observed, &evidence)
        .unwrap();

    // When a stale observed message is reconciled and replayed.
    let skipped = inbox.start_effect(message.seq, true, None).unwrap();
    inbox.finish_effect(message.seq).unwrap();

    // Then it retains its original observation but cannot be applied on a later replay.
    assert_eq!(skipped.status, EffectStatus::SkippedStale);
    assert_eq!(skipped.receipt, evidence);
    assert_eq!(
        inbox.start_effect(message.seq, false, None).unwrap(),
        skipped
    );
    assert_eq!(inbox.entry(message.seq).unwrap().effect, Some(skipped));
    assert!(inbox.state().unwrap().effects_applied.is_empty());
}

#[test]
fn started_effect_that_becomes_stale_keeps_receipt_and_never_applies() {
    // Given an effect interrupted before application, whose binding later became stale.
    let (root, store) = fixture();
    let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
    let message = inbox
        .post(post("superseded during effect", Urgency::Wake))
        .unwrap();
    let evidence = receipt("original transcript");
    inbox
        .record_receipt(message.seq, ReceiptLevel::Observed, &evidence)
        .unwrap();
    assert_eq!(
        inbox.start_effect(message.seq, false, None).unwrap().status,
        EffectStatus::Started
    );
    let reopened = Store::open(root.path()).unwrap();
    let inbox = Inbox::new(&reopened, Recipient::Pm).unwrap();

    // When crash recovery detects the stale binding before applying the effect.
    let skipped = inbox.start_effect(message.seq, true, None).unwrap();
    inbox.finish_effect(message.seq).unwrap();

    // Then the original evidence survives, but neither this nor later replay applies it.
    assert_eq!(skipped.status, EffectStatus::SkippedStale);
    assert_eq!(skipped.receipt, evidence);
    assert_eq!(
        inbox.entry(message.seq).unwrap().effect,
        Some(skipped.clone())
    );
    assert_eq!(
        inbox.start_effect(message.seq, false, None).unwrap(),
        skipped
    );
    assert!(inbox.state().unwrap().effects_applied.is_empty());
}

#[test]
fn decision_effect_journal_freezes_exact_answer_and_loads_old_records() {
    let (root, store) = fixture();
    let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
    let message = inbox.post(post("answer", Urgency::Wake)).unwrap();
    let observed = receipt("transcript");
    inbox
        .record_receipt(message.seq, ReceiptLevel::Observed, &observed)
        .unwrap();
    let answer = crate::ResumeReceipt {
        version: 1,
        id: "D1".into(),
        run: "R1".into(),
        outcome: crate::ReceiptOutcome::Answered,
        delivery: crate::ResumeDelivery::Acknowledged,
        observed_at: observed.observed_at,
        answer: Some("yes".into()),
        data: json!({"inbox_message":message.id}),
    };
    let effect = inbox
        .start_effect(message.seq, false, Some(answer.clone()))
        .unwrap();
    let reopened = Store::open(root.path()).unwrap();
    let inbox = Inbox::new(&reopened, Recipient::Pm).unwrap();
    let mut replacement = answer.clone();
    replacement.answer = Some("no".into());
    assert_eq!(
        inbox
            .start_effect(message.seq, false, Some(replacement))
            .unwrap(),
        effect
    );
    assert_eq!(effect.decision_receipt, Some(answer));
    let old: Effect =
        serde_json::from_value(json!({"status":"started","receipt":observed})).unwrap();
    assert_eq!(old.decision_receipt, None);
}
