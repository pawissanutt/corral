use std::sync::Barrier;
use std::thread;

use serde_json::json;

use super::*;
use crate::Store;

#[path = "tests_envelopes.rs"]
mod envelopes;
#[path = "tests_publication_units.rs"]
mod publication_units;

#[path = "tests_process.rs"]
mod process;

fn fixture() -> (tempfile::TempDir, Store) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    (root, store)
}

fn post(body: &str, urgency: Urgency) -> Post<'_> {
    Post {
        kind: "owner_note",
        urgency,
        author: "owner",
        body,
        binding: MessageBinding::default(),
        subject: None,
        max_payload: MAX_PAYLOAD,
    }
}

#[test]
fn nested_pm_posts_roll_back_with_full_and_head_parent_task_units() {
    for head_form in [false, true] {
        let (_root, store) = fixture();
        let task = store
            .create("Parent", None, crate::Status::Ready, "")
            .unwrap();
        let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
        let before = inbox.state().unwrap();
        let events = store.events().unwrap();
        let result = if head_form {
            store
                .update_head(&task.id, |_| {
                    inbox.post(post("must roll back", Urgency::Info))?;
                    anyhow::bail!("parent refused")
                })
                .map(drop)
        } else {
            store
                .update(&task.id, |_| {
                    inbox.post(post("must roll back", Urgency::Info))?;
                    anyhow::bail!("parent refused")
                })
                .map(drop)
        };
        assert!(result.unwrap_err().to_string().contains("parent refused"));
        assert_eq!(inbox.state().unwrap(), before);
        assert!(inbox.messages().unwrap().is_empty());
        assert_eq!(store.events().unwrap(), events);
    }
}

fn binding(session: &str) -> SessionBinding {
    SessionBinding {
        session: session.into(),
        endpoint: Some(PathBuf::from(format!("/fixture/{session}.sock"))),
        transcript: Some(PathBuf::from(format!("/fixture/{session}.jsonl"))),
    }
}

fn receipt(via: &str) -> Receipt {
    Receipt {
        envelope: Some(1),
        binding: Some(binding("original")),
        observed_at: "2026-10-19T00:00:00Z".parse().unwrap(),
        via: via.into(),
        detail: json!({"record": via}),
    }
}

fn publish(inbox: &Inbox<'_>, session: &str, flush: bool) -> Option<Envelope> {
    let _deliver = inbox.delivery_lock().unwrap();
    inbox
        .publish(&binding(session), "fixture", MAX_PAYLOAD, flush)
        .unwrap()
}

#[test]
fn concurrent_posters_reserve_distinct_sequences_and_persist_messages() {
    // Given independent store handles targeting one PM inbox.
    let (root, store) = fixture();
    let barrier = Barrier::new(8);
    let mut posted = thread::scope(|scope| {
        let workers: Vec<_> = (0..8)
            .map(|worker| {
                let barrier = &barrier;
                let root = root.path();
                scope.spawn(move || {
                    let store = Store::open(root).unwrap();
                    let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
                    let body = format!("poster {worker}");
                    // When all posters begin together.
                    barrier.wait();
                    inbox.post(post(&body, Urgency::Wake)).unwrap()
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>()
    });

    // Then each durable message owns one sequence, including after reopening.
    posted.sort_by_key(|message| message.seq);
    assert_eq!(
        posted.iter().map(|message| message.seq).collect::<Vec<_>>(),
        (1..=8).collect::<Vec<_>>()
    );
    drop(store);
    let reopened = Store::open(root.path()).unwrap();
    let inbox = Inbox::new(&reopened, Recipient::Pm).unwrap();
    assert_eq!(inbox.messages().unwrap(), posted);
    assert_eq!(inbox.state().unwrap().next_seq, 9);
    let next = inbox.post(post("after reopen", Urgency::Wake)).unwrap();
    assert_eq!(next.seq, 9);
    assert_eq!(inbox.message(9).unwrap(), next);
    assert_eq!(inbox.state().unwrap().next_seq, 10);
}

#[test]
fn receipt_creation_is_exclusive_and_preserves_first_receipt_bytes() {
    // Given an existing receipt at each independent receipt level.
    let (_root, store) = fixture();
    let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
    let message = inbox.post(post("receipt", Urgency::Wake)).unwrap();
    for level in [ReceiptLevel::Admitted, ReceiptLevel::Observed] {
        let first = receipt(level.suffix());
        assert!(inbox.record_receipt(message.seq, level, &first).unwrap());
        let kind = format!("receipt.{}", level.suffix());
        let key = format!("{:020}", message.seq);
        let record = inbox.record::<Value>(&kind, &key).unwrap();
        let mut replacement = receipt("replacement");
        replacement.observed_at = "2026-10-19T01:00:00Z".parse().unwrap();

        // When a later caller tries to replace it.
        assert!(
            !inbox
                .record_receipt(message.seq, level, &replacement)
                .unwrap()
        );

        // Then neither the original evidence nor its serialized bytes changes.
        assert_eq!(inbox.receipt(message.seq, level).unwrap(), Some(first));
        assert_eq!(inbox.record::<Value>(&kind, &key).unwrap(), record);
    }
}

#[test]
fn concurrent_receipt_writers_have_exactly_one_winner() {
    // Given one posted message and competing independent receipt writers.
    let (root, store) = fixture();
    let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
    let message = inbox.post(post("receipt race", Urgency::Wake)).unwrap();
    let barrier = Barrier::new(4);

    // When writers contend for the same exclusive receipt.
    let winners = thread::scope(|scope| {
        let workers: Vec<_> = (0..4)
            .map(|worker| {
                let barrier = &barrier;
                let root = root.path();
                let seq = message.seq;
                scope.spawn(move || {
                    let store = Store::open(root).unwrap();
                    let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
                    let evidence = receipt(&format!("writer-{worker}"));
                    barrier.wait();
                    let inserted = inbox
                        .record_receipt(seq, ReceiptLevel::Observed, &evidence)
                        .unwrap();
                    (inserted, evidence)
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .filter_map(|(inserted, evidence)| inserted.then_some(evidence))
            .collect::<Vec<_>>()
    });

    // Then the only successful writer supplied the persisted receipt.
    assert_eq!(winners.len(), 1);
    assert_eq!(
        inbox.receipt(message.seq, ReceiptLevel::Observed).unwrap(),
        Some(winners[0].clone())
    );
}

#[test]
fn observed_wins_terminal_races_and_both_serial_orders() {
    // Given cancellation, failure and abandonment competing with observation.
    let (_root, store) = fixture();
    let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
    for terminal in [
        TerminalState::Cancelled,
        TerminalState::Failed,
        TerminalState::Abandoned,
    ] {
        for order in 0..3 {
            let message = inbox.post(post("race", Urgency::Wake)).unwrap();
            let evidence = receipt("transcript");
            // When observation comes first, last, or starts concurrently.
            match order {
                0 => {
                    assert!(
                        inbox
                            .mark_terminal(message.seq, terminal, "end", "test")
                            .unwrap()
                    );
                    assert!(
                        inbox
                            .record_receipt(message.seq, ReceiptLevel::Observed, &evidence)
                            .unwrap()
                    );
                }
                1 => {
                    assert!(
                        inbox
                            .record_receipt(message.seq, ReceiptLevel::Observed, &evidence)
                            .unwrap()
                    );
                    assert!(
                        !inbox
                            .mark_terminal(message.seq, terminal, "end", "test")
                            .unwrap()
                    );
                }
                2 => {
                    let barrier = Barrier::new(2);
                    thread::scope(|scope| {
                        let observed = scope.spawn(|| {
                            barrier.wait();
                            inbox
                                .record_receipt(message.seq, ReceiptLevel::Observed, &evidence)
                                .unwrap()
                        });
                        let cancelled = scope.spawn(|| {
                            barrier.wait();
                            inbox
                                .mark_terminal(message.seq, terminal, "end", "test")
                                .unwrap()
                        });
                        assert!(observed.join().unwrap());
                        cancelled.join().unwrap();
                    });
                }
                _ => unreachable!(),
            }

            // Then observation is durable and no conflicting terminal file survives.
            assert_eq!(
                inbox.receipt(message.seq, ReceiptLevel::Observed).unwrap(),
                Some(evidence)
            );
            assert_eq!(inbox.terminal(message.seq).unwrap(), None);
            assert!(
                inbox
                    .record::<Value>(terminal.suffix(), &format!("{:020}", message.seq))
                    .unwrap()
                    .is_none()
            );
            assert!(
                !inbox
                    .mark_terminal(message.seq, terminal, "late", "test")
                    .unwrap()
            );
        }
    }
}

fn run_fixture() -> (tempfile::TempDir, Store, Recipient) {
    let (root, store) = fixture();
    let task = store
        .create("inbox run", None, crate::Status::Running, "")
        .unwrap();
    let run: crate::Run = serde_json::from_value(json!({
        "id":"R1","agent":"omo","worktree":root.path(),"branch":"b","base":"main",
        "base_sha":"0","started":"2026-10-19T00:00:00Z"
    }))
    .unwrap();
    store
        .update(&task.id, |task| {
            task.runs.push(run);
            Ok(())
        })
        .unwrap();
    let recipient = Recipient::Run {
        task: task.id,
        run: "R1".into(),
    };
    (root, store, recipient)
}

#[test]
fn an_unsaved_reservation_is_invisible_and_overwritten() {
    // Given a reservation whose task transaction never saved (a crash before commit).
    let (_dir, store, recipient) = run_fixture();
    let inbox = Inbox::new(&store, recipient.clone()).unwrap();
    let _lock = inbox.lock().unwrap();
    let mut lost = None;
    let failed = store.update(recipient.task().unwrap(), |task| {
        lost = Some(inbox.reserve_in(task, post("lost request", Urgency::Wake))?);
        anyhow::bail!("abort before commit")
    });
    assert!(failed.is_err());
    let lost = lost.unwrap();
    // Then neither the message row nor its index is visible, and the sequence is reused.
    assert!(inbox.message(lost.seq).is_err());
    assert!(inbox.records("messages").unwrap().is_empty());
    assert!(inbox.pending_messages().unwrap().is_empty());
    assert!(inbox.messages().unwrap().is_empty());
    drop(_lock);
    let kept = inbox.post(post("kept request", Urgency::Wake)).unwrap();
    assert_eq!(kept.seq, lost.seq);
    let messages = inbox.messages().unwrap();
    assert_eq!(messages.len(), 1);
    assert!(messages[0].body.ends_with("kept request"));
}

#[test]
fn a_saved_reservation_is_published_with_its_transaction() {
    let (_dir, store, recipient) = run_fixture();
    let inbox = Inbox::new(&store, recipient.clone()).unwrap();
    let _lock = inbox.lock().unwrap();
    let mut reserved = None;
    store
        .update(recipient.task().unwrap(), |task| {
            reserved = Some(inbox.reserve_in(task, post("together", Urgency::Wake))?);
            Ok(())
        })
        .unwrap();
    let messages = inbox.messages().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].id, reserved.unwrap().id);
    let mut task = store.get(recipient.task().unwrap()).unwrap();
    let oversized = "x".repeat(MAX_PAYLOAD);
    assert!(
        inbox
            .reserve_in(&mut task, post(&oversized, Urgency::Wake))
            .is_err()
    );
    assert_eq!(recipient.state_mut(&mut task).unwrap().next_seq, 2);
}

#[test]
fn a_head_reservation_reserves_and_publishes_as_the_full_one_does() {
    let mut observed = Vec::new();
    for head in [false, true] {
        let (_dir, store, recipient) = run_fixture();
        let id = recipient.task().unwrap().to_owned();
        let inbox = Inbox::new(&store, recipient.clone()).unwrap();
        let _lock = inbox.lock().unwrap();
        let mut reserved = None;
        if head {
            store
                .update_head(&id, |h| {
                    reserved = Some(inbox.reserve_in_head(h, post("together", Urgency::Wake))?);
                    Ok(())
                })
                .unwrap();
            let oversized = "x".repeat(MAX_PAYLOAD);
            assert!(
                store
                    .update_head(&id, |h| inbox
                        .reserve_in_head(h, post(&oversized, Urgency::Wake))
                        .map(drop))
                    .is_err()
            );
        } else {
            store
                .update(&id, |task| {
                    reserved = Some(inbox.reserve_in(task, post("together", Urgency::Wake))?);
                    Ok(())
                })
                .unwrap();
        }
        let reserved = reserved.unwrap();
        let messages = inbox.messages().unwrap();
        let mut task = store.get(&id).unwrap();
        let next = recipient.state_mut(&mut task).unwrap().next_seq;
        observed.push((
            reserved.id,
            reserved.seq,
            reserved.body,
            messages.len(),
            messages[0].id.clone(),
            next,
        ));
    }
    assert_eq!(observed[0], observed[1]);
    assert_eq!(observed[1].5, 2);
}

#[test]
fn a_refused_envelope_reads_back_held_not_failed_until_requeued() {
    // Given a PM wake whose envelope the bound host refused before admission:
    let (_root, store) = fixture();
    let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
    let message = inbox.post(post("wake", Urgency::Wake)).unwrap();
    let mut envelope = publish(&inbox, "g7", false).unwrap();
    inbox
        .begin_attempt(&mut envelope, Timestamp::now())
        .unwrap();
    let refusal = Refusal {
        attempt: 1,
        code: "input_limit".into(),
        generation: Some(7),
    };
    inbox.record_refusal(envelope.number, &refusal).unwrap();
    inbox
        .finish_attempt(
            &mut envelope,
            SendOutcome::Rejected {
                reason: "input_limit".into(),
                retryable_after_fix: true,
            },
        )
        .unwrap();

    // Then its readback reports the hold, not a terminal state:
    let entry = inbox.entry(message.seq).unwrap();
    assert_eq!(entry.terminal, None);
    let hold = InboxHold {
        envelope: 1,
        code: "input_limit".into(),
        generation: Some(7),
        supervised: false,
        release: PM_REPLACE.into(),
    };
    assert_eq!(entry.hold, Some(hold));
    // And retry inspection names the replacement that releases it:
    let inspection = inbox.inspect_retry(message.seq).unwrap();
    assert_eq!(inspection.reason_code, "held_for_replacement");
    assert_eq!(inspection.next_action, PM_REPLACE);

    // Once requeued for the replacement, it is no longer held:
    {
        let _deliver = inbox.delivery_lock().unwrap();
        inbox.requeue_refused(&envelope, &refusal).unwrap();
    }
    assert_eq!(inbox.entry(message.seq).unwrap().hold, None);
}
