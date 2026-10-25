//! Random head and full-update sequences on the fake and SQLite backends (T-320 store-tasks-db):
//! after every step the stored summary, the container facts and their revision counters agree
//! with the assembled task, which equals an independently maintained expected task.

mod store_support;

use std::collections::BTreeMap;
use std::sync::Arc;

use corral_core::backend::fake::FakeBackend;
use corral_core::backend::{
    BackendConfig, FaultInjector, HistoryRow, ManualClock, TaskNum, TaskPart, TaskParts,
};
use corral_core::{
    CommentAuthor, ContainerFacts, HistorySummary, ProfileSwitch, Run, Status, Store, Task,
    TaskComment, Usage, UsageEntry, WaitRecord,
};
use store_support::*;

type Rows = BTreeMap<(TaskPart, String), HistoryRow>;

struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

fn rows(store: &Store) -> Rows {
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

fn revision(row: &HistoryRow, field: &str) -> u64 {
    let value = &row.doc["_facts"][field];
    value
        .as_u64()
        .or_else(|| value.as_str()?.parse().ok())
        .unwrap()
}

fn changed_under(before: &Rows, after: &Rows, part: TaskPart, prefix: &str) -> bool {
    let docs = |rows: &Rows| -> Vec<(String, serde_json::Value)> {
        rows.iter()
            .filter(|((p, key), _)| *p == part && key.starts_with(prefix))
            .map(|((_, key), row)| (key.clone(), row.doc.clone()))
            .collect()
    };
    docs(before) != docs(after)
}

fn check(store: &Store, expected: &mut Task, before: &Rows, step: &str) {
    let task = store.get("T-001").unwrap();
    expected.updated = task.updated;
    assert_eq!(&task, expected, "{step}");
    let summary: HistorySummary = serde_json::from_value(
        store
            .view()
            .unwrap()
            .head(TaskNum(1))
            .unwrap()
            .unwrap()
            .summary,
    )
    .unwrap();
    assert_eq!(summary, HistorySummary::of(&task), "{step}");
    let after = rows(store);
    for (i, run) in task.runs.iter().enumerate() {
        let key = format!("{:06}", i + 1);
        let row = &after[&(TaskPart::Run, key.clone())];
        let stored: ContainerFacts = serde_json::from_value(row.doc["_facts"].clone()).unwrap();
        let derived = ContainerFacts::of_run(&key, run).unwrap();
        assert_eq!(
            stored.without_revisions(),
            derived.without_revisions(),
            "{step} run {key}"
        );
        let Some(old) = before.get(&(TaskPart::Run, key.clone())) else {
            continue;
        };
        let children = changed_under(before, &after, TaskPart::ReviewRun, &format!("{key}/"));
        assert_eq!(
            revision(row, "children_rev"),
            revision(old, "children_rev") + u64::from(children),
            "{step} run {key} children_rev"
        );
        let obligations = changed_under(
            before,
            &after,
            TaskPart::RunObligation,
            &format!("{key}/000000/"),
        );
        assert_eq!(
            revision(row, "obligations_rev"),
            revision(old, "obligations_rev") + u64::from(obligations),
            "{step} run {key} obligations_rev"
        );
    }
}

fn wait_record(run: &str, n: usize) -> WaitRecord {
    serde_json::from_value(wait(run, 9000 + n, false)).unwrap()
}

fn entry(n: usize) -> UsageEntry {
    serde_json::from_value(usage_entry(n, true)).unwrap()
}

fn current_id(task: &Task) -> String {
    task.runs.last().unwrap().id.clone()
}

fn step(store: &Store, expected: &mut Task, rng: &mut Rng, n: usize) -> String {
    let run = current_id(expected);
    match rng.below(11) {
        0 => {
            let title = format!("Title {n}");
            store
                .update_head("T-001", |h| {
                    h.title = title.clone();
                    Ok(())
                })
                .unwrap();
            expected.title = title;
            "head title".into()
        }
        1 => {
            let summary = Some(format!("current {n}"));
            store
                .update_head("T-001", |h| {
                    h.current_run_mut().unwrap().summary = summary.clone();
                    Ok(())
                })
                .unwrap();
            expected.runs.last_mut().unwrap().summary = summary;
            "current run field".into()
        }
        2 => {
            let comment = TaskComment {
                author: CommentAuthor::Owner,
                at: at(n as i64),
                text: format!("comment {n}"),
            };
            store
                .update_head("T-001", |h| h.add_comment(comment.clone()))
                .unwrap();
            expected.comments.push(comment);
            "add_comment".into()
        }
        3 => {
            let k = expected.runs.last().unwrap().review_runs.len() + 1;
            let child: Run =
                serde_json::from_value(review_child("T-001", &run, k, 1, true, "approve")).unwrap();
            store
                .update_head("T-001", |h| h.push_review_child(&run, child.clone()))
                .unwrap();
            expected.runs.last_mut().unwrap().review_runs.push(child);
            "push_review_child".into()
        }
        4 => {
            let wait = wait_record(&run, n);
            store
                .update_head("T-001", |h| h.push_wait(&run, wait.clone()))
                .unwrap();
            expected
                .runs
                .last_mut()
                .unwrap()
                .obligations
                .waits
                .push(wait);
            "push_wait".into()
        }
        5 => {
            let switch: ProfileSwitch = serde_json::from_value(switch(n)).unwrap();
            store
                .update_head("T-001", |h| h.push_switch(&run, switch.clone()))
                .unwrap();
            expected.runs.last_mut().unwrap().switches.push(switch);
            "push_switch".into()
        }
        6 => {
            store
                .update_head("T-001", |h| {
                    h.append_usage_entries(&run, None, vec![entry(n)])
                })
                .unwrap();
            let current = expected.runs.last_mut().unwrap();
            let mut all = current.usage.take().map(|u| u.entries).unwrap_or_default();
            all.push(entry(n));
            current.usage = Some(Usage::from_entries(all));
            "append_usage_entries".into()
        }
        7 => {
            let children = &expected.runs.last().unwrap().review_runs;
            let child = children[rng.below(children.len())].id.clone();
            let summary = Some(format!("child {n}"));
            store
                .update_head("T-001", |h| {
                    h.edit_review_child(&run, &child, |c| {
                        c.summary = summary.clone();
                        Ok(())
                    })
                })
                .unwrap();
            let current = expected.runs.last_mut().unwrap();
            current
                .review_runs
                .iter_mut()
                .find(|c| c.id == child)
                .unwrap()
                .summary = summary;
            "edit_review_child".into()
        }
        8 => {
            let old = rng.below(expected.runs.len() - 1);
            let summary = Some(format!("old run {n}"));
            store
                .update("T-001", |t| {
                    t.runs[old].summary = summary.clone();
                    Ok(())
                })
                .unwrap();
            expected.runs[old].summary = summary;
            "full old-run edit".into()
        }
        9 => {
            let old = rng.below(expected.runs.len() - 1);
            let k = rng.below(expected.runs[old].review_runs.len());
            let summary = Some(format!("old child {n}"));
            store
                .update("T-001", |t| {
                    t.runs[old].review_runs[k].summary = summary.clone();
                    Ok(())
                })
                .unwrap();
            expected.runs[old].review_runs[k].summary = summary;
            "full old-child edit".into()
        }
        _ => {
            let child = expected.runs.last().unwrap().review_runs[0].id.clone();
            let seq = store.view().unwrap().seq();
            let refused = store.update_head("T-001", |h| {
                h.edit_review_child(&run, &child, |c| {
                    c.id = "renamed".into();
                    Ok(())
                })
            });
            assert!(refused.is_err(), "an id change is refused");
            assert_eq!(
                store.view().unwrap().seq(),
                seq,
                "a refused edit writes nothing"
            );
            "refused id edit".into()
        }
    }
}

fn run_sequence(store: &Store, seed: u64) {
    let mut expected = store
        .create("Property", Some("app"), Status::Running, "Spec\n")
        .unwrap();
    grow(&mut expected, 3, 2, 2, 2);
    store.fixture_put_task(&expected).unwrap();
    let mut rng = Rng(seed);
    let mut before = rows(store);
    check(store, &mut expected, &before, "seed");
    for n in 0..40 {
        let label = step(store, &mut expected, &mut rng, n);
        check(
            store,
            &mut expected,
            &before,
            &format!("seed {seed} step {n}: {label}"),
        );
        before = rows(store);
    }
}

#[test]
fn random_head_and_full_updates_keep_summary_facts_and_revisions_exact() {
    for seed in [
        0x9e37_79b9_7f4a_7c15,
        0x2545_f491_4f6c_dd1d,
        0x1234_5678_9abc_def1,
    ] {
        let (_sqlite_dir, sqlite) = store();
        run_sequence(&sqlite, seed);

        let (_fake_dir, base) = store();
        let backend = FakeBackend::new(
            BackendConfig::default(),
            Arc::new(ManualClock::new(jiff::Timestamp::now())),
            Arc::new(FaultInjector::default()),
        );
        let fake = base.with_backend(Box::new(backend));
        run_sequence(&fake, seed);
    }
}
