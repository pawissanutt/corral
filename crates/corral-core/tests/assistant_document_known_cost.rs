//! T-671: exact opaque document costs; active-key discovery and callbacks stay outside.
mod store_support;
use corral_core::{
    Store,
    work::{self, Counter, HotScope, WorkSnapshot},
};
use serde_json::json;
use store_support::at;

fn bytes(store: &Store, area: &str, id: &str) -> u64 {
    serde_json::to_vec(&store.record_row(area, id).unwrap().unwrap().doc)
        .unwrap()
        .len() as u64
}
fn exact(report: &WorkSnapshot, site: &str, rows: u64, bytes: u64, views: u64) {
    report.check_identity().unwrap();
    assert_eq!(report.followup_calls("T-671", site), 1);
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
                .get(&("T-671".into(), site.into()))
                .copied()
                .unwrap_or(0),
            expected,
            "{site} {counter:?}"
        );
    }
    assert_eq!(report.unattributed_hydrations(), 0);
}
#[test]
fn actual_assistant_documents_roundtrip_growth_with_exact_load_save_and_enumeration_costs() {
    let mut costs = Vec::new();
    for scale in [1, 10] {
        let root = tempfile::tempdir().unwrap();
        let store = Store::init(root.path()).unwrap();
        store.fixture_pin_now(at(10)).unwrap();
        let session = store.create_assistant("pi", None, None, None).unwrap();
        store
            .update_assistant(&session.id, |s| {
                s.body = "retained body\n".repeat(20 * scale);
                Ok(())
            })
            .unwrap();
        store.update_assistant_container(&session.id, |c| {
            for n in 0..2*scale {
                c.state.rewinds.push(serde_json::from_value(json!({"id":format!("rw-{n:04}"), "checkpoint":"g1-t1", "commit":"commit", "head":"head", "leaf":"leaf", "from_generation":1, "stage":"aborted", "at":at(0), "updated":at(1)})).unwrap());
            }
            Ok(())
        }).unwrap();
        for _ in 0..2 * scale {
            let old = store.create_assistant("pi", None, None, None).unwrap();
            store.end_assistant(&old.id).unwrap();
        }
        for reopen in [false, true] {
            let store = if reopen {
                Store::open(root.path()).unwrap()
            } else {
                store.clone()
            };
            let sb = bytes(&store, "assistant-sessions", &session.id);
            let cb = bytes(&store, "assistant-containers", &session.id);
            let hot = HotScope::enter();
            let before = work::snapshot();
            assert_eq!(
                store.active_assistant_ids().unwrap().as_slice(),
                std::slice::from_ref(&session.id)
            );
            assert_eq!(
                store.get_assistant(&session.id).unwrap().body,
                "retained body\n".repeat(20 * scale)
            );
            assert_eq!(
                store
                    .get_assistant_container(&session.id)
                    .unwrap()
                    .state
                    .rewinds
                    .len(),
                2 * scale
            );
            let read = work::snapshot().since(&before);
            drop(hot);
            exact(&read, "assistant_session_load", 1, sb, 1);
            exact(&read, "assistant_container_load", 1, cb, 1);
            assert_eq!(read.followup_calls.len(), 2);
            costs.push(read);
            let hot = HotScope::enter();
            let before = work::snapshot();
            store
                .update_assistant_container(&session.id, |c| {
                    c.state.binding.closing = !c.state.binding.closing;
                    Ok(())
                })
                .unwrap();
            let write = work::snapshot().since(&before);
            drop(hot);
            exact(&write, "assistant_session_load", 1, sb, 1);
            exact(&write, "assistant_container_load", 1, cb, 1);
            exact(&write, "assistant_container_save", 1, cb, 1);
            assert_eq!(write.followup_calls.len(), 3);
            let docs = store.record_rows("assistant-sessions").unwrap();
            let all_bytes = docs
                .iter()
                .map(|r| serde_json::to_vec(&r.doc).unwrap().len() as u64)
                .sum();
            let hot = HotScope::enter();
            let before = work::snapshot();
            assert_eq!(store.list_assistants().unwrap().len(), 2 * scale + 1);
            let enumeration = work::snapshot().since(&before);
            drop(hot);
            exact(
                &enumeration,
                "assistant_session_enumeration",
                docs.len() as u64,
                all_bytes,
                1,
            );
            assert_eq!(enumeration.followup_calls.len(), 1);
            let hot = HotScope::enter();
            let before = work::snapshot();
            store
                .update_assistant(&session.id, |s| {
                    s.summary = "checkpoint".into();
                    Ok(())
                })
                .unwrap();
            let session_write = work::snapshot().since(&before);
            drop(hot);
            exact(&session_write, "assistant_session_load", 1, sb, 1);
            exact(&session_write, "assistant_session_save", 1, sb, 1);
            assert_eq!(session_write.followup_calls.len(), 2);
        }
    }
    for cost in &costs[1..] {
        for counter in Counter::ALL {
            assert_eq!(cost.get(counter), costs[0].get(counter), "{counter:?}");
        }
    }
}
