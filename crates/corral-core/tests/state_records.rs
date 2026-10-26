//! T-323: record reads share views and cross-handle writes join the task unit.

use std::sync::Arc;

use corral_core::backend::{
    BackendConfig, FaultInjector, ManualClock, StoreBackend, fake::FakeBackend,
};
use corral_core::inbox::{
    Inbox, MAX_PAYLOAD, MessageBinding, Post, Receipt, ReceiptLevel, Recipient, TerminalState,
    Urgency,
};
use corral_core::work::{self, Counter, HotScope};
use corral_core::{Status, Store};
use serde_json::{Value, json};

fn fixture(fake: bool) -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let store = if fake {
        store.with_backend(Box::new(FakeBackend::new(
            BackendConfig::default(),
            Arc::new(ManualClock::new("2026-10-25T00:00:00Z".parse().unwrap())),
            Arc::new(FaultInjector::default()),
        )) as Box<dyn StoreBackend>)
    } else {
        store
    };
    (dir, store)
}

#[test]
fn records_from_another_handle_join_task_commit_and_rollback_on_both_backends() {
    for fake in [true, false] {
        // Given a task and a different Store handle sharing its backend.
        let (dir, store) = fixture(fake);
        let task = store
            .create("atomic records", None, Status::Ready, "")
            .unwrap();
        let other = if fake {
            store.clone()
        } else {
            Store::open_unobserved(dir.path()).unwrap()
        };
        let before = store.view().unwrap().seq();

        // When a callback writes twice through the other handle, then commits.
        store
            .update(&task.id, |task| {
                other.save_record("test", "current", &json!({"value":1}))?;
                assert_eq!(
                    store.record::<Value>("test", "current")?,
                    Some(json!({"value":1}))
                );
                other.save_record("test", "current", &json!({"value":2}))?;
                task.title = "committed".into();
                Ok(())
            })
            .unwrap();

        // Then one operation contains the task and final version of its record.
        assert_eq!(store.view().unwrap().seq().0, before.0 + 1);
        let committed = store.record_row("test", "current").unwrap().unwrap();
        assert_eq!(committed.version, 1);
        assert_eq!(committed.doc, json!({"value":2}));
        let before = store.view().unwrap().seq();

        // When another callback buffers a deletion and an insertion but fails.
        let result = store.update(&task.id, |_| {
            other.delete_record("test", "current")?;
            other.save_record("test", "discarded", &true)?;
            anyhow::bail!("callback refused")
        });

        // Then neither write nor any change entry escaped its unit.
        assert!(result.is_err());
        assert_eq!(store.view().unwrap().seq(), before);
        assert_eq!(
            store.record_row("test", "current").unwrap(),
            Some(committed)
        );
        assert_eq!(store.record::<Value>("test", "discarded").unwrap(), None);
        assert_eq!(store.get(&task.id).unwrap().title, "committed");
    }
}

#[test]
fn record_unit_reads_one_committed_view_across_handles_and_keys() {
    for fake in [false, true] {
        // Given an atomic pair and independent handles to its backend.
        let (dir, store) = fixture(fake);
        store
            .records_transaction(|| {
                store.save_record("unit-view", "first", &0)?;
                store.save_record("unit-view", "second", &0)
            })
            .unwrap();
        let other = if fake {
            store.clone()
        } else {
            Store::open_unobserved(dir.path()).unwrap()
        };

        // When another thread atomically changes both keys between this unit's reads.
        store
            .records_transaction(|| {
                let first = store.record::<u64>("unit-view", "first")?;
                let (finished, completion) = std::sync::mpsc::channel();
                let writer = std::thread::spawn(move || {
                    let before = other.view().unwrap().seq();
                    let result = other.records_transaction(|| {
                        other.save_record("unit-view", "first", &1)?;
                        other.save_record("unit-view", "second", &1)
                    });
                    let after = other.view().unwrap().seq();
                    finished.send((result, before, after)).unwrap();
                });
                let (result, before, after) = completion
                    .recv_timeout(std::time::Duration::from_secs(10))
                    .unwrap();
                writer.join().unwrap();
                result?;
                assert_eq!(after.0, before.0 + 1, "writer must publish one operation");

                // Then both keyed and area reads still describe the original committed pair.
                assert_eq!(first, Some(0));
                assert_eq!(store.record::<u64>("unit-view", "second")?, Some(0));
                let rows = store.record_rows("unit-view")?;
                assert_eq!(rows.len(), 2);
                assert!(rows.iter().all(|row| row.doc == json!(0)));
                Ok(())
            })
            .unwrap();
        assert_eq!(store.record::<u64>("unit-view", "first").unwrap(), Some(1));
        assert_eq!(store.record::<u64>("unit-view", "second").unwrap(), Some(1));
    }
}

#[test]
fn record_snapshots_keep_their_committed_view_on_both_backends() {
    for fake in [true, false] {
        // Given a snapshot of the current record and an absent record.
        let (_dir, store) = fixture(fake);
        store.save_record("test", "current", &1).unwrap();
        let snapshot = store.read_only_snapshot().unwrap();

        // When the writer changes both keys after the view was opened.
        store.save_record("test", "current", &2).unwrap();
        store.save_record("test", "later", &3).unwrap();

        // Then keyed reads and area reads still agree on the original state.
        assert_eq!(snapshot.record::<u64>("test", "current").unwrap(), Some(1));
        assert_eq!(snapshot.record::<u64>("test", "later").unwrap(), None);
        assert_eq!(snapshot.record_rows("test").unwrap().len(), 1);
        assert_eq!(store.record::<u64>("test", "current").unwrap(), Some(2));
        assert_eq!(store.record_rows("test").unwrap().len(), 2);
        assert!(snapshot.save_record("test", "current", &1).is_err());
        assert!(snapshot.delete_record("test", "absent").is_err());
        assert!(snapshot.insert_record("test", "current", &1).is_err());
        assert!(snapshot.insert_record("test", "absent", &1).is_err());
    }
}

fn post() -> Post<'static> {
    Post {
        kind: "owner_note",
        urgency: Urgency::Wake,
        author: "owner",
        body: "bounded inbox",
        binding: MessageBinding::default(),
        subject: None,
        max_payload: MAX_PAYLOAD,
    }
}

#[test]
fn unfinished_index_ignores_tenfold_finished_history_on_both_backends() {
    for fake in [true, false] {
        let mut measurements = Vec::new();
        for history in [10, 100] {
            // Given the same active recipient with differing finished message/effect history.
            let (_dir, store) = fixture(fake);
            let task = store
                .create_initialized("inbox", None, Status::Running, "", |task| {
                    task.runs.push(serde_json::from_value(json!({
                        "id":"R1", "agent":"fixture", "worktree":"/fixture", "branch":"b",
                        "base":"main", "base_sha":"0", "started":"2026-10-25T00:00:00Z"
                    }))?);
                    Ok(())
                })
                .unwrap();
            let recipient = Recipient::Run {
                task: task.id,
                run: "R1".into(),
            };
            let inbox = Inbox::new(&store, recipient).unwrap();
            let receipt = Receipt {
                envelope: None,
                binding: None,
                observed_at: "2026-10-25T00:00:00Z".parse().unwrap(),
                via: "fixture".into(),
                detail: json!({}),
            };
            for _ in 0..history {
                let message = inbox.post(post()).unwrap();
                inbox
                    .record_receipt(message.seq, ReceiptLevel::Observed, &receipt)
                    .unwrap();
                inbox.start_effect(message.seq, false, None).unwrap();
                inbox.finish_effect(message.seq).unwrap();
            }
            // Keep identities and document sizes of the unfinished rows fixed in both variants.
            inbox
                .update_state(|state| {
                    state.next_seq = 1001;
                    Ok(())
                })
                .unwrap();
            let pending = inbox.post(post()).unwrap();
            let effect = inbox.post(post()).unwrap();
            let terminal = inbox.post(post()).unwrap();
            inbox
                .record_receipt(effect.seq, ReceiptLevel::Observed, &receipt)
                .unwrap();
            inbox
                .mark_terminal(
                    terminal.seq,
                    TerminalState::Cancelled,
                    "finished",
                    "fixture",
                )
                .unwrap();
            for message in [&pending, &effect] {
                let key = format!("{:020}", message.seq);
                let mut doc: Value = inbox.record("messages", &key).unwrap().unwrap();
                doc["message"]["created"] = json!("2026-10-25T00:00:00Z");
                inbox.save_record("messages", &key, &doc).unwrap();
            }
            let expected = inbox
                .messages()
                .unwrap()
                .into_iter()
                .filter_map(|message| {
                    let entry = inbox.entry(message.seq).unwrap();
                    ((entry.observed.is_none() && entry.terminal.is_none())
                        || (entry.observed.is_some()
                            && !inbox.effect_applied(message.seq).unwrap()
                            && entry.effect.is_none_or(|effect| {
                                effect.status == corral_core::inbox::EffectStatus::Started
                            })))
                    .then_some(message.seq)
                })
                .collect::<Vec<_>>();

            // When the hot reader selects the recipient's unfinished messages.
            let before = work::snapshot();
            let actual = {
                let _hot = HotScope::enter();
                inbox
                    .unfinished_messages()
                    .unwrap()
                    .into_iter()
                    .map(|m| m.seq)
                    .collect::<Vec<_>>()
            };
            let measured = work::snapshot().since(&before);

            // Then it equals the full historical selection but decodes no finished rows.
            assert_eq!(actual, expected);
            assert_eq!(actual, [pending.seq, effect.seq]);
            measured.check_identity().unwrap();
            assert_eq!(measured.full_task_hydrations(), 0);
            assert_eq!(measured.get(Counter::RecordDecodes), 4);
            measurements.push(measured);
        }
        assert_eq!(measurements[0], measurements[1]);
    }
}

#[test]
fn launch_reservations_have_atomic_pane_rows_for_full_and_head_writers() {
    use corral_core::backend::{AggregateName, PageReq, RowQuery, TaskPart};

    for fake in [true, false] {
        // Given an unlaunched task and its full/head writer surfaces.
        let (_dir, store) = fixture(fake);
        let task = store.create("launch", None, Status::Ready, "").unwrap();
        let reference = || {
            store
                .view()
                .unwrap()
                .rows(
                    corral_core::backend::TaskNum(1),
                    &RowQuery::part(TaskPart::PaneRef),
                    PageReq::first(10),
                )
                .unwrap()
                .items
        };
        let launch = |socket: &str| {
            serde_json::from_value(json!({
                "socket":socket, "stage":"reserved", "reserved":true
            }))
            .unwrap()
        };

        // When the full writer reserves ownership, then the head writer changes and clears it.
        store
            .update(&task.id, |task| {
                task.launch = Some(launch("/fixture/owner.sock"));
                Ok(())
            })
            .unwrap();
        let rows = reference();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].doc["socket"], "/fixture/owner.sock");
        let owners = || {
            store
                .view()
                .unwrap()
                .aggregates(&[AggregateName::PaneOwners])
                .unwrap()
                .pane_owners
        };
        assert_eq!(
            owners(),
            [corral_core::backend::PaneOwner {
                socket: "/fixture/owner.sock".into(),
                n: 1,
            }]
        );
        store
            .update_head(&task.id, |head| {
                head.launch = Some(launch("/fixture/rebound.sock"));
                Ok(())
            })
            .unwrap();
        let rows = reference();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].doc["socket"], "/fixture/rebound.sock");
        assert_eq!(
            owners(),
            [corral_core::backend::PaneOwner {
                socket: "/fixture/rebound.sock".into(),
                n: 1,
            }]
        );
        store
            .update_head(&task.id, |head| {
                head.launch = None;
                Ok(())
            })
            .unwrap();

        // Then no reservation row survives without its corresponding launch field.
        assert!(reference().is_empty());
        assert!(owners().is_empty());
        assert!(store.get(&task.id).unwrap().launch.is_none());
    }
}
