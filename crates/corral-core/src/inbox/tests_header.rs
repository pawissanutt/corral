//! Owner-header admission, bounded reads and retry-history regressions.
use super::*;
use crate::{Run, Status, Store};

fn fixture() -> (tempfile::TempDir, Store, Recipient) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    let task = store.create("Header", None, Status::Running, "").unwrap();
    store
        .update(&task.id, |task| {
            task.runs
                .push(serde_json::from_value::<Run>(serde_json::json!({
                    "id":"R1", "agent":"fixture", "worktree":"/fixture", "branch":"fixture",
                    "base":"main", "base_sha":"abc", "started":"2026-10-25T00:00:00Z"
                }))?);
            Ok(())
        })
        .unwrap();
    (
        root,
        store,
        Recipient::Run {
            task: task.id,
            run: "R1".into(),
        },
    )
}

fn post() -> Post<'static> {
    Post {
        kind: "owner_note",
        urgency: Urgency::Wake,
        author: "owner",
        body: "act",
        binding: MessageBinding::default(),
        subject: None,
        max_payload: MAX_PAYLOAD,
    }
}

#[test]
fn seventeenth_post_refuses_before_admission_and_all_identities_survive_restart() {
    // Given a full 8+8 retained owner header.
    let (root, store, recipient) = fixture();
    let inbox = Inbox::new(&store, recipient.clone()).unwrap();
    for _ in 0..16 {
        inbox.post(post()).unwrap();
    }
    let before = inbox.state().unwrap();
    let header = before.owner_header.as_ref().unwrap();
    assert_eq!(
        header.entries.iter().map(|e| e.seq).collect::<Vec<_>>(),
        (1..=8).collect::<Vec<_>>()
    );
    assert_eq!(
        header
            .overflow_entries
            .iter()
            .map(|e| e.seq)
            .collect::<Vec<_>>(),
        (9..=16).collect::<Vec<_>>()
    );
    let called = std::cell::Cell::new(false);
    // When the seventeenth publication tries to mutate domain state.
    let error = inbox
        .post_with(post(), |task, _| {
            called.set(true);
            task.title = "incorrect".into();
            Ok(())
        })
        .unwrap_err();
    // Then backpressure precedes sequence, file and domain effects.
    assert!(error.to_string().contains("owner_header_full"), "{error:#}");
    assert!(!called.get());
    assert_eq!(inbox.state().unwrap(), before);
    assert!(!inbox.message_path(17).exists());
    let reopened = Store::open(root.path()).unwrap();
    assert_eq!(
        Inbox::new(&reopened, recipient).unwrap().state().unwrap(),
        before
    );
}

#[test]
fn owner_reads_use_five_keyed_probes_and_no_envelope_or_body_reads() {
    // Given two retained messages.
    let (_root, store, recipient) = fixture();
    let inbox = Inbox::new(&store, recipient).unwrap();
    inbox.post(post()).unwrap();
    inbox.post(post()).unwrap();
    let state = inbox.state().unwrap();
    use crate::work::{Work, count};
    let counters = [
        Work::InboxRecordRead,
        Work::InboxMessageRead,
        Work::InboxEnvelopeRead,
        Work::InboxList,
    ];
    let before = counters.map(count);
    // When reading the fixed header.
    assert_eq!(inbox.owner_entries(&state).unwrap().len(), 2);
    // Then missing paths count too, without historical fallback.
    assert_eq!(
        counters
            .map(count)
            .into_iter()
            .zip(before)
            .map(|(a, b)| a - b)
            .collect::<Vec<_>>(),
        vec![10, 0, 0, 0]
    );
}

fn receipt() -> Receipt {
    Receipt {
        envelope: None,
        binding: None,
        observed_at: "2026-10-25T00:00:00Z".parse().unwrap(),
        via: "fixture".into(),
        detail: serde_json::json!({}),
    }
}

fn publish(inbox: &Inbox<'_>) -> Envelope {
    inbox
        .publish(
            &SessionBinding {
                session: "fixture".into(),
                endpoint: None,
                transcript: None,
            },
            "fixture",
            MAX_PAYLOAD,
            true,
        )
        .unwrap()
        .unwrap()
}

fn reject(inbox: &Inbox<'_>, envelope: &mut Envelope) {
    inbox
        .begin_attempt(envelope, receipt().observed_at)
        .unwrap();
    inbox
        .finish_attempt(
            envelope,
            SendOutcome::Rejected {
                reason: "fixture".into(),
                retryable_after_fix: true,
            },
        )
        .unwrap();
}

#[test]
fn reserve_in_refuses_capacity_before_file_or_local_state_changes() {
    // Given sixteen committed reservations through the transaction producer.
    let (_root, store, recipient) = fixture();
    let inbox = Inbox::new(&store, recipient.clone()).unwrap();
    for _ in 0..16 {
        let _lock = inbox.lock().unwrap();
        store
            .update(recipient.task().unwrap(), |task| {
                inbox.reserve_in(task, post())?;
                Ok(())
            })
            .unwrap();
    }
    let before = inbox.state().unwrap();
    let mut task = store.get(recipient.task().unwrap()).unwrap();
    // When an additional reservation is refused.
    let error = inbox.reserve_in(&mut task, post()).unwrap_err();
    // Then neither the caller's task nor a file includes that reservation.
    assert!(error.to_string().contains("owner_header_full"));
    assert_eq!(*recipient.state_mut(&mut task).unwrap(), before);
    assert!(!inbox.message_path(17).exists());
}

#[test]
fn observed_supersession_promotes_the_complete_ninth_entry_atomically() {
    // Given a terminal ninth identity whose evidence must survive FIFO promotion.
    let (root, store, recipient) = fixture();
    let inbox = Inbox::new(&store, recipient.clone()).unwrap();
    for _ in 0..16 {
        inbox.post(post()).unwrap();
    }
    inbox
        .mark_terminal(9, TerminalState::Cancelled, "unsent", "fixture")
        .unwrap();
    let original = inbox
        .state()
        .unwrap()
        .owner_header
        .unwrap()
        .overflow_entries[0]
        .clone();
    for seq in 1..=8 {
        inbox
            .record_receipt(seq, ReceiptLevel::Observed, &receipt())
            .unwrap();
    }
    // When a restarted owner performs one bounded maintenance transaction.
    let reopened = Store::open(root.path()).unwrap();
    let inbox = Inbox::new(&reopened, recipient).unwrap();
    assert!(inbox.maintain_owner_header(receipt().observed_at).unwrap());
    let state = inbox.state().unwrap();
    let header = state.owner_header.as_ref().unwrap();
    // Then seven older observations are superseded, and the ninth keeps every proof field.
    assert_eq!(header.entries[1], original);
    assert_eq!(
        header.entries.iter().map(|e| e.seq).collect::<Vec<_>>(),
        (8..=15).collect::<Vec<_>>()
    );
    assert_eq!(header.overflow_entries[0].seq, 16);
    assert!(header.full_since.is_none());
    assert_eq!(
        inbox.owner_entries(&state).unwrap()[1].terminal,
        Some(TerminalState::Cancelled)
    );
    assert!(!inbox.maintain_owner_header(receipt().observed_at).unwrap());
    inbox.post(post()).unwrap();
}

#[test]
fn only_later_observation_not_terminal_or_observation_alone_frees_capacity() {
    // Given full obligations, including an observation with no later owner observation.
    let (_root, store, recipient) = fixture();
    let inbox = Inbox::new(&store, recipient).unwrap();
    for _ in 0..16 {
        inbox.post(post()).unwrap();
    }
    inbox
        .record_receipt(1, ReceiptLevel::Observed, &receipt())
        .unwrap();
    for seq in 2..=16 {
        inbox
            .mark_terminal(seq, TerminalState::Failed, "fixture", "fixture")
            .unwrap();
    }
    // When attempting a retry or another owner admission.
    assert!(
        inbox
            .retry(2)
            .unwrap_err()
            .to_string()
            .contains("owner_header_full")
    );
    assert!(
        inbox
            .post(post())
            .unwrap_err()
            .to_string()
            .contains("owner_header_full")
    );
    // Then terminality alone retains all sixteen, but non-owner kinds are unaffected.
    assert_eq!(inbox.state().unwrap().owner_header.unwrap().len(), 16);
    let mut info = post();
    info.kind = "progress";
    assert_eq!(inbox.post(info).unwrap().seq, 17);
}

#[test]
fn committed_publication_gap_survives_and_unsaved_reservation_is_ignored() {
    // Given a reservation whose file was written but task transaction never committed.
    let (_root, store, recipient) = fixture();
    let inbox = Inbox::new(&store, recipient.clone()).unwrap();
    let mut task = store.get(recipient.task().unwrap()).unwrap();
    inbox.reserve_in(&mut task, post()).unwrap();
    assert!(inbox.messages().unwrap().is_empty());
    // When a committed post loses its file at the task-before-file boundary.
    let message = inbox.post(post()).unwrap();
    std::fs::remove_file(inbox.message_path(message.seq)).unwrap();
    // Then the exact identity remains inspectable without pretending it was safely unsent.
    let entries = inbox.owner_entries(&inbox.state().unwrap()).unwrap();
    assert_eq!(entries[0].message.seq, 1);
    assert!(entries[0].publication_gap);
    let inspection = inbox.inspect_retry(message.seq).unwrap();
    assert!(!inspection.evidence_complete);
    assert!(!inspection.eligible);
    assert_eq!(inspection.reason_code, "evidence_incomplete");
}

#[test]
fn legacy_indexing_is_explicit_bounded_and_does_not_certify_retry_history() {
    // Given twenty historical owner identities followed by a superseding observation.
    let (_root, store, recipient) = fixture();
    let inbox = Inbox::new(&store, recipient).unwrap();
    // Model a pre-upgrade writer with no owner header after each committed post.
    for _ in 0..20 {
        inbox
            .update_state(|state| {
                state.owner_header = Some(OwnerHeader::default());
                Ok(())
            })
            .unwrap();
        inbox.post(post()).unwrap();
    }
    for seq in 1..20 {
        inbox
            .mark_terminal(seq, TerminalState::Failed, "fixture", "fixture")
            .unwrap();
    }
    inbox
        .record_receipt(20, ReceiptLevel::Observed, &receipt())
        .unwrap();
    inbox
        .update_state(|state| {
            state.owner_header = None;
            Ok(())
        })
        .unwrap();
    // When sync initializes only the debt descriptor.
    use crate::work::{Work, count};
    let before = count(Work::InboxMessageRead);
    inbox.maintain_owner_header(receipt().observed_at).unwrap();
    assert_eq!(count(Work::InboxMessageRead), before);
    assert!(
        inbox
            .post(post())
            .unwrap_err()
            .to_string()
            .contains("owner_header_index_pending")
    );
    // Then explicit pages obey their budget and the observed watermark avoids history saturation.
    loop {
        let before = count(Work::InboxIndexRead);
        let header = inbox.index_owner(7).unwrap();
        assert!(count(Work::InboxIndexRead) - before <= 7);
        if header.legacy_debt.is_none() {
            assert_eq!(header.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![20]);
            assert_eq!(
                header.entries[0].retry_evidence.coverage,
                RetryCoverage::Unknown
            );
            break;
        }
    }
}

#[test]
fn full_legacy_collection_stops_at_first_unadmitted_identity_until_supersession() {
    // Given more than sixteen legacy obligations without a later observation.
    let (root, store, recipient) = fixture();
    let inbox = Inbox::new(&store, recipient.clone()).unwrap();
    for _ in 0..19 {
        inbox
            .update_state(|state| {
                state.owner_header = Some(OwnerHeader::default());
                Ok(())
            })
            .unwrap();
        inbox.post(post()).unwrap();
    }
    inbox
        .update_state(|state| {
            state.owner_header = None;
            Ok(())
        })
        .unwrap();
    // When indexing reaches capacity.
    let header = inbox.index_owner(64).unwrap();
    assert_eq!(header.len(), 16);
    assert_eq!(header.legacy_debt.unwrap().next_seq, 17);
    let reopened = Store::open(root.path()).unwrap();
    let inbox = Inbox::new(&reopened, recipient).unwrap();
    assert_eq!(
        inbox.index_owner(64).unwrap().legacy_debt.unwrap().next_seq,
        17
    );
    // Then true supersession permits continuation without silently losing the tail.
    for seq in 1..=16 {
        inbox
            .record_receipt(seq, ReceiptLevel::Observed, &receipt())
            .unwrap();
    }
    let header = inbox.index_owner(64).unwrap();
    assert!(header.legacy_debt.is_none());
    assert_eq!(
        header.iter().map(|e| e.seq).collect::<Vec<_>>(),
        vec![16, 17, 18, 19]
    );
}

#[test]
fn rejection_projection_is_complete_but_uncertainty_and_owner_resend_are_sticky() {
    for unsafe_first in [false, true] {
        // Given an original publication, possibly with prior uncertain delivery.
        let (_root, store, recipient) = fixture();
        let inbox = Inbox::new(&store, recipient).unwrap();
        inbox.post(post()).unwrap();
        let mut envelope = publish(&inbox);
        if unsafe_first {
            inbox
                .begin_attempt(&mut envelope, receipt().observed_at)
                .unwrap();
            inbox
                .finish_attempt(
                    &mut envelope,
                    SendOutcome::Uncertain {
                        reason: "timeout".into(),
                    },
                )
                .unwrap();
            let evidence = inbox.state().unwrap().owner_header.unwrap().entries[0]
                .retry_evidence
                .clone();
            inbox.recover(Recovery::Resend).unwrap();
            assert_eq!(
                inbox.state().unwrap().owner_header.unwrap().entries[0].retry_evidence,
                evidence
            );
            envelope = publish(&inbox);
        } else {
            inbox.retire_unattempted(&envelope).unwrap();
            envelope = publish(&inbox);
        }
        // When the replacement has an actual definitive rejection.
        reject(&inbox, &mut envelope);
        inbox
            .mark_terminal(1, TerminalState::Failed, "rejected", "fixture")
            .unwrap();
        let evidence = inbox.state().unwrap().owner_header.unwrap().entries[0]
            .retry_evidence
            .clone();
        // Then cumulative history, not the latest locator, determines retry safety.
        assert_eq!(evidence.permits(TerminalState::Failed), !unsafe_first);
        assert_eq!(inbox.inspect_retry(1).unwrap().eligible, !unsafe_first);
    }
}

#[test]
fn interrupted_attempt_and_outcome_projection_require_explicit_inspection() {
    // Given a durable attempt fence written before any envelope attempt or transport call.
    let (_root, store, recipient) = fixture();
    let inbox = Inbox::new(&store, recipient).unwrap();
    inbox.post(post()).unwrap();
    let mut envelope = publish(&inbox);
    let reference = AttemptRef {
        envelope: 1,
        ordinal: 1,
    };
    inbox
        .project_members(&[1], |entry| entry.retry_evidence.pending = Some(reference))
        .unwrap();
    inbox
        .mark_terminal(1, TerminalState::Failed, "fixture", "fixture")
        .unwrap();
    // When restarting before attempt persistence, full inspection cannot clear absent evidence.
    assert_eq!(
        inbox.inspect_retry(1).unwrap().reason_code,
        "evidence_incomplete"
    );
    // Then a persisted definitive rejection may prove eligibility on demand without a certificate.
    envelope.stages = vec![Stage::Dispatching, Stage::Outcome];
    envelope.attempts = vec![Attempt {
        at: receipt().observed_at,
        outcome: Some(SendOutcome::Rejected {
            reason: "fixture".into(),
            retryable_after_fix: true,
        }),
    }];
    envelope.outcome = envelope.attempts[0].outcome.clone();
    super::storage::write_json(&inbox.envelope_path(1), &envelope).unwrap();
    let before = inbox.state().unwrap();
    assert!(inbox.inspect_retry(1).unwrap().eligible);
    assert_eq!(inbox.state().unwrap(), before);
    assert!(
        !before.owner_header.unwrap().entries[0]
            .retry_evidence
            .permits(TerminalState::Failed)
    );
}

#[test]
fn generic_writers_receipts_and_stale_callbacks_never_improve_retry_proof() {
    // Given two attempts whose last outcome is a safe rejection.
    let (_root, store, recipient) = fixture();
    let inbox = Inbox::new(&store, recipient).unwrap();
    inbox.post(post()).unwrap();
    let mut envelope = publish(&inbox);
    inbox
        .begin_attempt(&mut envelope, receipt().observed_at)
        .unwrap();
    let mut stale = envelope.clone();
    inbox
        .finish_attempt(
            &mut envelope,
            SendOutcome::Rejected {
                reason: "first".into(),
                retryable_after_fix: true,
            },
        )
        .unwrap();
    inbox
        .begin_attempt(&mut envelope, receipt().observed_at)
        .unwrap();
    let before = inbox.state().unwrap();
    // When an old callback arrives it cannot clear the second fence.
    inbox
        .finish_attempt(
            &mut stale,
            SendOutcome::Rejected {
                reason: "first".into(),
                retryable_after_fix: true,
            },
        )
        .unwrap();
    assert_eq!(inbox.state().unwrap(), before);
    // Then a generic evidence mutation bars proof before publishing it, even if later truncated.
    envelope.outcome = Some(SendOutcome::Accepted);
    inbox.save_envelope(&envelope).unwrap();
    envelope.outcome = None;
    envelope.attempts.clear();
    envelope.stages.clear();
    inbox.save_envelope(&envelope).unwrap();
    let evidence = inbox.state().unwrap().owner_header.unwrap().entries[0]
        .retry_evidence
        .clone();
    assert_eq!(evidence.coverage, RetryCoverage::Unknown);
    assert!(matches!(evidence.history, RetryHistory::Barred { .. }));
    inbox
        .record_receipt(1, ReceiptLevel::Admitted, &receipt())
        .unwrap();
    assert_eq!(
        inbox.state().unwrap().owner_header.unwrap().entries[0].retry_evidence,
        evidence
    );
}

fn crash<T>(point: &'static str, run: impl FnOnce() -> anyhow::Result<T>) {
    super::header::CRASH_AT.with(|selected| selected.set(Some(point)));
    let result = run();
    super::header::CRASH_AT.with(|selected| selected.set(None));
    assert!(result.is_err(), "barrier {point} was not reached");
    assert!(
        result.err().unwrap().to_string().contains("injected_crash"),
        "{point}"
    );
}

#[test]
fn publication_and_receipt_crash_windows_retain_exact_identity_and_conservative_proof() {
    for point in [
        "reserve_file_written",
        "post_reserved",
        "envelope_written",
        "receipt_barred",
        "receipt_written",
    ] {
        // Given a stopped producer at a named persistence boundary.
        let (root, store, recipient) = fixture();
        let inbox = Inbox::new(&store, recipient.clone()).unwrap();
        match point {
            "reserve_file_written" => crash(point, || {
                store.update(recipient.task().unwrap(), |task| {
                    inbox.reserve_in(task, post())?;
                    Ok(())
                })
            }),
            "post_reserved" => crash(point, || inbox.post(post())),
            "envelope_written" => {
                inbox.post(post()).unwrap();
                crash(point, || {
                    inbox.publish(
                        &SessionBinding {
                            session: "fixture".into(),
                            endpoint: None,
                            transcript: None,
                        },
                        "fixture",
                        MAX_PAYLOAD,
                        true,
                    )
                });
            }
            "receipt_barred" | "receipt_written" => {
                inbox.post(post()).unwrap();
                crash(point, || {
                    inbox.record_receipt(1, ReceiptLevel::Observed, &receipt())
                });
            }
            _ => unreachable!(),
        }
        // When a new store reads the committed records.
        let reopened = Store::open(root.path()).unwrap();
        let inbox = Inbox::new(&reopened, recipient).unwrap();
        let state = inbox.state().unwrap();
        if point == "reserve_file_written" {
            assert_eq!(state.next_seq, 1);
            assert!(inbox.owner_entries(&state).unwrap().is_empty());
            assert!(inbox.messages().unwrap().is_empty());
            continue;
        }
        // Then committed publication gaps and receipt bars cannot disappear.
        let entries = inbox.owner_entries(&state).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message.seq, 1);
        assert_eq!(entries[0].publication_gap, point == "post_reserved");
        if point == "envelope_written" {
            assert_eq!(entries[0].message.envelope, None);
            inbox.recover_publication().unwrap();
            assert_eq!(
                inbox.state().unwrap().owner_header.unwrap().entries[0].envelope,
                Some(1)
            );
        }
        if point.starts_with("receipt_") {
            assert!(
                !entries[0]
                    .message
                    .retry_evidence
                    .permits(TerminalState::Failed)
            );
            assert_eq!(entries[0].observed.is_some(), point == "receipt_written");
        }
    }
}

#[test]
fn all_attempt_crash_windows_are_safe_or_explicitly_incomplete_after_restart() {
    for point in [
        "before_attempt_fence",
        "attempt_fenced",
        "attempt_written",
        "before_attempt_outcome",
        "attempt_outcome_written",
        "attempt_projected",
    ] {
        // Given a publication stopped before or after a fence, attempt, or outcome write.
        let (root, store, recipient) = fixture();
        let inbox = Inbox::new(&store, recipient.clone()).unwrap();
        inbox.post(post()).unwrap();
        let mut envelope = publish(&inbox);
        if point.starts_with("before_attempt_outcome")
            || point.starts_with("attempt_outcome")
            || point == "attempt_projected"
        {
            inbox
                .begin_attempt(&mut envelope, receipt().observed_at)
                .unwrap();
            // This is the independent transport-call boundary; the fixture outcome is definitive.
            crash(point, || {
                inbox.finish_attempt(
                    &mut envelope,
                    SendOutcome::Rejected {
                        reason: "not admitted".into(),
                        retryable_after_fix: true,
                    },
                )
            });
        } else {
            crash(point, || {
                inbox.begin_attempt(&mut envelope, receipt().observed_at)
            });
        }
        // When the store restarts and the message is terminalized.
        let reopened = Store::open(root.path()).unwrap();
        let inbox = Inbox::new(&reopened, recipient).unwrap();
        inbox
            .mark_terminal(1, TerminalState::Failed, "fixture", "fixture")
            .unwrap();
        let evidence = inbox.state().unwrap().owner_header.unwrap().entries[0]
            .retry_evidence
            .clone();
        let inspection = inbox.inspect_retry(1).unwrap();
        // Then only an entirely unattempted or fully projected rejection is safe on the hot path.
        assert_eq!(
            evidence.permits(TerminalState::Failed),
            matches!(point, "before_attempt_fence" | "attempt_projected"),
            "{point}"
        );
        assert_eq!(
            inspection.eligible,
            matches!(
                point,
                "before_attempt_fence" | "attempt_outcome_written" | "attempt_projected"
            ),
            "{point}"
        );
        assert_eq!(
            inbox.state().unwrap().owner_header.unwrap().entries[0].retry_evidence,
            evidence
        );
    }
}

#[test]
fn promotion_crashes_leave_one_complete_old_or_new_header() {
    for point in ["before_promotion", "promotion_committed"] {
        // Given sixteen entries and seven superseded observations.
        let (root, store, recipient) = fixture();
        let inbox = Inbox::new(&store, recipient.clone()).unwrap();
        for _ in 0..16 {
            inbox.post(post()).unwrap();
        }
        for seq in 1..=8 {
            inbox
                .record_receipt(seq, ReceiptLevel::Observed, &receipt())
                .unwrap();
        }
        let ninth = inbox
            .state()
            .unwrap()
            .owner_header
            .unwrap()
            .overflow_entries[0]
            .clone();
        // When stopped at either side of the maintenance task commit.
        crash(point, || inbox.maintain_owner_header(receipt().observed_at));
        let reopened = Store::open(root.path()).unwrap();
        let inbox = Inbox::new(&reopened, recipient).unwrap();
        // Then the ninth identity is present exactly once with byte-equal evidence.
        let header = inbox.state().unwrap().owner_header.unwrap();
        assert_eq!(header.iter().filter(|entry| entry.seq == 9).count(), 1);
        assert_eq!(header.iter().find(|entry| entry.seq == 9).unwrap(), &ninth);
        assert_eq!(
            header.entries.iter().any(|entry| entry.seq == 9),
            point == "promotion_committed"
        );
    }
}

#[test]
fn hot_header_work_is_equal_with_tenfold_message_envelope_attempt_and_stage_history() {
    // Given equal current owner projections but 1x and 10x historical evidence on disk.
    let mut measured = Vec::new();
    for multiplier in [1, 10] {
        let (_root, store, recipient) = fixture();
        let inbox = Inbox::new(&store, recipient).unwrap();
        for _ in 0..2 {
            inbox.post(post()).unwrap();
        }
        let mut envelope = publish(&inbox);
        for _ in 0..multiplier {
            reject(&inbox, &mut envelope);
        }
        // Keep current metadata equal; an equal final proof is independent of history length.
        let mut state = inbox.state().unwrap();
        let header = state.owner_header.as_mut().unwrap();
        for entry in header.entries.iter_mut() {
            entry.retry_evidence = Default::default();
            entry.delivery = None;
        }
        for seq in 100..100 + multiplier * 10 {
            std::fs::write(inbox.message_path(seq), "older body").unwrap();
            std::fs::write(inbox.envelope_path(seq), "older envelope").unwrap();
        }
        use crate::work::{Work, count};
        let before = Work::ALL.map(count);
        let entries = inbox.owner_entries(&state).unwrap();
        let delta = Work::ALL
            .map(count)
            .into_iter()
            .zip(before)
            .map(|(a, b)| a - b)
            .collect::<Vec<_>>();
        measured.push((entries, delta));
    }
    // When assessed, then current results and every measured work counter are equal.
    // Message-created times are canonical but distinct across the independent fixtures.
    for entries in measured.iter_mut().map(|(entries, _)| entries) {
        for entry in entries {
            entry.message.created = receipt().observed_at;
        }
    }
    assert_eq!(measured[0], measured[1]);
}

#[test]
fn full_owner_header_serialization_has_a_measured_fixed_bound() {
    // Given maximum-width scalar values, the longest owner kind, and every proof slot filled.
    let reference = AttemptRef {
        envelope: u64::MAX,
        ordinal: u64::MAX,
    };
    let entry = OwnerMessage {
        seq: u64::MAX,
        kind: "changes_requested".into(),
        created: Timestamp::MAX,
        envelope: Some(u64::MAX),
        delivery: Some(DeliveryHeader {
            state: DeliveryState::Dispatching,
            at: Some(Timestamp::MAX),
            attempt: Some(reference),
        }),
        retry_evidence: RetryEvidence {
            coverage: RetryCoverage::Complete,
            route: RetryRoute::AlternateUnknown,
            history: RetryHistory::Barred {
                first: RetryWitness {
                    reason: WitnessReason::AlternateSubmission,
                    attempt: Some(reference),
                },
            },
            pending: Some(reference),
        },
    };
    let header = OwnerHeader {
        entries: vec![entry.clone(); 8],
        overflow_entries: vec![entry; 8],
        overflow_since: Some(Timestamp::MAX),
        full_since: Some(Timestamp::MAX),
        legacy_debt: Some(LegacyDebt {
            phase: IndexPhase::Watermark,
            next_seq: u64::MAX,
            end_seq: u64::MAX,
            latest_observed_owner: Some(u64::MAX),
            started: Timestamp::MAX,
        }),
    };
    // When serializing the complete bounded shape, then no history vector contributes bytes.
    let bytes = serde_json::to_vec(&header).unwrap().len();
    println!("maximum-width owner header JSON: {bytes} bytes");
    assert!(bytes < 16 * 1024, "{bytes}");
}
