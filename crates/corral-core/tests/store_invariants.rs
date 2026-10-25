//! Store invariants found by the T-320 round-1 review: CAS across a read-modify-write callback,
//! the store gate held by escaping views, legacy refusal on init, snapshot-bound views,
//! bounded mutations and coherent heads.

mod store_support;

use corral_core::backend::{Operation, StoreBackend, TaskChange, TaskNum, UpdateForm, UpdateTask};
use corral_core::sqlite::{SqliteBackend, SqliteOptions};
use corral_core::{Status, Store};
use store_support::*;

/// Commits a title and rounds change through a backend that holds no lease.
fn bypass(backend: &SqliteBackend) {
    let head = backend.view().unwrap().head(TaskNum(1)).unwrap().unwrap();
    let mut fields = head.header.fields.clone();
    fields.title = "unleased change".into();
    let mut doc = head.doc;
    doc["spec"]["title"] = "unleased change".into();
    doc["state"]["rounds"] = 41_u32.into();
    backend
        .apply(Operation::UpdateTask(UpdateTask {
            change: TaskChange {
                num: TaskNum(1),
                expected_rev: head.header.rev,
                fields,
                doc,
                summary: head.summary,
                rows: Vec::new(),
                events: Vec::new(),
                records: Vec::new(),
                aggregates: Vec::new(),
                reservations: Vec::new(),
            },
            form: UpdateForm::Head,
        }))
        .unwrap();
}

#[test]
fn a_commit_during_the_callback_is_refused_never_overwritten() {
    for entry in ["update", "try_update", "dispatch_update"] {
        // Given a task read by a leased writer.
        let (_dir, store) = store();
        store.create("original", None, Status::Backlog, "").unwrap();
        let other = SqliteBackend::open(store.dir(), SqliteOptions::default()).unwrap();
        // When a writer that holds no lease commits while the callback runs.
        let result = match entry {
            "update" => store
                .update("T-001", |task| {
                    bypass(&other);
                    task.rounds += 1;
                    Ok(())
                })
                .map(drop),
            "try_update" => store
                .try_update("T-001", |task| {
                    bypass(&other);
                    task.rounds += 1;
                    Ok(true)
                })
                .map(drop),
            _ => store
                .dispatch_update("T-001", |_, task| {
                    bypass(&other);
                    task.rounds += 1;
                    Ok(())
                })
                .map(drop),
        };
        // Then the leased write is refused and the other commit stands unchanged.
        let error = format!("{:#}", result.unwrap_err());
        assert!(
            error.contains("concurrent write refused"),
            "{entry}: {error}"
        );
        let saved = store.get("T-001").unwrap();
        assert_eq!(
            (saved.title.as_str(), saved.rounds),
            ("unleased change", 41),
            "{entry}"
        );
    }
}

#[test]
fn every_escaping_view_holds_the_store_gate_until_it_closes() {
    for escape in ["view", "head", "heads", "board"] {
        // Given a view that escapes the last Store.
        let (dir, store) = store();
        store.create("retained", None, Status::Backlog, "").unwrap();
        let corral = dir.path().join(".corral");
        let held: Box<dyn std::any::Any> = match escape {
            "view" => Box::new(store.view().unwrap()),
            "head" => Box::new(store.head("T-001").unwrap()),
            "heads" => Box::new(
                store
                    .heads(&corral_core::backend::HeadQuery::Open { project: None })
                    .unwrap(),
            ),
            _ => Box::new(store.board(None).unwrap()),
        };
        drop(store);
        // Then maintenance cannot take the gate exclusively while it lives.
        assert!(
            corral_core::try_exclusive_store_gate(&corral)
                .unwrap()
                .is_none(),
            "{escape}"
        );
        assert!(
            !corral_core::store_holders(&corral).unwrap().is_empty(),
            "{escape}"
        );
        // And once it closes the gate is free and no holder remains.
        drop(held);
        assert!(
            corral_core::try_exclusive_store_gate(&corral)
                .unwrap()
                .is_some(),
            "{escape}"
        );
        assert!(
            corral_core::store_holders(&corral).unwrap().is_empty(),
            "{escape}"
        );
    }
}

#[test]
fn init_refuses_a_legacy_board_marked_or_not_and_preserves_it() {
    for marker in [None, Some("3\n")] {
        // Given a legacy board with one task file, with or without its schema marker.
        let dir = tempfile::tempdir().unwrap();
        let corral = dir.path().join(".corral");
        std::fs::create_dir_all(corral.join("tasks")).unwrap();
        let task = corral.join("tasks/T-001-legacy.md");
        let text = "---\nid: T-001\ntitle: Legacy\nstatus: backlog\ncreated: 2026-01-01T00:00:00Z\nupdated: 2026-01-01T00:00:00Z\n---\nCanonical legacy task\n";
        std::fs::write(&task, text).unwrap();
        if let Some(marker) = marker {
            std::fs::create_dir_all(corral.join("state")).unwrap();
            std::fs::write(corral.join("state/schema-version"), marker).unwrap();
        }
        // When init runs.
        let error = format!("{:#}", Store::init(dir.path()).unwrap_err());
        // Then it refuses naming migration and leaves the board as it was.
        assert!(error.contains("corral migrate"), "{marker:?}: {error}");
        assert_eq!(std::fs::read_to_string(&task).unwrap(), text);
        assert!(
            !corral.join("db").exists(),
            "{marker:?}: no database is created"
        );
        assert_eq!(
            std::fs::read_to_string(corral.join("state/schema-version"))
                .ok()
                .as_deref(),
            marker,
            "the marker is unchanged"
        );
    }
}

fn with_run(store: &Store) -> corral_core::Task {
    let mut task = store
        .create("run fixture", None, Status::Running, "")
        .unwrap();
    task.runs = vec![coding_run("T-001", "R0001", 10, 1, 1, false)];
    (task.created, task.updated) = (at(0), at(0));
    store.fixture_put_task(&task).unwrap();
    store.get("T-001").unwrap()
}

#[test]
fn setting_a_new_cursor_reads_no_earlier_bindings() {
    use corral_core::inbox::{ObserveCursor, Recipient, SessionBinding, SessionCursor};
    use corral_core::work::{self, Counter, HotScope};
    let cursor = |session: &str| SessionCursor {
        adapter: "pi".into(),
        binding: SessionBinding {
            session: session.into(),
            endpoint: None,
            transcript: None,
        },
        cursor: ObserveCursor::default(),
    };
    let mut decodes = Vec::new();
    for earlier in [10, 100] {
        // Given a current run with `earlier` observe cursors.
        let (_dir, store) = store();
        let mut task = with_run(&store);
        task.runs[0].inbox.observe_cursors =
            (0..earlier).map(|i| cursor(&format!("old-{i}"))).collect();
        store.fixture_put_task(&task).unwrap();
        // When one new binding's cursor is set.
        let before = work::snapshot();
        {
            let _hot = HotScope::enter();
            store
                .update_head("T-001", |h| {
                    h.set_observe_cursor(
                        &Recipient::Run {
                            task: "T-001".into(),
                            run: "R0001".into(),
                        },
                        cursor("new"),
                    )
                })
                .unwrap();
        }
        decodes.push(
            work::snapshot()
                .since(&before)
                .total(Counter::NestedRowDecodes),
        );
        let stored = store.get("T-001").unwrap().runs[0]
            .inbox
            .observe_cursors
            .clone();
        assert_eq!(stored.len(), earlier + 1);
        assert_eq!(stored.last().unwrap().binding.session, "new");
    }
    // Then the work does not grow with the earlier bindings.
    assert_eq!(decodes[0], decodes[1], "{decodes:?}");
}

fn child_revs(store: &Store) -> (u64, u64) {
    let run = store.head("T-001").unwrap().current_run.unwrap();
    let child = run.current_child.unwrap();
    (run.facts.children_rev, child.facts.obligations_rev)
}

fn edit_child_wait(store: &Store, detail: &str) {
    let detail = detail.to_owned();
    store
        .update_head("T-001", |h| {
            h.edit_current_review_child("R0001", |child| {
                child.obligations.waits[0].observation.detail = detail;
                Ok(())
            })
        })
        .unwrap();
}

#[test]
fn keyed_child_edits_carry_and_advance_the_child_revisions() {
    // Given a current run whose current review child has waits.
    let (_dir, store) = store();
    with_run(&store);
    let (parent0, child0) = child_revs(&store);
    // When one wait changes A -> B -> A.
    edit_child_wait(&store, "B");
    assert_eq!(child_revs(&store), (parent0 + 1, child0 + 1));
    edit_child_wait(&store, "observed");
    assert_eq!(child_revs(&store), (parent0 + 2, child0 + 2));
    // Then an unchanged edit advances neither and writes nothing.
    let seq = store.view().unwrap().seq();
    store
        .update_head("T-001", |h| {
            h.edit_current_review_child("R0001", |_| Ok(()))
        })
        .unwrap();
    assert_eq!(child_revs(&store), (parent0 + 2, child0 + 2));
    assert_eq!(store.view().unwrap().seq(), seq);
    // And a usage edit of the child keeps its obligations revision.
    let child = store
        .head("T-001")
        .unwrap()
        .current_run
        .unwrap()
        .current_child
        .unwrap()
        .id;
    let entry: corral_core::UsageEntry = serde_json::from_value(usage_entry(9, true)).unwrap();
    store
        .update_head("T-001", |h| {
            h.append_usage_entries("R0001", Some(&child), vec![entry])
        })
        .unwrap();
    assert_eq!(child_revs(&store), (parent0 + 3, child0 + 2));
    let stored = store.get("T-001").unwrap();
    assert_eq!(
        stored.runs[0].review_runs[0].obligations.waits[0]
            .observation
            .detail,
        "observed"
    );
}

type Step<'a> = Box<dyn Fn(&mut corral_core::TaskHeadMut) -> anyhow::Result<()> + 'a>;
type Edit = fn(&mut corral_core::TaskHeadMut);

#[test]
fn byte_identical_child_mutations_write_nothing() {
    use corral_core::inbox::Recipient;
    let (_dir, store) = store();
    with_run(&store);
    let recipient = Recipient::Run {
        task: "T-001".into(),
        run: "R0001".into(),
    };
    store
        .update_head("T-001", |h| h.mark_effect_applied(&recipient, 7))
        .unwrap();
    let cursor = store.get("T-001").unwrap().runs[0].inbox.observe_cursors[0].clone();
    let steps: Vec<(&str, Step<'_>)> = vec![
        (
            "edit_wait",
            Box::new(|h| h.edit_wait("R0001", "W0000", |_| Ok(()))),
        ),
        (
            "edit_decision",
            Box::new(|h| h.edit_decision("R0001", "D0000", |_| Ok(()))),
        ),
        (
            "mark_effect_applied",
            Box::new(|h| h.mark_effect_applied(&recipient, 7)),
        ),
        (
            "set_observe_cursor",
            Box::new(|h| h.set_observe_cursor(&recipient, cursor.clone())),
        ),
        (
            "edit_review_child",
            Box::new(|h| h.edit_current_review_child("R0001", |_| Ok(()))),
        ),
    ];
    for (name, step) in steps {
        let before = store.view().unwrap().seq();
        let facts = store.head("T-001").unwrap().current_run.unwrap().facts;
        store.update_head("T-001", |h| step(h)).unwrap();
        assert_eq!(store.view().unwrap().seq(), before, "{name} wrote");
        assert_eq!(
            store.head("T-001").unwrap().current_run.unwrap().facts,
            facts,
            "{name}"
        );
    }
}

#[test]
fn direct_projection_edits_cannot_change_an_identity() {
    let edits: Vec<(&str, Edit)> = vec![
        ("child id", |h| {
            h.current_run_mut()
                .unwrap()
                .current_child
                .as_deref_mut()
                .unwrap()
                .id = "renamed".into();
        }),
        ("child start", |h| {
            h.current_run_mut()
                .unwrap()
                .current_child
                .as_deref_mut()
                .unwrap()
                .started = at(999);
        }),
        ("child removed", |h| {
            h.current_run_mut().unwrap().current_child = None
        }),
        ("run id", |h| {
            h.current_run_mut().unwrap().id = "renamed".into()
        }),
        ("run start", |h| {
            h.current_run_mut().unwrap().started = at(999)
        }),
        ("plan run id", |h| {
            h.last_plan_run_mut().unwrap().id = "renamed".into()
        }),
        ("plan id", |h| {
            h.last_plan_mut().unwrap().id = "renamed".into()
        }),
        ("plan review id", |h| {
            h.last_plan_mut()
                .unwrap()
                .current_review
                .as_mut()
                .unwrap()
                .id = "renamed".into();
        }),
        ("attempt id", |h| {
            h.last_aggregate_attempt_mut().unwrap().id = "renamed".into()
        }),
        ("aggregate review id", |h| {
            h.last_aggregate_attempt_mut()
                .unwrap()
                .current_review
                .as_mut()
                .unwrap()
                .id = "renamed".into();
        }),
    ];
    for (name, edit) in edits {
        // Given a task with every current container.
        let (_dir, store) = store();
        let mut task = store.create("Rich", None, Status::Running, "").unwrap();
        grow(&mut task, 2, 2, 2, 1);
        store.fixture_put_task(&task).unwrap();
        let before = store.get("T-001").unwrap();
        let seq = store.view().unwrap().seq();
        // When a mutation edits an identity field of a projection directly.
        let result = store.update_head("T-001", |h| {
            edit(h);
            Ok(())
        });
        // Then it is refused with nothing written.
        assert!(result.is_err(), "{name} was accepted");
        assert_eq!(store.view().unwrap().seq(), seq, "{name}");
        assert_eq!(store.get("T-001").unwrap(), before, "{name}");
    }
}

#[test]
fn every_snapshot_read_entry_point_observes_the_snapshot_state() {
    // Given a snapshot taken before another write.
    let (_dir, store) = store();
    store.create("before", None, Status::Backlog, "").unwrap();
    let snapshot = store.read_only_snapshot().unwrap();
    let seq = snapshot.view().unwrap().seq();
    store
        .update("T-001", |task| {
            task.title = "after".into();
            Ok(())
        })
        .unwrap();
    // Then its raw view, feed and record reads all read the snapshot's state.
    let raw = snapshot.view().unwrap();
    assert_eq!(raw.seq(), seq);
    assert_eq!(
        raw.head(TaskNum(1)).unwrap().unwrap().header.fields.title,
        "before"
    );
    assert!(snapshot.changes_since(seq, 10).unwrap().entries.is_empty());
    assert!(
        snapshot.task_record("T-001").is_err(),
        "recovery records are live only"
    );
    assert!(
        snapshot.task_records().is_err(),
        "recovery records are live only"
    );
    assert_eq!(store.task_record("T-001").unwrap().task.title, "after");
}

#[test]
fn an_expired_snapshot_refuses_its_cached_listing() {
    use corral_core::backend::fake::FakeBackend;
    use corral_core::backend::{BackendConfig, FaultInjector, ManualClock};
    use std::sync::Arc;
    // Given a snapshot on a manual clock whose listing is cached.
    let (_dir, base) = store();
    let clock = Arc::new(ManualClock::new(at(0)));
    let fake = FakeBackend::new(
        BackendConfig::default(),
        clock.clone(),
        Arc::new(FaultInjector::default()),
    );
    let store = base.with_backend(Box::new(fake));
    store.create("before", None, Status::Backlog, "").unwrap();
    let snapshot = store.read_only_snapshot().unwrap();
    assert_eq!(snapshot.list_shared().unwrap().len(), 1);
    // When the view outlives view_max_age.
    clock.advance(std::time::Duration::from_secs(121));
    // Then every read refuses, the cached listing included.
    assert!(snapshot.head("T-001").is_err());
    assert!(snapshot.list_shared().is_err());
    assert!(snapshot.list().is_err());
    assert!(snapshot.task_view().is_err());
}

fn rich_store() -> (tempfile::TempDir, Store) {
    let (dir, store) = store();
    let mut task = store.create("Rich", None, Status::Running, "").unwrap();
    grow(&mut task, 2, 2, 2, 1);
    store.fixture_put_task(&task).unwrap();
    (dir, store)
}

fn new_run(id: &str) -> corral_core::Run {
    let mut run: corral_core::Run =
        serde_json::from_value(review_child("T-001", "R0002", 9, 1, false, "approve")).unwrap();
    run.id = id.into();
    run
}

#[test]
fn a_mutation_reads_the_children_it_queued() {
    // A child pushed and edited twice in one operation.
    let (_dir, store) = rich_store();
    store
        .update_head("T-001", |h| {
            h.push_review_child("R0002", new_run("C-new"))?;
            h.edit_current_review_child("R0002", |c| {
                c.summary = Some("first".into());
                Ok(())
            })?;
            h.edit_review_child("R0002", "C-new", |c| {
                c.blocked_excerpt = Some("second".into());
                Ok(())
            })
        })
        .unwrap();
    let child = store.get("T-001").unwrap().runs[1]
        .review_runs
        .last()
        .unwrap()
        .clone();
    assert_eq!(child.id, "C-new");
    assert_eq!(child.summary.as_deref(), Some("first"));
    assert_eq!(child.blocked_excerpt.as_deref(), Some("second"));

    // A plan review pushed, then edited and terminalized in one operation.
    let (_dir, store) = rich_store();
    let review: corral_core::PlanReview =
        serde_json::from_value(plan_review("PR-new", "rev", None)).unwrap();
    store
        .update_head("T-001", |h| {
            h.terminalize_pending_plan_reviews("P0002", "before the new round")?;
            h.push_plan_review("P0002", review.clone())?;
            h.edit_plan_review("P0002", "PR-new", |r| {
                r.policy_digest = "edited".into();
                Ok(())
            })?;
            h.terminalize_pending_plan_reviews("P0002", "superseded")
        })
        .unwrap();
    let plan = store.get("T-001").unwrap().plans.last().unwrap().clone();
    let pushed = plan.reviews.iter().find(|r| r.id == "PR-new").unwrap();
    assert_eq!(pushed.policy_digest, "edited");
    assert!(
        pushed.verdict.is_some(),
        "the queued review was terminalized"
    );
    assert_eq!(pushed.notes.as_deref(), Some("superseded"));
    assert!(
        store
            .head("T-001")
            .unwrap()
            .pending_plan_reviews("P0002")
            .unwrap()
            .is_empty()
    );

    // An aggregate review pushed and superseded in one operation.
    let (_dir, store) = rich_store();
    let review: corral_core::AggregateReview =
        serde_json::from_value(aggregate_review("A0002-R9", true)).unwrap();
    store
        .update_head("T-001", |h| {
            h.supersede_pending_aggregate_reviews("A0002", "A0002-R9")?;
            h.push_aggregate_review("A0002", review.clone())?;
            h.edit_aggregate_review("A0002", "A0002-R9", |r| {
                r.evidence_digest = "edited".into();
                Ok(())
            })?;
            h.supersede_pending_aggregate_reviews("A0002", "later")
        })
        .unwrap();
    let head = store.head("T-001").unwrap();
    assert!(head.pending_aggregate_reviews("A0002").unwrap().is_empty());
    let pushed = head.aggregate_review("A0002", "A0002-R9").unwrap().unwrap();
    assert_eq!(pushed.evidence_digest, "edited");

    // A plan review pushed and popped in one operation leaves the task as it was.
    let (_dir, store) = rich_store();
    let before = store.get("T-001").unwrap();
    let review: corral_core::PlanReview =
        serde_json::from_value(plan_review("PR-pop", "rev", None)).unwrap();
    store
        .update_head("T-001", |h| {
            h.push_plan_review("P0002", review.clone())?;
            h.pop_current_plan_review("P0002")
        })
        .unwrap();
    let mut after = store.get("T-001").unwrap();
    after.updated = before.updated;
    assert_eq!(after, before);
}

#[test]
fn the_returned_head_reads_the_state_it_describes() {
    let (_dir, store) = rich_store();
    let comments = store.get("T-001").unwrap().comments.len();
    let head = store
        .update_head("T-001", |h| {
            h.add_comment(corral_core::TaskComment {
                author: corral_core::CommentAuthor::Owner,
                at: at(5),
                text: "new comment".into(),
            })
        })
        .unwrap();
    assert_eq!(head.summary.comments as usize, comments + 1);
    let read = head.emitted_comments().unwrap();
    assert_eq!(read.len(), comments + 1);
    assert_eq!(read.last().unwrap().text, "new comment");
    assert_eq!(head, store.head("T-001").unwrap());
}

fn counted(f: impl FnOnce()) -> corral_core::work::WorkSnapshot {
    use corral_core::work::{self, HotScope};
    let before = work::snapshot();
    {
        let _hot = HotScope::enter();
        f();
    }
    let delta = work::snapshot().since(&before);
    delta.check_identity().unwrap();
    delta
}

#[test]
fn every_actual_decode_and_assembly_is_counted_once() {
    use corral_core::backend::HeadQuery;
    use corral_core::work::Counter;
    let (_dir, store) = rich_store();
    // A board read counts the same head documents as a heads read of the same task.
    let heads = counted(|| {
        store.heads(&HeadQuery::Open { project: None }).unwrap();
    });
    let board = counted(|| {
        assert_eq!(store.board(None).unwrap().heads.len(), 1);
    });
    assert!(board.get(Counter::TaskHeadDecodes) > 0);
    assert_eq!(
        board.get(Counter::TaskHeadDecodes),
        heads.get(Counter::TaskHeadDecodes)
    );
    assert_eq!(board.decoded_doc_bytes(), heads.decoded_doc_bytes());
    let since = store.view().unwrap().seq();
    store
        .update_head("T-001", |h| {
            h.title = "changed".into();
            Ok(())
        })
        .unwrap();
    let delta = counted(|| {
        assert_eq!(store.board(Some(since)).unwrap().heads.len(), 1);
    });
    assert_eq!(
        delta.get(Counter::TaskHeadDecodes),
        board.get(Counter::TaskHeadDecodes)
    );
    // Each full read-modify-write and record read assembles the task once, and counts it.
    for (name, expected, delta) in [
        (
            "update",
            1,
            counted(|| {
                store
                    .update("T-001", |t| {
                        t.rounds += 1;
                        Ok(())
                    })
                    .unwrap();
            }),
        ),
        (
            "get then save",
            2,
            counted(|| {
                store.save(&store.get("T-001").unwrap()).unwrap();
            }),
        ),
        (
            "task_record",
            1,
            counted(|| {
                store.task_record("T-001").unwrap();
            }),
        ),
    ] {
        assert_eq!(delta.full_task_hydrations(), expected, "{name}");
        assert_eq!(delta.unattributed_hydrations(), expected, "{name}");
    }
}

fn payloads(store: &Store) -> Vec<String> {
    store
        .view()
        .unwrap()
        .task(TaskNum(1), &corral_core::backend::TaskParts::All)
        .unwrap()
        .unwrap()
        .rows
        .into_iter()
        .filter(|row| row.part == corral_core::backend::TaskPart::CheckpointPayload)
        .map(|row| row.key)
        .collect()
}

fn clear_child_checkpoints(store: &Store) {
    store
        .update_head("T-001", |h| {
            h.edit_current_review_child("R0001", |child| {
                child.checkpoints.clear();
                Ok(())
            })
        })
        .unwrap();
}

#[test]
fn a_checkpoint_payload_is_deleted_with_its_last_reference() {
    // Given a review child whose checkpoint payload only it references.
    let (_dir, store) = store();
    let task = with_run(&store);
    let child = &task.runs[0].review_runs[0];
    let own = |task: &corral_core::Task| -> Vec<String> {
        task.runs[0]
            .checkpoints
            .iter()
            .map(|c| c.artifact.clone())
            .collect()
    };
    assert!(!child.checkpoints.is_empty());
    let before = payloads(&store);
    // When its checkpoints are cleared through a keyed edit, its payload goes with them.
    clear_child_checkpoints(&store);
    let after = payloads(&store);
    assert!(after.len() < before.len(), "{before:?} -> {after:?}");
    assert_eq!(
        store.get("T-001").unwrap().runs[0].review_runs[0]
            .checkpoints
            .len(),
        0
    );
    assert_eq!(own(&store.get("T-001").unwrap()), own(&task));

    // Given a payload the run and its child share.
    let (_dir, store) = store_support::store();
    let mut task = with_run(&store);
    let shared = task.runs[0].review_runs[0].checkpoints[0].clone();
    task.runs[0].checkpoints = vec![shared];
    store.fixture_put_task(&task).unwrap();
    let shared_payloads = payloads(&store);
    // When the child's reference goes, the payload stays for the run's.
    clear_child_checkpoints(&store);
    assert_eq!(payloads(&store), shared_payloads);
    let stored = store.get("T-001").unwrap();
    assert_eq!(stored.runs[0].checkpoints, task.runs[0].checkpoints);
    // And when the last reference goes, so does the payload.
    store
        .update("T-001", |t| {
            t.runs[0].checkpoints.clear();
            Ok(())
        })
        .unwrap();
    assert!(payloads(&store).is_empty(), "{:?}", payloads(&store));
}

#[test]
fn task_records_reads_every_record_with_its_recoveries() {
    let (_dir, store) = store();
    for title in ["one", "two", "three"] {
        store.create(title, None, Status::Backlog, "").unwrap();
    }
    let all = store.task_records().unwrap();
    let ids: Vec<&str> = all.iter().map(|r| r.task.id.as_str()).collect();
    assert_eq!(ids, ["T-001", "T-002", "T-003"]);
    for record in &all {
        assert_eq!(record, &store.task_record(&record.task.id).unwrap());
    }
}
