//! T-321: callback events are part of the task transaction, not a second append.

mod store_support;

use std::sync::Arc;
use std::time::Duration;

use corral_core::backend::{ChangeScope, FaultInjector, TaskNum};
use corral_core::sqlite::{SqliteBackend, SqliteOptions};
use corral_core::{Event, Status, Store};
use serde_json::json;
use store_support::*;

fn event(store: &Store, id: &str, marker: &str) -> anyhow::Result<()> {
    store.log_event(
        Some(id),
        "acceptance",
        json!({"run": "R1", "marker": marker}),
    )
}

#[test]
fn a_successful_event_only_callback_commits_even_without_a_task_field_change() {
    for head in [false, true] {
        let (_dir, store) = store();
        let task = store
            .create("Event only", None, Status::Backlog, "")
            .unwrap();
        let seq = store.view().unwrap().seq();
        if head {
            store
                .update_head(&task.id, |_| event(&store, &task.id, "event-only"))
                .unwrap();
        } else {
            store
                .update(&task.id, |_| event(&store, &task.id, "event-only"))
                .unwrap();
        }
        assert_eq!(store.get(&task.id).unwrap(), task);
        let events = store.events_of(Some(&task.id), &["acceptance"]).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].detail["marker"], "event-only");
        assert_eq!(store.changes_since(seq, 10).unwrap().entries.len(), 1);
    }
}

fn edit(store: &Store, id: &str, head: bool) -> anyhow::Result<()> {
    if head {
        store.update_head(id, |task| {
            task.rounds += 1;
            event(store, id, "committed")
        })?;
    } else {
        store.update(id, |task| {
            task.rounds += 1;
            event(store, id, "committed")
        })?;
    }
    Ok(())
}

#[test]
fn legacy_event_serde_defaults_sequence_and_preserves_explicit_sequences() {
    let legacy = json!({
        "ts": T0, "task": "T-001", "kind": "done", "detail": {"run": "R1"}
    });
    let event: Event = serde_json::from_value(legacy.clone()).unwrap();
    assert_eq!(event.seq, 0);
    let mut sequenced = legacy;
    sequenced["seq"] = json!(42);
    let event: Event = serde_json::from_value(sequenced).unwrap();
    assert_eq!(event.seq, 42);
    assert_eq!(
        serde_json::from_value::<Event>(serde_json::to_value(&event).unwrap()).unwrap(),
        event
    );
}

#[test]
fn successful_full_and_head_callbacks_publish_task_and_events_in_one_change() {
    for head in [false, true] {
        let (dir, store) = store();
        let task = store.create("Atomic", None, Status::Backlog, "").unwrap();
        let old_events = store.events().unwrap();
        let seq = store.view().unwrap().seq();
        let before = store.read_only_snapshot().unwrap();
        edit(&store, &task.id, head).unwrap();

        let page = store.changes_since(seq, 10).unwrap();
        assert_eq!(page.entries.len(), 1, "head={head}: {page:?}");
        assert!(page.entries[0].scopes.contains(&ChangeScope::Task {
            num: TaskNum(1),
            parts: vec![],
        }));
        assert!(page.entries[0].scopes.contains(&ChangeScope::Events {
            task: Some(task.id.clone()),
            kind: "acceptance".into(),
        }));
        assert_eq!(before.get(&task.id).unwrap(), task);
        assert_eq!(before.events().unwrap(), old_events);
        let fresh = Store::open(dir.path()).unwrap();
        assert_eq!(fresh.get(&task.id).unwrap().rounds, task.rounds + 1);
        let events = fresh.events_of(Some(&task.id), &["acceptance"]).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].detail["marker"], "committed");
        assert!(events[0].seq > old_events.last().unwrap().seq);
        assert!(!dir.path().join(".corral/events.jsonl").exists());
    }
}

#[test]
fn errors_and_false_callbacks_discard_events_and_leave_no_buffer_for_later_writes() {
    for head in [false, true] {
        for returns_false in [false, true] {
            let (_dir, store) = store();
            let task = store.create("Abort", None, Status::Backlog, "").unwrap();
            let old_events = store.events().unwrap();
            let seq = store.view().unwrap().seq();
            let callback = || -> anyhow::Result<bool> {
                event(&store, &task.id, "discard")?;
                if returns_false {
                    Ok(false)
                } else {
                    anyhow::bail!("callback rejected")
                }
            };
            if head {
                let result = store.try_update_head(&task.id, |h| {
                    h.rounds += 1;
                    callback()
                });
                if returns_false {
                    assert!(result.unwrap().is_none());
                } else {
                    assert!(result.is_err());
                }
            } else {
                let result = store.try_update(&task.id, |t| {
                    t.rounds += 1;
                    callback()
                });
                if returns_false {
                    assert!(result.unwrap().is_none());
                } else {
                    assert!(result.is_err());
                }
            }
            assert_eq!(store.get(&task.id).unwrap(), task);
            assert_eq!(store.events().unwrap(), old_events);
            assert_eq!(store.view().unwrap().seq(), seq);
            event(&store, &task.id, "later").unwrap();
            let events = store.events_of(Some(&task.id), &["acceptance"]).unwrap();
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].detail["marker"], "later");
        }
    }
}

#[test]
fn dispatch_callbacks_buffer_events_from_the_fresh_store_and_discard_on_error() {
    for head in [false, true] {
        for succeeds in [false, true] {
            let (_dir, store) = store();
            let task = store.create("Dispatch", None, Status::Backlog, "").unwrap();
            let seq = store.view().unwrap().seq();
            let log = |fresh: &Store| -> anyhow::Result<u32> {
                event(fresh, &task.id, "dispatch")?;
                if succeeds {
                    Ok(17)
                } else {
                    anyhow::bail!("dispatch rejected")
                }
            };
            let result = if head {
                store
                    .dispatch_update_head(&task.id, |fresh, h| {
                        h.rounds += 1;
                        log(fresh)
                    })
                    .map(|(_, value)| value)
            } else {
                store
                    .dispatch_update(&task.id, |fresh, t| {
                        t.rounds += 1;
                        log(fresh)
                    })
                    .map(|(_, value)| value)
            };
            if succeeds {
                assert_eq!(result.unwrap(), 17);
                assert_eq!(store.changes_since(seq, 10).unwrap().entries.len(), 1);
                assert_eq!(
                    store
                        .events_of(Some(&task.id), &["acceptance"])
                        .unwrap()
                        .len(),
                    1
                );
            } else {
                assert!(result.is_err());
                assert_eq!(store.view().unwrap().seq(), seq);
                let events = store.events_of(Some(&task.id), &["acceptance"]).unwrap();
                assert!(events.is_empty());
                assert_eq!(store.get(&task.id).unwrap(), task);
            }
        }
    }
}

#[test]
fn reopened_handles_on_the_same_thread_join_the_callback_buffer() {
    for head in [false, true] {
        for succeeds in [false, true] {
            let (dir, store) = store();
            let task = store.create("Reopen", None, Status::Backlog, "").unwrap();
            let other = Store::open(dir.path()).unwrap();
            let old_events = other.events().unwrap();
            let callback = || -> anyhow::Result<()> {
                // Both an existing independent handle and a new opener must join.
                event(&other, &task.id, "existing")?;
                event(&Store::open(dir.path()).unwrap(), &task.id, "reopened")?;
                assert_eq!(other.events().unwrap(), old_events, "no early publication");
                if succeeds {
                    Ok(())
                } else {
                    anyhow::bail!("abort reopened events")
                }
            };
            let result = if head {
                store
                    .update_head(&task.id, |h| {
                        h.rounds += 1;
                        callback()
                    })
                    .map(|_| ())
            } else {
                store
                    .update(&task.id, |t| {
                        t.rounds += 1;
                        callback()
                    })
                    .map(|_| ())
            };
            assert_eq!(result.is_ok(), succeeds);
            let events = other.events_of(Some(&task.id), &["acceptance"]).unwrap();
            let markers: Vec<_> = events
                .iter()
                .map(|e| e.detail["marker"].as_str().unwrap())
                .collect();
            assert_eq!(
                markers,
                if succeeds {
                    vec!["existing", "reopened"]
                } else {
                    vec![]
                }
            );
        }
    }
}

#[test]
fn another_thread_and_another_database_do_not_join_an_aborted_buffer() {
    let (dir, store) = store();
    let (_other_dir, independent) = store_support::store();
    let task = store.create("Thread", None, Status::Backlog, "").unwrap();
    let result = store.update(&task.id, |t| {
        t.rounds += 1;
        event(&store, &task.id, "discard")?;
        let (done, completed) = std::sync::mpsc::channel();
        let root = dir.path().to_path_buf();
        let id = task.id.clone();
        let writer = std::thread::spawn(move || {
            let result = Store::open(&root).and_then(|other| event(&other, &id, "thread"));
            done.send(result).unwrap();
        });
        completed.recv_timeout(Duration::from_secs(30)).unwrap()?;
        writer.join().unwrap();
        event(&independent, &task.id, "other-db")?;
        anyhow::bail!("abort only this thread/database")
    });
    assert!(result.is_err());
    assert_eq!(store.get(&task.id).unwrap(), task);
    let events = store.events_of(Some(&task.id), &["acceptance"]).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].detail["marker"], "thread");
    let events = independent
        .events_of(Some(&task.id), &["acceptance"])
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].detail["marker"], "other-db");
}

fn fault_store() -> (tempfile::TempDir, Store, Arc<FaultInjector>, String) {
    let (dir, base) = store();
    let faults = Arc::new(FaultInjector::default());
    let backend = SqliteBackend::open(
        base.dir(),
        SqliteOptions {
            faults: Arc::clone(&faults),
            ..SqliteOptions::default()
        },
    )
    .unwrap();
    let store = base.with_backend(Box::new(backend));
    let task = store.create("Fault", None, Status::Backlog, "").unwrap();
    (dir, store, faults, task.id)
}

#[test]
fn every_primitive_write_failure_rolls_back_task_events_and_change_feed() {
    for head in [false, true] {
        let (_dir, store, faults, id) = fault_store();
        faults.reset_count();
        edit(&store, &id, head).unwrap();
        let writes = faults.writes();
        assert!(writes > 0);
        for nth in 1..=writes {
            let (_dir, store, faults, id) = fault_store();
            let before = store.get(&id).unwrap();
            let events = store.events().unwrap();
            let seq = store.view().unwrap().seq();
            faults.arm(nth);
            let result = edit(&store, &id, head);
            assert!(result.is_err(), "head={head}, write={nth}/{writes}");
            faults.disarm();
            assert_eq!(store.get(&id).unwrap(), before);
            assert_eq!(store.events().unwrap(), events);
            assert_eq!(store.view().unwrap().seq(), seq);
            edit(&store, &id, head).unwrap();
            assert_eq!(
                store.events_of(Some(&id), &["acceptance"]).unwrap().len(),
                1
            );
        }
    }
}

#[test]
fn snapshots_keep_task_and_all_event_readers_on_the_same_commit() {
    let (dir, store) = store();
    let task = store.create("Snapshot", None, Status::Backlog, "").unwrap();
    event(&store, &task.id, "before").unwrap();
    let snapshot = store.read_only_snapshot().unwrap();
    // Do not read events until after the writer: the view, not a lazy event cache, pins them.
    assert_eq!(snapshot.get(&task.id).unwrap(), task);
    let old_events = store.events().unwrap();
    let other = Store::open(dir.path()).unwrap();
    edit(&other, &task.id, true).unwrap();
    assert_eq!(snapshot.events().unwrap(), old_events);
    assert_eq!(*snapshot.events_shared().unwrap(), old_events);
    assert_eq!(snapshot.events_for(&task.id).unwrap(), old_events);
    let expected: Vec<_> = old_events
        .into_iter()
        .filter(|e| e.kind == "acceptance")
        .collect();
    assert_eq!(
        snapshot.events_of(Some(&task.id), &["acceptance"]).unwrap(),
        expected
    );
    assert_eq!(
        snapshot
            .events_page(Some(&task.id), &["acceptance"], Some("R1"), 0, 500)
            .unwrap(),
        expected
    );
    assert_eq!(
        snapshot
            .events_of_run(Some(&task.id), &["acceptance"], Some("R1"), 0, 500)
            .unwrap(),
        expected
    );
    let forbidden = snapshot.log_event(Some(&task.id), "forbidden", json!({}));
    assert!(forbidden.is_err());
    assert_eq!(snapshot.get(&task.id).unwrap(), task);
    let fresh = store.read_only_snapshot().unwrap();
    assert_eq!(
        fresh
            .events_of(Some(&task.id), &["acceptance"])
            .unwrap()
            .len(),
        2
    );
    assert_eq!(fresh.get(&task.id).unwrap().rounds, task.rounds + 1);
}

#[test]
fn owner_approval_is_not_consumed_by_a_failed_task_and_event_transaction() {
    for head in [false, true] {
        let (_dir, store, faults, _) = fault_store();
        let idea = store
            .create("Owner", None, Status::Idea, "Keep scope\n")
            .unwrap();
        store
            .owner_decision(&idea.id, "drop", "No longer needed", "flag")
            .unwrap();
        let events = store.events().unwrap();
        let drop_idea = || {
            if head {
                store
                    .update_head(&idea.id, |h| {
                        h.status = Status::Dropped;
                        Ok(())
                    })
                    .map(|_| ())
            } else {
                store
                    .update(&idea.id, |t| {
                        t.status = Status::Dropped;
                        Ok(())
                    })
                    .map(|_| ())
            }
        };
        faults.arm(1);
        assert!(drop_idea().is_err());
        faults.disarm();
        assert_eq!(store.get(&idea.id).unwrap(), idea);
        assert_eq!(store.events().unwrap(), events);
        // The durable approval still authorizes exactly one successful transition.
        drop_idea().unwrap();
        let audit = store.events_of(Some(&idea.id), &["idea_decided"]).unwrap();
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].detail["reason"], "No longer needed");
        assert_eq!(audit[0].detail["owner_ok"], true);
        store
            .update(&idea.id, |t| {
                t.status = Status::Idea;
                Ok(())
            })
            .unwrap();
        assert!(drop_idea().is_err());
    }
}

#[test]
fn owner_approval_logged_inside_the_callback_authorizes_only_its_committed_change() {
    for head in [false, true] {
        for succeeds in [false, true] {
            let (_dir, store) = store();
            let idea = store
                .create("Owner", None, Status::Idea, "Keep scope\n")
                .unwrap();
            let old = store.events().unwrap();
            let seq = store.view().unwrap().seq();
            let approve = || -> anyhow::Result<()> {
                store.log_event(
                    Some(&idea.id),
                    "owner_decision",
                    json!({
                        "actor":"owner", "action":"drop", "reason":"Approved inside callback",
                        "source":"flag", "revision":corral_core::owner_revision(&idea)?
                    }),
                )?;
                assert_eq!(
                    store.events().unwrap(),
                    old,
                    "staged approval is not published"
                );
                if !succeeds {
                    anyhow::bail!("abort owner move");
                }
                Ok(())
            };
            let result = if head {
                store
                    .update_head(&idea.id, |task| {
                        approve()?;
                        task.status = Status::Dropped;
                        Ok(())
                    })
                    .map(|_| ())
            } else {
                store
                    .update(&idea.id, |task| {
                        approve()?;
                        task.status = Status::Dropped;
                        Ok(())
                    })
                    .map(|_| ())
            };
            if succeeds {
                result.unwrap();
                assert_eq!(store.get(&idea.id).unwrap().status, Status::Dropped);
                assert_eq!(store.changes_since(seq, 10).unwrap().entries.len(), 1);
                let audit = store
                    .events_of(Some(&idea.id), &["owner_decision", "idea_decided"])
                    .unwrap();
                assert_eq!(audit.len(), 2);
                assert_eq!(audit[0].kind, "owner_decision");
                assert_eq!(audit[1].kind, "idea_decided");
                assert_eq!(audit[1].detail["decision_at"], audit[0].ts.to_string());
            } else {
                assert!(result.is_err());
                assert_eq!(store.get(&idea.id).unwrap(), idea);
                assert_eq!(store.events().unwrap(), old);
                assert_eq!(store.view().unwrap().seq(), seq);
            }
        }
    }
}
