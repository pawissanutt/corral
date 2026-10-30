mod store_support;
use corral_core::Store;
use corral_core::work::{self, Counter, HotScope};
use store_support::at;

#[test]
fn current_and_historical_review_notes_match_the_full_facade_with_constant_work() {
    for historical in [false, true] {
        let mut measured = Vec::new();
        for scale in [1, 10] {
            let root = tempfile::tempdir().unwrap();
            let store = Store::init(root.path()).unwrap();
            let mut expected = store_support::rich(&store, 2 * scale + 1, 2, 1, 1);
            expected.created = at(0);
            expected.updated = at(0);
            expected.status_since = Some(at(0));
            let selected = if historical {
                0
            } else {
                expected.runs.len() - 1
            };
            let run = expected.runs[selected].id.clone();
            let child = expected.runs[selected].review_runs[0].id.clone();
            expected.runs[selected].review_runs[0]
                .review
                .as_mut()
                .unwrap()
                .notes = None;
            store.fixture_put_task(&expected).unwrap();
            store.fixture_pin_now(at(5)).unwrap();
            {
                let _hot = HotScope::enter();
                let before = work::snapshot();
                assert!(
                    store
                        .try_update_head(&expected.id, |head| head
                            .set_review_child_notes_if_absent(&run, &child, "exact notes"))
                        .unwrap()
                        .is_some()
                );
                assert!(
                    store
                        .try_update_head(&expected.id, |head| head
                            .set_review_child_notes_if_absent(&run, &child, "must not overwrite"))
                        .unwrap()
                        .is_none()
                );
                assert!(
                    store
                        .try_update_head(&expected.id, |head| head
                            .set_review_child_notes_if_absent(&run, "absent", "unused"))
                        .unwrap()
                        .is_none()
                );
                let after = work::snapshot();
                measured.push(
                    [
                        Counter::TaskHeadDecodes,
                        Counter::NestedRowDecodes,
                        Counter::HistoryRowDecodes,
                        Counter::FullTaskHydrations,
                        Counter::DecodedDocBytes,
                        Counter::ViewsOpened,
                    ]
                    .map(|counter| after.total(counter) - before.total(counter)),
                );
            }
            expected.runs[selected].review_runs[0]
                .review
                .as_mut()
                .unwrap()
                .notes = Some("exact notes".into());
            expected.updated = at(5);
            assert_eq!(store.get(&expected.id).unwrap(), expected);
            assert_eq!(
                store
                    .head(&expected.id)
                    .unwrap()
                    .review_child(&run, &child)
                    .unwrap()
                    .unwrap()
                    .review
                    .unwrap()
                    .notes
                    .as_deref(),
                Some("exact notes")
            );
        }
        assert_eq!(measured[0], measured[1], "historical={historical}");
        assert_eq!(measured[0][3], 0);
    }
}
