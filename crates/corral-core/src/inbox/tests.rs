use std::fs;
use std::sync::Barrier;
use std::thread;

use serde_json::json;

use super::*;
use crate::Store;

#[path = "tests_envelopes.rs"]
mod envelopes;

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
        let path = inbox.record_path(message.seq, &format!("receipt.{}", level.suffix()));
        let bytes = fs::read(&path).unwrap();
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
        assert_eq!(fs::read(path).unwrap(), bytes);
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
            assert!(!inbox.record_path(message.seq, terminal.suffix()).exists());
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
    let mut task = store.get(recipient.task().unwrap()).unwrap();
    let lost = inbox
        .reserve_in(&mut task, post("lost request", Urgency::Wake))
        .unwrap();
    assert!(inbox.message_path(lost.seq).is_file());
    // Then nothing reads it, and the next committed post takes the same seq.
    assert!(inbox.messages().unwrap().is_empty());
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
