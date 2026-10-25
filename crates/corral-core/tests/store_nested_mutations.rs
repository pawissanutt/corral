mod store_support;

use std::sync::Arc;

use corral_core::backend::fake::FakeBackend;
use corral_core::backend::{BackendConfig, FaultInjector, ManualClock, TaskPart};
use corral_core::inbox::{Inbox, Recipient, SessionCursor};
use corral_core::{HistorySummary, Store, TaskHeadMut};
use store_support::*;

fn backends() -> Vec<(tempfile::TempDir, Store)> {
    let sqlite = store();
    let (dir, base) = store();
    let fake = base.with_backend(Box::new(FakeBackend::new(
        BackendConfig::default(),
        Arc::new(ManualClock::new(at(0))),
        Arc::new(FaultInjector::default()),
    )));
    vec![sqlite, (dir, fake)]
}

#[test]
fn nested_obligation_noops_do_not_advance_parent_revisions() {
    for child in 0..2 {
        for decision in [false, true] {
            for (_dir, store) in backends() {
                // Given an earlier or current review child with a wait and a decision.
                let before = rich(&store, 2, 2, 2, 1);
                let id = &before.runs[1].review_runs[child].id;
                let original = &before.runs[1].review_runs[child].obligations;
                let old_rows = task_rows(&store);
                let old_seq = store.view().unwrap().seq();
                // When a nested obligation changes and is restored in one callback.
                store
                    .update_head("T-001", |h| {
                        h.edit_review_child("R0002", id, |c| {
                            if decision {
                                c.obligations.decisions[0].observation.detail = "B".into();
                            } else {
                                c.obligations.waits[0].observation.detail = "B".into();
                            }
                            Ok(())
                        })?;
                        h.edit_review_child("R0002", id, |c| {
                            c.obligations = original.clone();
                            Ok(())
                        })
                    })
                    .unwrap();
                // Then even the parent witness, timestamp and change feed remain unchanged.
                assert_eq!(store.get("T-001").unwrap(), before);
                assert_eq!(
                    task_rows(&store),
                    old_rows,
                    "child={child}, decision={decision}"
                );
                assert_eq!(store.view().unwrap().seq(), old_seq);
            }
        }
    }
}

#[test]
fn separate_nested_obligation_commits_advance_both_witnesses() {
    for child in 0..2 {
        for decision in [false, true] {
            for (_dir, store) in backends() {
                // Given the original revision witnesses for a nested obligation.
                let before = rich(&store, 2, 2, 2, 1);
                let id = &before.runs[1].review_runs[child].id;
                let original = &before.runs[1].review_runs[child].obligations;
                let old_rows = task_rows(&store);
                let old_seq = store.view().unwrap().seq();
                // When A -> B -> A is committed as two separate operations.
                store
                    .update_head("T-001", |h| {
                        h.edit_review_child("R0002", id, |c| {
                            if decision {
                                c.obligations.decisions[0].observation.detail = "B".into();
                            } else {
                                c.obligations.waits[0].observation.detail = "B".into();
                            }
                            Ok(())
                        })
                    })
                    .unwrap();
                store
                    .update_head("T-001", |h| {
                        h.edit_review_child("R0002", id, |c| {
                            c.obligations = original.clone();
                            Ok(())
                        })
                    })
                    .unwrap();
                // Then both witnesses distinguish this from an uncommitted no-op.
                let rows = task_rows(&store);
                for (key, field) in [
                    ((TaskPart::Run, "000002".to_owned()), "children_rev"),
                    (
                        (TaskPart::ReviewRun, format!("000002/{:06}", child + 1)),
                        "obligations_rev",
                    ),
                ] {
                    assert_eq!(
                        revision(&rows[&key], field),
                        revision(&old_rows[&key], field) + 2
                    );
                }
                assert!(store.view().unwrap().seq() > old_seq);
                let mut after = store.get("T-001").unwrap();
                after.updated = before.updated;
                assert_eq!(after, before);
            }
        }
    }
}

fn queued_recipient(h: &mut TaskHeadMut, kind: usize, demote: bool) -> anyhow::Result<Recipient> {
    match kind {
        0 => {
            h.start_run(coding_run("T-001", "R0003", 1000, 1, 1, false))?;
            if demote {
                h.start_run(coding_run("T-001", "R0004", 1100, 1, 1, false))?;
            }
            Ok(Recipient::Run {
                task: "T-001".into(),
                run: "R0003".into(),
            })
        }
        1 => {
            h.start_plan_run(serde_json::from_value(plan_run("D0003", false))?)?;
            if demote {
                h.start_plan_run(serde_json::from_value(plan_run("D0004", false))?)?;
            }
            Ok(Recipient::PlanRun {
                task: "T-001".into(),
                plan_run: "D0003".into(),
            })
        }
        2 => {
            h.push_review_child(
                "R0002",
                serde_json::from_value(review_child("T-001", "R0002", 77, 1, true, "approve"))?,
            )?;
            if demote {
                h.start_run(coding_run("T-001", "R0003", 1000, 1, 1, false))?;
            }
            Ok(Recipient::ReviewRun {
                task: "T-001".into(),
                run: "R0002".into(),
                review: "R0002-review0077".into(),
            })
        }
        3 => {
            h.push_plan_review(
                "P0002",
                serde_json::from_value(plan_review("PR-new", "rev", None))?,
            )?;
            if demote {
                h.push_plan(serde_json::from_value(plan("T-001", "P0003", 1))?)?;
            }
            Ok(Recipient::PlanReview {
                task: "T-001".into(),
                plan: "P0002".into(),
                review: "PR-new".into(),
            })
        }
        _ => unreachable!(),
    }
}

#[test]
fn queued_inbox_recipients_resolve_before_and_after_demotion() {
    for kind in 0..4 {
        for demote in [false, true] {
            for cursor in [false, true] {
                for (_dir, store) in backends() {
                    // Given a queued recipient of each kind, possibly already demoted.
                    rich(&store, 2, 2, 2, 1);
                    let value: SessionCursor = serde_json::from_value(serde_json::json!({
                        "adapter": "pi",
                        "binding": {"kind": "pi", "session": "nested-mutations"},
                        "cursor": {"offset": 77, "last_complete_line": 77, "coverage": "complete"}
                    }))
                    .unwrap();
                    // When either inbox helper addresses it within the same operation.
                    let (_, to) = store
                        .dispatch_update_head("T-001", |_, h| {
                            let to = queued_recipient(h, kind, demote)?;
                            if cursor {
                                h.set_observe_cursor(&to, value.clone())?;
                            } else {
                                h.mark_effect_applied(&to, 88)?;
                            }
                            Ok(to)
                        })
                        .unwrap_or_else(|e| {
                            panic!("kind={kind}, demote={demote}, cursor={cursor}: {e:#}")
                        });
                    // Then the persisted inbox and full-task summary agree with the request.
                    let head = store.head("T-001").unwrap();
                    let full_inbox = Inbox::new(&store, to.clone()).unwrap().state().unwrap();
                    if cursor {
                        assert!(full_inbox.observe_cursors.contains(&value));
                        assert_eq!(
                            head.observe_cursor(&to, &value.adapter, &value.binding)
                                .unwrap(),
                            Some(value),
                        );
                    } else {
                        assert!(full_inbox.effects_applied.contains(&88));
                        assert!(head.effect_applied(&to, 88).unwrap());
                    }
                    assert_eq!(
                        head.summary,
                        HistorySummary::of(&store.get("T-001").unwrap()).unwrap()
                    );
                }
            }
        }
    }
}
