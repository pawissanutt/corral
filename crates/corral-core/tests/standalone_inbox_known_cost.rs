//! T-669 owns only physical standalone inbox documents, not surrounding delivery work.
use corral_core::work::{self, Counter, HotScope, WorkSnapshot};
use corral_core::{
    Store,
    inbox::{Inbox, ObserveCursor, Recipient, SessionBinding, SessionCursor},
};

fn exact(report: &WorkSnapshot, site: &str, rows: u64, bytes: u64, views: u64, calls: u64) {
    report.check_identity().unwrap();
    assert_eq!(report.followup_calls("T-669", site), calls);
    for counter in Counter::ALL {
        let expected = match counter {
            Counter::RecordDecodes => rows,
            Counter::DecodedDocBytes => bytes,
            Counter::ViewsOpened => views,
            _ => 0,
        };
        assert_eq!(
            report
                .cells(counter)
                .followup
                .get(&("T-669".into(), site.into()))
                .copied()
                .unwrap_or(0),
            expected,
            "{site} {counter:?}"
        );
    }
    assert_eq!(report.unattributed_hydrations(), 0);
}

#[test]
fn canonical_standalone_loads_and_saves_have_exact_doc_cost_for_both_owner_kinds() {
    for assistant in [false, true] {
        let mut reads = Vec::new();
        let mut writes = Vec::new();
        for scale in [1, 10] {
            let root = tempfile::tempdir().unwrap();
            let store = Store::init(root.path()).unwrap();
            let (recipient, area, key) = if assistant {
                store.create_assistant("pi", None, None, None).unwrap();
                (
                    Recipient::Assistant {
                        session: "A-1".into(),
                    },
                    "assistant-inbox",
                    "A-1",
                )
            } else {
                (Recipient::Pm, "pm-inbox", "current")
            };
            let inbox = Inbox::new(&store, recipient).unwrap();
            let binding = SessionBinding {
                session: "old-0001".into(),
                endpoint: None,
                transcript: None,
            };
            inbox
                .update_state(|state| {
                    state.effects_applied = (1..=2 * scale).collect();
                    state.observe_cursors = (1..=2 * scale)
                        .map(|n| SessionCursor {
                            adapter: "fixture".into(),
                            binding: SessionBinding {
                                session: format!("old-{n:04}"),
                                endpoint: None,
                                transcript: None,
                            },
                            cursor: ObserveCursor {
                                offset: n,
                                ..Default::default()
                            },
                        })
                        .collect();
                    Ok(())
                })
                .unwrap();
            let doc = store.record_row(area, key).unwrap().unwrap().doc;
            let bytes = serde_json::to_vec(&doc).unwrap().len() as u64;
            let hot = HotScope::enter();
            let before = work::snapshot();
            inbox.state_head().unwrap();
            assert!(inbox.effect_applied(1).unwrap());
            assert_eq!(
                inbox
                    .observe_cursor_of("fixture", &binding)
                    .unwrap()
                    .unwrap()
                    .offset,
                1
            );
            let report = work::snapshot().since(&before);
            drop(hot);
            exact(&report, "standalone_inbox_load", 3, 3 * bytes, 3, 3);
            reads.push(report);
            let hot = HotScope::enter();
            let before = work::snapshot();
            inbox
                .update_state(|state| {
                    state.effects_applied.insert(999);
                    Ok(())
                })
                .unwrap();
            let report = work::snapshot().since(&before);
            drop(hot);
            exact(&report, "standalone_inbox_load", 1, bytes, 1, 1);
            exact(&report, "standalone_inbox_save", 1, bytes, 0, 1);
            writes.push(report);
            assert_eq!(inbox.state().unwrap().effects_applied.last(), Some(&999));
        }
        for counter in Counter::ALL {
            assert_eq!(
                reads[0].get(counter),
                reads[1].get(counter),
                "read {counter:?}"
            );
            assert_eq!(
                writes[0].get(counter),
                writes[1].get(counter),
                "write {counter:?}"
            );
        }
    }
}

#[test]
fn absent_standalone_document_cost_is_only_its_view_and_never_task_hydration() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
    let hot = HotScope::enter();
    let before = work::snapshot();
    assert_eq!(inbox.state_head().unwrap(), Default::default());
    let report = work::snapshot().since(&before);
    drop(hot);
    exact(&report, "standalone_inbox_load", 0, 0, 1, 1);
}
