//! Container CAS guards (plan T-253 section 4 table): each guard's db316644 predicate over full
//! tasks against its head form over facts, after random interleavings of child inserts and
//! edits between the read and the guard. The reviewer and usage guards decide exactly as
//! db316644; the two aggregate-attempt and the two obligations guards decide the same except
//! that they refuse an A->B->A change of child content.

mod store_support;

use std::collections::BTreeMap;

use corral_core::backend::{TaskNum, TaskPart, TaskParts};
use corral_core::{Run, Status, Store, Task, TaskHead};
use serde_json::Value;
use store_support::*;

const RUN: &str = "R0002";
const ATTEMPT: &str = "A0002";

/// What a guard read: the full task (db316644) and the head with the run's own rows.
struct Read {
    task: Task,
    head: TaskHead,
    own_rows: BTreeMap<(TaskPart, String), Value>,
}

fn without_revisions(mut doc: Value) -> Value {
    if let Some(facts) = doc.get_mut("_facts").and_then(Value::as_object_mut) {
        facts.remove("children_rev");
        facts.remove("obligations_rev");
    }
    doc
}

fn read(store: &Store) -> Read {
    let seq = format!("{:06}", 2);
    let own_rows = store
        .view()
        .unwrap()
        .task(TaskNum(1), &TaskParts::All)
        .unwrap()
        .unwrap()
        .rows
        .into_iter()
        .filter(|row| {
            let key = row.key.strip_prefix("run:").unwrap_or(&row.key);
            (row.part == TaskPart::Run && key == seq) || key.starts_with(&format!("{seq}/"))
        })
        .map(|row| ((row.part, row.key), without_revisions(row.doc)))
        .collect();
    Read {
        task: store.get("T-001").unwrap(),
        head: store.head("T-001").unwrap(),
        own_rows,
    }
}

fn run_of(task: &Task) -> &Run {
    task.runs.iter().find(|r| r.id == RUN).unwrap()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Guard {
    Reviewer,
    Usage,
    AggregateReview,
    AggregateAccept,
    Sync,
    Obligations,
}

const GUARDS: [Guard; 6] = [
    Guard::Reviewer,
    Guard::Usage,
    Guard::AggregateReview,
    Guard::AggregateAccept,
    Guard::Sync,
    Guard::Obligations,
];

impl Guard {
    /// The db316644 predicate: whether the guard accepts `now` against what it read.
    fn db316644(self, read: &Task, now: &Task) -> bool {
        let (run, cur) = (run_of(read), run_of(now));
        match self {
            // reviewer.rs:384-388
            Guard::Reviewer => {
                cur.review_runs.len() == run.review_runs.len() && cur.review == run.review
            }
            // usage.rs:136-139
            Guard::Usage => cur == run,
            // plan_acceptance/review.rs:398-402 and accept.rs:119-122
            Guard::AggregateReview | Guard::AggregateAccept => {
                now.aggregate_attempts.last() == read.aggregate_attempts.last()
            }
            // sync.rs:478-484
            Guard::Sync => {
                cur.stuck == run.stuck
                    && cur.pending_reply == run.pending_reply
                    && cur.obligations == run.obligations
                    && cur.seen_pane == run.seen_pane
            }
            // obligations_sync.rs:82
            Guard::Obligations => cur.obligations == run.obligations && cur.gate == run.gate,
        }
    }

    /// The head form: the same read set over projections, facts and revisions.
    fn head_form(self, read: &Read, now: &Read) -> bool {
        let run = |r: &Read| r.head.current_run.clone().unwrap();
        let (a, b) = (run(read), run(now));
        let attempt = |r: &Read| {
            let attempt = r.head.last_aggregate_attempt.clone().unwrap();
            (serde_json::to_value(&attempt).unwrap(), attempt.facts)
        };
        match self {
            Guard::Reviewer => a.facts.children == b.facts.children && a.review == b.review,
            Guard::Usage => read.own_rows == now.own_rows,
            Guard::AggregateReview | Guard::AggregateAccept => attempt(read) == attempt(now),
            Guard::Sync => {
                a.stuck == b.stuck
                    && a.pending_reply == b.pending_reply
                    && a.facts.obligations_rev == b.facts.obligations_rev
                    && a.seen_pane == b.seen_pane
            }
            Guard::Obligations => {
                a.facts.obligations_rev == b.facts.obligations_rev && a.gate == b.gate
            }
        }
    }

    fn exact(self) -> bool {
        matches!(self, Guard::Reviewer | Guard::Usage)
    }

    /// The child content whose A->B->A change the stricter guards refuse.
    fn children(self, task: &Task) -> Value {
        match self {
            Guard::AggregateReview | Guard::AggregateAccept => {
                serde_json::to_value(&task.aggregate_attempts.last().unwrap().reviews).unwrap()
            }
            _ => serde_json::to_value(&run_of(task).obligations).unwrap(),
        }
    }
}

fn step(store: &Store, rng: &mut Rng, n: usize) {
    let before = store.get("T-001").unwrap();
    match rng.below(10) {
        0 => {
            let k = run_of(&before).review_runs.len() + 1;
            let child: Run =
                serde_json::from_value(review_child("T-001", RUN, k + 100, 1, true, "approve"))
                    .unwrap();
            store
                .update_head("T-001", |h| h.push_review_child(RUN, child))
                .unwrap();
        }
        1 => {
            let entry: corral_core::UsageEntry =
                serde_json::from_value(usage_entry(n, true)).unwrap();
            store
                .update("T-001", |t| {
                    let earlier = &mut t.runs[1].review_runs[0];
                    let mut entries = earlier.usage.take().map(|u| u.entries).unwrap_or_default();
                    entries.push(entry);
                    earlier.usage = Some(corral_core::Usage::from_entries(entries));
                    Ok(())
                })
                .unwrap();
        }
        2 => {
            let summary = ["A", "B"][rng.below(2)].to_owned();
            store
                .update_head("T-001", |h| {
                    h.edit_current_review_child(RUN, |c| {
                        c.summary = Some(summary);
                        Ok(())
                    })
                })
                .unwrap();
        }
        3 => {
            let review = run_of(&before).review_runs[0].review.clone();
            let review = if run_of(&before).review.is_some() {
                None
            } else {
                review
            };
            store
                .update("T-001", |t| {
                    t.runs[1].review = review;
                    Ok(())
                })
                .unwrap();
        }
        4 => {
            let first = before.aggregate_attempts.last().unwrap().reviews[0]
                .id
                .clone();
            let digest = ["e", "B"][rng.below(2)].to_owned();
            store
                .update_head("T-001", |h| {
                    h.edit_aggregate_review(ATTEMPT, &first, |r| {
                        r.evidence_digest = digest;
                        Ok(())
                    })
                })
                .unwrap();
        }
        5 => {
            let detail = ["observed", "B"][rng.below(2)].to_owned();
            store
                .update_head("T-001", |h| {
                    h.edit_wait(RUN, "W0000", |w| {
                        w.observation.detail = detail;
                        Ok(())
                    })
                })
                .unwrap();
        }
        6 => {
            let wait: corral_core::WaitRecord =
                serde_json::from_value(wait(RUN, 5000 + n, false)).unwrap();
            store
                .update_head("T-001", |h| h.push_wait(RUN, wait))
                .unwrap();
        }
        7 => {
            let reply = ["x", ""][rng.below(2)];
            store
                .update_head("T-001", |h| {
                    h.current_run_mut().unwrap().pending_reply =
                        (!reply.is_empty()).then(|| reply.to_owned());
                    Ok(())
                })
                .unwrap();
        }
        8 => {
            store
                .update_head("T-001", |h| h.edit_wait(RUN, "W0000", |_| Ok(())))
                .unwrap();
        }
        _ => {
            store.update("T-001", |_| Ok(())).unwrap();
        }
    }
}

fn rich() -> (tempfile::TempDir, Store) {
    let (dir, store) = store();
    let mut task = store.create("Guarded", None, Status::Running, "").unwrap();
    grow(&mut task, 2, 3, 2, 1);
    (task.created, task.updated) = (at(0), at(0));
    store.fixture_put_task(&task).unwrap();
    (dir, store)
}

#[test]
fn head_form_guards_decide_as_db316644_except_stricter_on_a_b_a() {
    let mut compared = BTreeMap::<(&str, bool, bool), usize>::new();
    for seed in 1..=40u64 {
        let (_dir, store) = rich();
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ seed.wrapping_mul(0x2545_f491_4f6c_dd1d));
        // Given what each guard read.
        let base = read(&store);
        // When random child inserts and edits land before the guard runs.
        let mut changed_children: BTreeMap<&'static str, bool> = BTreeMap::new();
        for n in 0..(1 + rng.below(5)) {
            let before = store.get("T-001").unwrap();
            step(&store, &mut rng, seed as usize * 10 + n);
            let after = store.get("T-001").unwrap();
            for (family, guard) in [
                ("attempt", Guard::AggregateReview),
                ("obligations", Guard::Sync),
            ] {
                if guard.children(&before) != guard.children(&after) {
                    changed_children.insert(family, true);
                }
            }
        }
        let now = read(&store);
        // Then each guard decides as db316644 does, or refuses an A->B->A change.
        for guard in GUARDS {
            let old = guard.db316644(&base.task, &now.task);
            let new = guard.head_form(&base, &now);
            *compared
                .entry((format!("{guard:?}").leak(), old, new))
                .or_default() += 1;
            if guard.exact() || old == new {
                assert_eq!(new, old, "seed {seed}: {guard:?}");
                continue;
            }
            assert!(
                old && !new,
                "seed {seed}: {guard:?} accepted a genuine change"
            );
            let family = match guard {
                Guard::AggregateReview | Guard::AggregateAccept => "attempt",
                _ => "obligations",
            };
            assert_eq!(
                guard.children(&base.task),
                guard.children(&now.task),
                "seed {seed}: {guard:?} refused other than on A->B->A"
            );
            assert!(
                changed_children.contains_key(family),
                "seed {seed}: {guard:?} refused although no write changed its children"
            );
            // And after the refusal the next pass rereads and accepts, as after a genuine change.
            let retry = read(&store);
            assert!(guard.db316644(&retry.task, &now.task) && guard.head_form(&retry, &now));
        }
    }
    // Every guard saw both an accepted and a refused interleaving.
    for guard in GUARDS {
        let name = format!("{guard:?}");
        let refused = compared
            .iter()
            .any(|((g, old, _), n)| *g == name && !old && *n > 0);
        let accepted = compared
            .iter()
            .any(|((g, old, _), n)| *g == name && *old && *n > 0);
        assert!(refused && accepted, "{name}: {compared:?}");
    }
}

#[test]
fn byte_identical_writes_never_make_a_guard_refuse() {
    let (_dir, store) = rich();
    let base = read(&store);
    store.update("T-001", |_| Ok(())).unwrap();
    store
        .update_head("T-001", |h| h.edit_wait(RUN, "W0000", |_| Ok(())))
        .unwrap();
    let first = base.task.aggregate_attempts.last().unwrap().reviews[0]
        .id
        .clone();
    store
        .update_head("T-001", |h| {
            h.edit_aggregate_review(ATTEMPT, &first, |_| Ok(()))
        })
        .unwrap();
    store
        .update_head("T-001", |h| h.edit_current_review_child(RUN, |_| Ok(())))
        .unwrap();
    let now = read(&store);
    for guard in GUARDS {
        assert!(guard.db316644(&base.task, &now.task), "{guard:?}");
        assert!(guard.head_form(&base, &now), "{guard:?}");
    }
}

#[test]
fn an_earlier_childs_usage_edit_passes_the_reviewer_guard_only() {
    let (_dir, store) = rich();
    let base = read(&store);
    let entry: corral_core::UsageEntry = serde_json::from_value(usage_entry(77, true)).unwrap();
    store
        .update("T-001", |t| {
            let earlier = &mut t.runs[1].review_runs[0];
            let mut entries = earlier.usage.take().map(|u| u.entries).unwrap_or_default();
            entries.push(entry);
            earlier.usage = Some(corral_core::Usage::from_entries(entries));
            Ok(())
        })
        .unwrap();
    let now = read(&store);
    assert!(Guard::Reviewer.db316644(&base.task, &now.task));
    assert!(Guard::Reviewer.head_form(&base, &now));
    assert!(!Guard::Usage.db316644(&base.task, &now.task));
    assert!(!Guard::Usage.head_form(&base, &now));
}

fn edit_first_aggregate_review(store: &Store, value: &'static str) {
    let id = store
        .get("T-001")
        .unwrap()
        .aggregate_attempts
        .last()
        .unwrap()
        .reviews[0]
        .id
        .clone();
    store
        .update_head("T-001", |h| {
            h.edit_aggregate_review(ATTEMPT, &id, |r| {
                r.evidence_digest = value.into();
                Ok(())
            })
        })
        .unwrap();
}

fn edit_first_wait(store: &Store, value: &'static str) {
    store
        .update_head("T-001", |h| {
            h.edit_wait(RUN, "W0000", |w| {
                w.observation.detail = value.into();
                Ok(())
            })
        })
        .unwrap();
}

type AbaCase = (&'static [Guard], fn(&Store, &'static str), &'static str);

#[test]
fn stricter_guards_refuse_a_b_a_and_accept_on_the_next_read() {
    let cases: [AbaCase; 2] = [
        (
            &[Guard::AggregateReview, Guard::AggregateAccept],
            edit_first_aggregate_review,
            "e",
        ),
        (
            &[Guard::Sync, Guard::Obligations],
            edit_first_wait,
            "observed",
        ),
    ];
    for (guards, edit, original) in cases {
        // Given what the guards read.
        let (_dir, store) = rich();
        let base = read(&store);
        // When a child goes A -> B -> A before they run.
        edit(&store, "B");
        edit(&store, original);
        let now = read(&store);
        for guard in guards {
            // Then db316644 accepts, the head form refuses, and a reread accepts.
            assert_eq!(
                guard.children(&base.task),
                guard.children(&now.task),
                "{guard:?}"
            );
            assert!(guard.db316644(&base.task, &now.task), "{guard:?}");
            assert!(!guard.head_form(&base, &now), "{guard:?}");
            let retry = read(&store);
            assert!(
                guard.db316644(&retry.task, &now.task) && guard.head_form(&retry, &now),
                "{guard:?}"
            );
        }
        // And the exact guards are unaffected by the round trip.
        assert_eq!(
            Guard::Reviewer.db316644(&base.task, &now.task),
            Guard::Reviewer.head_form(&base, &now)
        );
    }
}
