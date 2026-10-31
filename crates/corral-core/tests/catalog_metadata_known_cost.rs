//! Exact T-672 expansion of already selected actual catalog owners, not discovery/output.
mod store_support;
use corral_core::{
    Store,
    backend::{TaskDoc, TaskNum, TaskPart, TaskParts},
    work::{self, Counter, HotScope},
};
use std::collections::BTreeMap;

fn full_cost(doc: &TaskDoc) -> BTreeMap<Counter, u64> {
    let mut expected: BTreeMap<_, _> = Counter::ALL.into_iter().map(|c| (c, 0)).collect();
    expected.insert(Counter::FullTaskHydrations, 1);
    expected.insert(Counter::TaskHeadDecodes, 1);
    let size = |doc: &serde_json::Value| serde_json::to_vec(doc).unwrap().len() as u64;
    let mut bytes = size(&doc.head.doc) + size(&doc.head.summary);
    for row in &doc.rows {
        let counter = match row.part {
            TaskPart::Run if row.current => Counter::RunDecodes,
            TaskPart::PlanRun | TaskPart::Plan | TaskPart::AggregateAttempt if row.current => {
                Counter::TaskHeadDecodes
            }
            TaskPart::Run
            | TaskPart::PlanRun
            | TaskPart::Plan
            | TaskPart::AggregateAttempt
            | TaskPart::Comment
            | TaskPart::Artifact
            | TaskPart::ScopeRevision
            | TaskPart::PmReceipt
            | TaskPart::MemberGateHistory
            | TaskPart::Recovery
            | TaskPart::RecoveryOutcome
            | TaskPart::PaneRef
            | TaskPart::Finding
            | TaskPart::Prompt
            | TaskPart::GateAttempt
            | TaskPart::ReviewEvidence => Counter::HistoryRowDecodes,
            _ => Counter::NestedRowDecodes,
        };
        *expected.get_mut(&counter).unwrap() += 1;
        bytes += size(&row.doc);
    }
    expected.insert(Counter::DecodedDocBytes, bytes);
    expected
}

#[test]
fn catalog_discovery_and_output_stay_headed_while_selected_metadata_has_exact_full_cost() {
    let mut costs = Vec::new();
    for scale in [1, 10] {
        let root = tempfile::tempdir().unwrap();
        let store = Store::init(root.path()).unwrap();
        store.fixture_pin_now(store_support::at(10)).unwrap();
        let mut task = store_support::rich(&store, 2 * scale, 2 * scale, 2 * scale, 2 * scale);
        task.artifacts.truncate(1);
        store.fixture_put_task(&task).unwrap();
        for reopen in [false, true] {
            let store = if reopen {
                Store::open(root.path()).unwrap()
            } else {
                store.clone()
            };
            let expected_task = store.get(&task.id).unwrap();
            let doc = store
                .view()
                .unwrap()
                .task(TaskNum(1), &TaskParts::All)
                .unwrap()
                .unwrap();
            let expected = full_cost(&doc);
            let hot = HotScope::enter();
            let before = work::snapshot();
            let owners = store.artifact_owners().unwrap();
            let report = work::snapshot().since(&before);
            drop(hot);
            assert_eq!(owners.len(), 1);
            assert_eq!(owners[0].id, expected_task.id);
            assert_eq!(owners[0].title, expected_task.title);
            assert_eq!(owners[0].artifacts, expected_task.artifacts);
            assert_eq!(owners[0].task(), Some(&expected_task));
            report.check_identity().unwrap();
            assert_eq!(report.unattributed_hydrations(), 0);
            assert_eq!(report.followup_calls("T-672", "catalog_task_metadata"), 1);
            assert_eq!(
                report.followup_calls("T-671", "assistant_session_enumeration"),
                1
            );
            assert_eq!(report.followup_calls.len(), 2);
            for counter in Counter::ALL {
                let cells = report.cells(counter);
                assert_eq!(
                    cells
                        .followup
                        .get(&("T-672".into(), "catalog_task_metadata".into()))
                        .copied()
                        .unwrap_or(0),
                    expected[&counter],
                    "{counter:?}"
                );
                // No assistant document exists: enumeration uses the same pinned view.
                assert_eq!(
                    cells
                        .followup
                        .get(&("T-671".into(), "assistant_session_enumeration".into()))
                        .copied()
                        .unwrap_or(0),
                    0,
                    "assistant {counter:?}"
                );
            }
            costs.push(report);
        }
    }
    for cost in &costs[1..] {
        for counter in Counter::ALL {
            assert_eq!(cost.get(counter), costs[0].get(counter), "{counter:?}");
        }
    }
}
