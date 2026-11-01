//! Heads on the database store (T-320 store-tasks-db): cold-process update_head budgets across
//! history and nested growth, constant-size projections and keyed child reads.

mod store_support;

use std::collections::BTreeMap;
use std::process::{Command, Stdio};

use corral_core::backend::{HistoryRow, TaskNum, TaskPart, TaskParts};
use corral_core::work::{self, Counter, HotScope, WorkSnapshot};
use corral_core::{Status, Store};
use serde_json::Value;
use store_support::*;

const CHILD: &str = "CORRAL_STORE_HEADS_CHILD";
const ROOT: &str = "CORRAL_STORE_HEADS_ROOT";

/// One task grown to `runs` coding runs and `history` of each history vector, the current
/// containers holding `children` review children and `nested` of each nested collection.
fn variant(
    runs: usize,
    history: usize,
    children: usize,
    nested: usize,
) -> (tempfile::TempDir, Store) {
    let (dir, store) = store();
    // Creation also stamps status_since; pin the mutation clock before any derived rows.
    store.fixture_pin_now(at(0)).unwrap();
    let mut task = store
        .create("Grown", Some("app"), Status::Running, "Spec body\n")
        .unwrap();
    grow(&mut task, runs, children, history, nested);
    (task.created, task.updated) = (at(0), at(0));
    assert_eq!(task.status_since, Some(at(0)));
    store.fixture_put_task(&task).unwrap();
    (dir, store)
}

fn rows(store: &Store) -> BTreeMap<(TaskPart, String), HistoryRow> {
    store
        .view()
        .unwrap()
        .task(TaskNum(1), &TaskParts::All)
        .unwrap()
        .unwrap()
        .rows
        .into_iter()
        .map(|row| ((row.part, row.key.clone()), row))
        .collect()
}

fn counters(delta: &WorkSnapshot) -> BTreeMap<String, u64> {
    let mut out: BTreeMap<String, u64> = Counter::ALL
        .iter()
        .map(|c| (c.name().to_owned(), delta.get(*c)))
        .collect();
    let followup = Counter::ALL
        .iter()
        .map(|c| delta.cells(*c).followup_sum())
        .sum();
    out.insert("followup".to_owned(), followup);
    out
}

/// Re-executed by the cold-process test: one fresh process per measured step.
#[test]
fn child_process() {
    let (Ok(role), Ok(root)) = (std::env::var(CHILD), std::env::var(ROOT)) else {
        return;
    };
    assert_eq!(role, "update-head");
    let store = Store::open(std::path::Path::new(&root)).unwrap();
    let before = work::snapshot();
    {
        let _hot = HotScope::enter();
        store
            .update_head("T-001", |h| {
                h.current_run_mut().unwrap().summary = Some("cold step".into());
                Ok(())
            })
            .unwrap();
    }
    let delta = work::snapshot().since(&before);
    delta.check_identity().unwrap();
    println!(
        "\nCOUNTERS {}",
        serde_json::to_string(&counters(&delta)).unwrap()
    );
}

fn cold_update_head(root: &std::path::Path) -> BTreeMap<String, u64> {
    let out = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "child_process",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD, "update-head")
        .env(ROOT, root)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8(out.stdout).unwrap();
    let line = text
        .lines()
        .find_map(|line| line.strip_prefix("COUNTERS "))
        .unwrap_or_else(|| panic!("no counters in {text}"));
    serde_json::from_str(line).unwrap()
}

#[test]
fn cold_update_head_costs_the_same_at_24_and_240_runs_and_with_nested_growth() {
    let mut reports = Vec::new();
    for (runs, history, children, nested) in [(24, 3, 3, 1), (240, 30, 3, 1), (240, 30, 30, 10)] {
        // Given the task at this size.
        let (dir, store) = variant(runs, history, children, nested);
        let before = rows(&store);
        let seq = store.view().unwrap().seq();
        // When a fresh process changes one field of the current run through update_head.
        let report = cold_update_head(dir.path());
        // Then it reads only the head and the current run, and writes only the current run row.
        assert_eq!(report["run_decodes"], 1, "{runs}: {report:?}");
        assert_eq!(report["nested_row_decodes"], 0, "{runs}: {report:?}");
        assert_eq!(report["full_task_hydrations"], 0, "{runs}: {report:?}");
        assert_eq!(report["followup"], 0, "{runs}: {report:?}");
        let after = rows(&store);
        let changed: Vec<_> = after
            .iter()
            .filter(|(key, row)| before.get(*key) != Some(*row))
            .map(|(key, _)| key.clone())
            .collect();
        assert_eq!(changed, [(TaskPart::Run, format!("{runs:06}"))]);
        assert_eq!(before.len(), after.len());
        assert_eq!(store.changes_since(seq, 10).unwrap().entries.len(), 1);
        assert_eq!(
            store
                .get("T-001")
                .unwrap()
                .current_run()
                .unwrap()
                .summary
                .as_deref(),
            Some("cold step")
        );
        reports.push(report);
    }
    for name in [
        "task_head_decodes",
        "decoded_doc_bytes",
        "record_decodes",
        "file_stats",
        "history_row_decodes",
    ] {
        let values: Vec<u64> = reports.iter().map(|r| r[name]).collect();
        assert!(
            values.windows(2).all(|w| w[0] == w[1]),
            "{name}: {values:?}"
        );
    }
}

fn head_sizes(store: &Store) -> (Vec<usize>, BTreeMap<String, u64>) {
    let before = work::snapshot();
    let head = {
        let _hot = HotScope::enter();
        store.head("T-001").unwrap()
    };
    let delta = work::snapshot().since(&before);
    let size = |value: Value| value.to_string().len();
    let sizes = vec![
        size(serde_json::to_value(&head.current_run).unwrap()),
        size(serde_json::to_value(&head.last_plan_run).unwrap()),
        size(serde_json::to_value(&head.last_plan).unwrap()),
        size(serde_json::to_value(&head.last_aggregate_attempt).unwrap()),
    ];
    (sizes, counters(&delta))
}

#[test]
fn projections_and_head_reads_keep_their_size_as_nested_collections_grow() {
    let (_small_dir, small) = variant(2, 2, 3, 1);
    let (_large_dir, large) = variant(2, 2, 30, 10);
    let (small_sizes, small_counts) = head_sizes(&small);
    let (large_sizes, large_counts) = head_sizes(&large);
    assert_eq!(small_sizes, large_sizes);
    for name in ["task_head_decodes", "decoded_doc_bytes", "run_decodes"] {
        assert_eq!(small_counts[name], large_counts[name], "{name}");
    }
    assert_eq!(large_counts["followup"], 0);
    assert_eq!(large_counts["nested_row_decodes"], 0);
    assert_eq!(large_counts["full_task_hydrations"], 0);
}

fn nested_decodes(f: impl FnOnce()) -> u64 {
    let before = work::snapshot();
    {
        let _hot = HotScope::enter();
        f();
    }
    work::snapshot()
        .since(&before)
        .get(Counter::NestedRowDecodes)
}

#[test]
fn keyed_child_reads_decode_only_the_rows_they_return() {
    let (_dir, store) = variant(2, 2, 30, 10);
    let head = store.head("T-001").unwrap();
    let run = "R0002";
    assert_eq!(
        nested_decodes(|| {
            assert!(
                head.review_child(run, "R0002-review0007")
                    .unwrap()
                    .is_some()
            );
        }),
        1
    );
    assert_eq!(
        nested_decodes(|| assert_eq!(head.open_review_children(run).unwrap().len(), 1)),
        1
    );
    assert_eq!(
        nested_decodes(|| assert!(head.wait(run, "W0003").unwrap().is_some())),
        1
    );
    let open = head.open_obligations(run).unwrap();
    assert_eq!(
        nested_decodes(|| {
            head.open_obligations(run).unwrap();
        }),
        (open.waits.len() + open.decisions.len()) as u64
    );
    let plan = head.last_plan.as_ref().unwrap().id.clone();
    assert_eq!(
        nested_decodes(|| assert_eq!(head.pending_plan_reviews(&plan).unwrap().len(), 1)),
        1
    );
    let metas: Vec<_> = head
        .checkpoint_metas_newest_first(run)
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(metas.len(), 10);
    let newest = metas[0].clone();
    assert_eq!(
        nested_decodes(|| {
            let first = head.checkpoint_metas_newest_first(run).unwrap().next();
            assert_eq!(first.unwrap().unwrap(), newest);
        }),
        1
    );
    let payload = head
        .checkpoint_payload(run, newest.ordinal)
        .unwrap()
        .unwrap();
    assert_eq!(payload.run_obligations.decisions.len(), 10);
    assert_eq!(
        nested_decodes(|| {
            head.checkpoint_payload(run, newest.ordinal)
                .unwrap()
                .unwrap();
        }),
        2,
        "its meta row and exactly one payload"
    );
}
