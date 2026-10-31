//! T-321: indexed event selection, legacy import equivalence and bounded decode work.

mod store_support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use corral_core::Event;
use corral_core::backend::fake::FakeBackend;
use corral_core::backend::{
    AppendEvents, BackendConfig, EventQuery, FaultInjector, ImportBatch, ImportedEvent,
    ManualClock, NewEvent, Operation, PageReq, StoreBackend,
};
use corral_core::sqlite::{SqliteBackend, SqliteOptions};
use corral_core::work::{self, Counter, HotScope, WorkSnapshot};
use serde_json::{Value, json};
use store_support::*;

fn new_event(task: Option<&str>, kind: &str, detail: Value) -> NewEvent {
    NewEvent {
        ts: at(0),
        task: task.map(str::to_owned),
        kind: kind.into(),
        actor: None,
        via: Some("acceptance".into()),
        // Deliberately wrong: neither append nor import may trust this secondary column.
        run: Some("caller-lie".into()),
        detail,
    }
}

fn import(backend: &dyn StoreBackend, events: Vec<ImportedEvent>) {
    backend
        .apply(Operation::ImportBatch(ImportBatch {
            tasks: vec![],
            blobs: vec![],
            records: vec![],
            events,
        }))
        .unwrap();
}

fn imported(event: NewEvent) -> ImportedEvent {
    ImportedEvent {
        event,
        malformed: false,
    }
}

fn measured(f: impl FnOnce() -> Vec<Event>) -> (Vec<Event>, WorkSnapshot) {
    let before = work::snapshot();
    let events = {
        let _hot = HotScope::enter();
        f()
    };
    let delta = work::snapshot().since(&before);
    delta.check_identity().unwrap();
    assert_eq!(
        delta.get(Counter::EventDecodes),
        u64::try_from(events.len()).unwrap()
    );
    for counter in Counter::DECODES {
        if counter != Counter::EventDecodes {
            assert_eq!(delta.get(counter), 0, "{}", counter.name());
        }
    }
    assert_eq!(delta.full_task_hydrations(), 0);
    (events, delta)
}

#[test]
fn append_and_import_derive_run_only_from_string_detail_on_both_backends() {
    for fake in [false, true] {
        for bulk in [false, true] {
            let (_dir, base) = store();
            let backend: Box<dyn StoreBackend> = if fake {
                Box::new(FakeBackend::new(
                    BackendConfig::default(),
                    Arc::new(ManualClock::new(at(0))),
                    Arc::new(FaultInjector::default()),
                ))
            } else {
                Box::new(SqliteBackend::open(base.dir(), SqliteOptions::default()).unwrap())
            };
            let mut events: Vec<_> = [
                json!({"run": "R1"}),
                json!({}),
                json!({"run": null}),
                json!({"run": 7}),
                json!({"run": {"id": "R1"}}),
                json!({"run": ""}),
            ]
            .into_iter()
            .map(|detail| new_event(Some("T-001"), "run-test", detail))
            .collect();
            // A missing secondary column must also be populated from detail.
            let mut missing = new_event(Some("T-001"), "run-test", json!({"run": "R2"}));
            missing.run = None;
            events.push(missing);
            if bulk {
                import(backend.as_ref(), events.into_iter().map(imported).collect());
            } else {
                backend
                    .apply(Operation::AppendEvents(AppendEvents {
                        events,
                        reservations: vec![],
                    }))
                    .unwrap();
            }
            let store = base.with_backend(backend);
            let rows = store
                .view()
                .unwrap()
                .events(&EventQuery::default(), PageReq::first(100))
                .unwrap()
                .items;
            let runs: Vec<_> = rows.iter().map(|row| row.event.run.as_deref()).collect();
            assert_eq!(
                runs,
                [Some("R1"), None, None, None, None, Some(""), Some("R2")],
                "fake={fake}, import={bulk}"
            );
            assert_eq!(
                store
                    .events_of_run(Some("T-001"), &[], Some("R1"), 0, 100)
                    .unwrap()
                    .len(),
                1
            );
            assert_eq!(
                store
                    .events_of_run(None, &[], Some("R2"), 0, 100)
                    .unwrap()
                    .len(),
                1
            );
            assert_eq!(
                store
                    .events_of_run(None, &[], Some(""), 0, 100)
                    .unwrap()
                    .len(),
                1
            );
            assert!(
                store
                    .events_of_run(None, &[], Some("caller-lie"), 0, 100)
                    .unwrap()
                    .is_empty()
            );
        }
    }
}

fn reference(
    all: &[Event],
    task: Option<&str>,
    kinds: &[&str],
    run: Option<&str>,
    after: u64,
    limit: u32,
) -> Vec<Event> {
    all.iter()
        .filter(|e| e.seq > after)
        .filter(|e| task.is_none_or(|task| e.task.as_deref() == Some(task)))
        .filter(|e| kinds.is_empty() || kinds.contains(&e.kind.as_str()))
        .filter(|e| run.is_none_or(|run| e.detail.get("run").and_then(Value::as_str) == Some(run)))
        .take(usize::try_from(limit).unwrap())
        .cloned()
        .collect()
}

#[test]
fn deterministic_random_log_import_matches_filtering_and_cursor_pagination() {
    let (_dir, base) = store();
    let backend = SqliteBackend::open(base.dir(), SqliteOptions::default()).unwrap();
    let mut rng = Rng(0x321a_1cce_55ed);
    let mut expected = Vec::new();
    let mut batch = Vec::new();
    for n in 0..2300_u64 {
        let task = [None, Some("T-001"), Some("T-002"), Some("T-003")][rng.below(4)];
        let kind = ["done", "verdict", "owner_decision", "noise"][rng.below(4)];
        let detail = match rng.below(6) {
            0 => json!({"n": n}),
            1 => json!({"n": n, "run": null}),
            2 => json!({"n": n, "run": 1}),
            r => json!({"n": n, "run": format!("R{}", r - 2)}),
        };
        let mut event = new_event(task, kind, detail);
        // Sequence order must not accidentally be timestamp order.
        event.ts = at(i64::try_from(rng.below(100)).unwrap());
        expected.push(Event {
            seq: n + 1,
            ts: event.ts,
            task: event.task.clone(),
            kind: event.kind.clone(),
            actor: None,
            via: event.via.clone(),
            detail: event.detail.clone(),
        });
        batch.push(imported(event));
    }
    import(&backend, batch);
    let store = base.with_backend(Box::new(backend));
    assert_eq!(store.events().unwrap(), expected);
    for task in [None, Some("T-001"), Some("T-absent")] {
        for kinds in [
            &[][..],
            &["done"][..],
            &["done", "verdict"][..],
            &["missing"][..],
        ] {
            for run in [None, Some("R1"), Some("R3"), Some("missing")] {
                let after = u64::try_from(rng.below(2400)).unwrap();
                let limit = [1, 7, 37, 500][rng.below(4)];
                let want = reference(&expected, task, kinds, run, after, limit);
                assert_eq!(
                    store.events_page(task, kinds, run, after, limit).unwrap(),
                    want,
                    "task={task:?} kinds={kinds:?} run={run:?} after={after} limit={limit}"
                );
                assert_eq!(
                    store.events_of_run(task, kinds, run, after, limit).unwrap(),
                    want
                );
            }
            // The legacy kind-filter API keeps its empty-list-means-no-kinds semantics.
            let want = if kinds.is_empty() {
                vec![]
            } else {
                reference(&expected, task, kinds, None, 0, u32::MAX)
            };
            assert_eq!(store.events_of(task, kinds).unwrap(), want);
        }
    }
    assert_eq!(
        store.events_for("T-002").unwrap(),
        reference(&expected, Some("T-002"), &[], None, 0, u32::MAX)
    );
    for (task, kinds, run) in [
        (None, vec![], None),
        (Some("T-001"), vec!["done", "verdict"], Some("R2")),
    ] {
        let mut after = 0;
        let mut actual = Vec::new();
        loop {
            let page = store.events_page(task, &kinds, run, after, 37).unwrap();
            if page.is_empty() {
                break;
            }
            assert!(page.windows(2).all(|pair| pair[0].seq < pair[1].seq));
            assert!(page[0].seq > after);
            after = page.last().unwrap().seq;
            actual.extend(page);
        }
        assert_eq!(actual, reference(&expected, task, &kinds, run, 0, u32::MAX));
    }
    assert!(
        store
            .events_page(None, &[], None, u64::try_from(expected.len()).unwrap(), 500)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn malformed_import_rows_are_skipped_without_losing_later_valid_events() {
    let (_dir, base) = store();
    let backend = SqliteBackend::open(base.dir(), SqliteOptions::default()).unwrap();
    let batch = (0..1100)
        .map(|n| ImportedEvent {
            event: new_event(Some("T-001"), "import", json!({"n": n})),
            malformed: n % 2 == 0,
        })
        .collect();
    import(&backend, batch);
    let store = base.with_backend(Box::new(backend));
    let events = store.events().unwrap();
    assert_eq!(events.len(), 550);
    assert_eq!(events.first().unwrap().seq, 2);
    assert_eq!(events.last().unwrap().seq, 1100);
    assert_eq!(store.events_for("T-001").unwrap(), events);
    assert_eq!(store.events_of(None, &["import"]).unwrap(), events);
}

fn scaled_reads(unrelated: usize) -> Vec<(Vec<Event>, WorkSnapshot)> {
    let (_dir, base) = store();
    let backend = SqliteBackend::open(base.dir(), SqliteOptions::default()).unwrap();
    let target: Vec<_> = (0..1200)
        .map(|n| {
            imported(new_event(
                Some("T-001"),
                if n % 2 == 0 { "done" } else { "verdict" },
                json!({"run": if n % 3 == 0 { "R1" } else { "R2" }, "n": n}),
            ))
        })
        .collect();
    import(&backend, target);
    import(
        &backend,
        (0..32)
            .map(|n| {
                imported(new_event(
                    Some("T-003"),
                    "kept",
                    json!({"run": "R1", "n": n}),
                ))
            })
            .collect(),
    );
    // Noise shares individual indexed fields, but never all the requested filters.
    let noise = (0..unrelated)
        .map(|n| {
            let (task, kind, run) = match n % 3 {
                0 => ("T-002", "done", "R1"),
                1 => ("T-001", "noise", "R1"),
                _ => ("T-001", "done", "other-run"),
            };
            imported(new_event(Some(task), kind, json!({"run": run, "noise": n})))
        })
        .collect();
    import(&backend, noise);
    let store = base.with_backend(Box::new(backend));
    vec![
        measured(|| {
            store
                .events_page(Some("T-001"), &["done", "verdict"], Some("R1"), 101, 100)
                .unwrap()
        }),
        measured(|| {
            store
                .events_of_run(Some("T-001"), &["done"], Some("R1"), 101, 100)
                .unwrap()
        }),
        measured(|| store.events_of(Some("T-001"), &["verdict"]).unwrap()),
        measured(|| store.events_for("T-003").unwrap()),
        measured(|| {
            store
                .events_page(Some("T-001"), &["missing"], Some("R1"), 0, 500)
                .unwrap()
        }),
    ]
}

#[test]
fn tenfold_unrelated_history_does_not_change_output_or_decode_work() {
    let small = scaled_reads(1200);
    let grown = scaled_reads(12_000);
    for (((before, before_work), (after, after_work)), count) in
        small.into_iter().zip(grown).zip([100, 100, 600, 32, 0])
    {
        assert_eq!(before.len(), count);
        assert_eq!(after, before);
        assert_eq!(
            after_work, before_work,
            "unrelated history must not be decoded"
        );
    }
}

#[test]
fn owner_change_reads_only_its_approval_when_unrelated_history_grows_tenfold() {
    let check = |noise| {
        let (_dir, base) = store();
        let backend = SqliteBackend::open(base.dir(), SqliteOptions::default()).unwrap();
        let store = base.with_backend(Box::new(backend.handle().unwrap()));
        store.fixture_pin_now(at(0)).unwrap();
        let idea = store
            .create_initialized(
                "Owner idea",
                None,
                corral_core::Status::Idea,
                "Scope\n",
                |task| {
                    // Byte budgets must compare identical timestamp precision.
                    task.created = at(0);
                    Ok(())
                },
            )
            .unwrap();
        import(
            &backend,
            (0..noise)
                .map(|n| {
                    imported(new_event(
                        Some("T-999"),
                        if n % 2 == 0 {
                            "owner_decision"
                        } else {
                            "idea_decided"
                        },
                        json!({"run":"R1","n":n}),
                    ))
                })
                .collect(),
        );
        import(
            &backend,
            vec![imported(new_event(
                Some(&idea.id),
                "owner_decision",
                json!({
                    "actor":"owner", "action":"drop", "reason":"Approved", "source":"acceptance",
                    "revision":corral_core::owner_revision(&idea).unwrap()
                }),
            ))],
        );
        let before = work::snapshot();
        {
            let _hot = HotScope::enter();
            store
                .update_head(&idea.id, |head| {
                    head.status = corral_core::Status::Dropped;
                    Ok(())
                })
                .unwrap();
        }
        let delta = work::snapshot().since(&before);
        delta.check_identity().unwrap();
        assert_eq!(delta.get(Counter::EventDecodes), 1);
        assert_eq!(delta.full_task_hydrations(), 0);
        let audit = store
            .events_of(Some(&idea.id), &["idea_decided", "edited"])
            .unwrap();
        assert_eq!(
            audit
                .iter()
                .map(|event| event.kind.as_str())
                .collect::<Vec<_>>(),
            ["idea_decided"]
        );
        assert_eq!(audit[0].detail["reason"], "Approved");
        assert_eq!(audit[0].detail["action"], "drop");
        delta
    };
    assert_eq!(check(1000), check(10_000));
}

#[test]
#[ignore = "dedicated million-event latency acceptance; run explicitly"]
fn million_events_page_500_p99_under_20ms_without_other_task_decodes() {
    let (_dir, base) = store();
    let backend = SqliteBackend::open(base.dir(), SqliteOptions::default()).unwrap();
    // Bounded 10k batches: no million-element allocation or million fsync calls.
    for start in (0..1_000_000_u64).step_by(10_000) {
        let batch = (start..start + 10_000)
            .map(|n| {
                imported(new_event(
                    Some(if n % 10 == 0 { "T-001" } else { "T-002" }),
                    "bench",
                    json!({"run": "R1", "n": n}),
                ))
            })
            .collect();
        import(&backend, batch);
    }
    let store = base.with_backend(Box::new(backend));
    for shape in ["task", "since", "kind-run"] {
        let mut samples = Vec::with_capacity(200);
        for n in 0..200_u64 {
            let after = n * 5000;
            let before = work::snapshot();
            let (events, elapsed) = {
                let _hot = HotScope::enter();
                let start = Instant::now();
                let events = if shape == "since" {
                    let mut reader = store
                        .events_since(Some(corral_core::EventCursor {
                            seq: after,
                            ..Default::default()
                        }))
                        .unwrap();
                    (0..500)
                        .map(|_| reader.next_event().unwrap().unwrap().0)
                        .collect()
                } else {
                    let (kinds, run) = if shape == "task" {
                        (&[][..], None)
                    } else {
                        (&["bench", "absent"][..], Some("R1"))
                    };
                    store
                        .events_page(Some("T-001"), kinds, run, after, 500)
                        .unwrap()
                };
                (events, start.elapsed())
            };
            samples.push(elapsed);
            let delta = work::snapshot().since(&before);
            delta.check_identity().unwrap();
            assert_eq!(events.len(), 500);
            assert_eq!(delta.get(Counter::EventDecodes), 500);
            assert_eq!(delta.full_task_hydrations(), 0);
            for (offset, event) in events.iter().enumerate() {
                let offset = u64::try_from(offset).unwrap();
                if shape == "since" {
                    assert_eq!(event.seq, after + offset + 1);
                } else {
                    assert_eq!(event.task.as_deref(), Some("T-001"));
                    assert_eq!(event.detail["n"], after + offset * 10);
                }
            }
        }
        samples.sort_unstable();
        // Nearest-rank p99 of 200 samples is the 198th ordered sample.
        let p99 = samples[197];
        eprintln!("million events; {shape}; 200 pages of 500; p99={p99:?}");
        assert!(
            p99 < Duration::from_millis(20),
            "page p99={p99:?}, required <20ms"
        );
    }
}
