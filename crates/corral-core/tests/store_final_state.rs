//! Mutation invariants from the T-320 round-2 review: an operation is judged by its final
//! stored documents (no-ops, revisions of every touched container, usage settled before a
//! demotion folds it), queued children resolve as inbox recipients, and hot head reads stay
//! bounded and fully counted.

mod store_support;

use corral_core::backend::TaskPart;
use corral_core::inbox::Recipient;
use corral_core::work::{self, Counter, HotScope};
use corral_core::{HistorySummary, Status, Task, TaskHeadMut};
use store_support::*;

type Step = fn(&mut TaskHeadMut, &Task) -> anyhow::Result<()>;

#[test]
fn identical_final_documents_leave_rows_revisions_and_feed_unchanged() {
    let cases: [(&str, Step); 11] = [
        ("old aggregate review no-op", |h, _| {
            h.edit_aggregate_review("A0002", "A0002-R0001", |_| Ok(()))
        }),
        ("same run usage", |h, t| {
            h.set_usage("R0002", None, t.runs[1].usage.clone().unwrap())
        }),
        ("empty run usage append", |h, _| {
            h.append_usage_entries("R0002", None, Vec::new())
        }),
        ("same child usage", |h, t| {
            let child = &t.runs[1].review_runs[1];
            h.set_usage("R0002", Some(&child.id), child.usage.clone().unwrap())
        }),
        ("empty child usage append", |h, t| {
            h.append_usage_entries("R0002", Some(&t.runs[1].review_runs[1].id), Vec::new())
        }),
        ("wait A-B-A in one write", |h, _| {
            h.edit_wait("R0002", "W0000", |w| {
                w.observation.detail = "B".into();
                Ok(())
            })?;
            h.edit_wait("R0002", "W0000", |w| {
                w.observation.detail = "observed".into();
                Ok(())
            })
        }),
        ("decision A-B-A in one write", |h, _| {
            h.edit_decision("R0002", "D0000", |d| {
                d.observation.detail = "B".into();
                Ok(())
            })?;
            h.edit_decision("R0002", "D0000", |d| {
                d.observation.detail = "asked".into();
                Ok(())
            })
        }),
        ("child A-B-A in one write", |h, t| {
            let id = &t.runs[1].review_runs[1].id;
            h.edit_review_child("R0002", id, |c| {
                c.summary = Some("B".into());
                Ok(())
            })?;
            h.edit_review_child("R0002", id, |c| {
                c.summary = None;
                Ok(())
            })
        }),
        ("plan review A-B-A in one write", |h, t| {
            let id = &t.plans[1].reviews[0].id;
            h.edit_plan_review("P0002", id, |r| {
                r.notes = Some("B".into());
                Ok(())
            })?;
            h.edit_plan_review("P0002", id, |r| {
                r.notes = None;
                Ok(())
            })
        }),
        ("aggregate A-B-A in one write", |h, _| {
            h.edit_aggregate_review("A0002", "A0002-R0001", |r| {
                r.evidence_digest = "B".into();
                Ok(())
            })?;
            h.edit_aggregate_review("A0002", "A0002-R0001", |r| {
                r.evidence_digest = "e".into();
                Ok(())
            })
        }),
        ("plan review push-pop", |h, _| {
            let review = serde_json::from_value(plan_review("PR-new", "rev", None)).unwrap();
            h.push_plan_review("P0002", review)?;
            h.pop_current_plan_review("P0002")
        }),
    ];
    let mut failures = Vec::new();
    for (label, step) in cases {
        // Given a complete stored task and its original canonical rows.
        let (_dir, store) = store();
        let before = rich(&store, 2, 2, 2, 1);
        let rows = task_rows(&store);
        let seq = store.view().unwrap().seq();
        let facts = store.head("T-001").unwrap().current_run.unwrap().facts;
        // When a logical mutation finishes with identical domain documents.
        store.update_head("T-001", |h| step(h, &before)).unwrap();
        // Then it must not manufacture writes or revision conflicts.
        let mut after = store.get("T-001").unwrap();
        after.updated = before.updated;
        let same_task = after == before;
        let same_rows = task_rows(&store) == rows;
        let same_seq = store.view().unwrap().seq() == seq;
        let new_facts = store.head("T-001").unwrap().current_run.unwrap().facts;
        println!(
            "{label}: task_equal={same_task}, rows_equal={same_rows}, seq_equal={same_seq}, children_rev={}->{}, obligations_rev={}->{}",
            facts.children_rev,
            new_facts.children_rev,
            facts.obligations_rev,
            new_facts.obligations_rev
        );
        if !same_task || !same_rows || !same_seq {
            failures.push(label);
        }
    }
    assert!(
        failures.is_empty(),
        "identical final documents wrote: {failures:?}"
    );
}

#[test]
fn inbox_mutations_see_a_just_pushed_plan_review() {
    // Given a task whose next plan review exists only in the mutation's projection.
    let (_dir, store) = store();
    rich(&store, 2, 2, 2, 1);
    let review = serde_json::from_value(plan_review("PR-new", "rev", None)).unwrap();
    let to = Recipient::PlanReview {
        task: "T-001".into(),
        plan: "P0002".into(),
        review: "PR-new".into(),
    };
    // When that review's inbox is mutated in the same logical operation.
    let result = store.update_head("T-001", |h| {
        h.push_plan_review("P0002", review)?;
        h.mark_effect_applied(&to, 88)
    });
    // Then the queued review must be resolvable.
    println!("pushed plan review inbox result: {result:?}");
    assert!(
        result.is_ok(),
        "the mutation cannot resolve its queued recipient"
    );
    assert!(
        store
            .head("T-001")
            .unwrap()
            .effect_applied(&to, 88)
            .unwrap()
    );
}

#[test]
fn plan_child_revision_advances_only_for_its_stored_document() {
    // Given a plan and the exact revision of its review rows.
    let (_dir, store) = store();
    rich(&store, 2, 2, 2, 1);
    let old = task_rows(&store);
    let before = store
        .head("T-001")
        .unwrap()
        .last_plan
        .unwrap()
        .facts
        .children_rev;
    // When only a nested inbox effect of a plan review changes.
    store
        .update_head("T-001", |h| {
            h.edit_plan_review("P0002", "P0002-PR0001", |r| {
                r.inbox.effects_applied.insert(88);
                Ok(())
            })
        })
        .unwrap();
    // Then the plan review's canonical row is equal and its revision must stay equal.
    let new = task_rows(&store);
    let row_equal = old[&(TaskPart::PlanReview, "000002/000001".into())]
        == new[&(TaskPart::PlanReview, "000002/000001".into())];
    let after = store
        .head("T-001")
        .unwrap()
        .last_plan
        .unwrap()
        .facts
        .children_rev;
    println!("plan inbox-only edit: review_row_equal={row_equal}, children_rev={before}->{after}");
    assert!(row_equal);
    assert_eq!(
        after, before,
        "a nested inbox row is not a changed review row"
    );
}

#[test]
fn demotion_folds_usage_changed_in_the_same_mutation() {
    // Given a coding run whose usage is about to change.
    let (_dir, store) = store();
    rich(&store, 2, 2, 2, 1);
    let new = coding_run("T-001", "R0003", 1000, 1, 1, false);
    let usage = corral_core::Usage::from_entries(vec![
        serde_json::from_value(usage_entry(9, true)).unwrap(),
        serde_json::from_value(usage_entry(10, true)).unwrap(),
    ]);
    // When the usage is replaced and the run demoted before finish.
    store
        .update_head("T-001", |h| {
            h.set_usage("R0002", None, usage)?;
            h.start_run(new)
        })
        .unwrap();
    // Then the demotion's summary must derive from the final stored history.
    let head = store.head("T-001").unwrap();
    let expected = HistorySummary::of_record(&store.task_record("T-001").unwrap()).unwrap();
    println!(
        "demoted run summary: stored_tokens={}, expected_tokens={}, stored_cost={:?}, expected_cost={:?}",
        head.summary.tokens.input, expected.tokens.input, head.summary.cost, expected.cost
    );
    assert_eq!(head.summary, expected);
}

#[test]
fn operating_revision_uses_no_historical_rows() {
    let mut reports = Vec::new();
    for history in [10, 100] {
        // Given equal-sized heads whose scope amendment histories differ tenfold.
        let (_dir, store) = store();
        let mut task = store.create("Scoped", None, Status::Backlog, "").unwrap();
        task.approved_scope_history = (0..history)
            .map(|n| {
                serde_json::from_value(serde_json::json!({
                    "revision": n, "approved_scope": null, "superseded_at": at(n)
                }))
                .unwrap()
            })
            .collect();
        store.fixture_put_task(&task).unwrap();
        let head = store.head("T-001").unwrap();
        let before = work::snapshot();
        // When computing the head's contract revision on a hot path.
        {
            let _hot = HotScope::enter();
            assert_eq!(
                head.operating_contract_revision(),
                corral_core::operating::operating_contract_revision(&task).unwrap()
            );
        }
        // Then it must not decode history to compute the head digest.
        let counted = work::snapshot().since(&before);
        counted.check_identity().unwrap();
        reports.push(counted.get(Counter::HistoryRowDecodes));
    }
    println!("head operating revision history decodes at 10/100 amendments: {reports:?}");
    assert_eq!(
        reports[0], reports[1],
        "the head digest must not grow with amendment history"
    );
}

#[test]
fn replacing_checkpoint_content_deletes_the_old_unreferenced_payload() {
    // Given a child checkpoint whose payload no other metadata references.
    let (_dir, store) = store();
    let task = rich(&store, 2, 2, 2, 1);
    let child = &task.runs[1].review_runs[1];
    let old_digest = corral_core::CheckpointMeta::of(1, &child.checkpoints[0])
        .unwrap()
        .payload_digest;
    // When a keyed child edit replaces that checkpoint's content at the same ordinal.
    store
        .update_head("T-001", |h| {
            h.edit_review_child("R0002", &child.id, |c| {
                c.checkpoints[0].checkpoint.input.next_action = "replaced payload".into();
                Ok(())
            })
        })
        .unwrap();
    // Then the old payload has no references and must be deleted in the same operation.
    let rows = task_rows(&store);
    let refs = rows
        .values()
        .filter(|r| {
            r.part == TaskPart::RunCheckpoint
                && r.doc["payload_digest"].as_str() == Some(&old_digest)
        })
        .count();
    let kept = rows.contains_key(&(TaskPart::CheckpointPayload, old_digest.clone()));
    println!("replaced checkpoint: old_payload_references={refs}, old_payload_retained={kept}");
    assert_eq!(refs, 0);
    assert!(!kept, "old payload survived its last metadata reference");
}

#[test]
fn each_touched_container_revision_advances_once_from_its_original_document() {
    let cases: [(&str, Step, TaskPart, &str); 4] = [
        (
            "earlier child",
            |h, t| {
                let id = &t.runs[1].review_runs[0].id;
                for detail in ["B", "C"] {
                    h.edit_review_child("R0002", id, |c| {
                        c.obligations.waits[0].observation.detail = detail.into();
                        Ok(())
                    })?;
                }
                Ok(())
            },
            TaskPart::ReviewRun,
            "000002/000001",
        ),
        (
            "current child",
            |h, t| {
                let id = &t.runs[1].review_runs[1].id;
                for detail in ["B", "C"] {
                    h.edit_review_child("R0002", id, |c| {
                        c.obligations.waits[0].observation.detail = detail.into();
                        Ok(())
                    })?;
                }
                Ok(())
            },
            TaskPart::ReviewRun,
            "000002/000002",
        ),
        (
            "demoted run",
            |h, _| {
                for detail in ["B", "C"] {
                    h.edit_wait("R0002", "W0000", |w| {
                        w.observation.detail = detail.into();
                        Ok(())
                    })?;
                }
                h.start_run(coding_run("T-001", "R0003", 1000, 1, 1, false))
            },
            TaskPart::Run,
            "000002",
        ),
        (
            "current run",
            |h, _| {
                for detail in ["B", "C"] {
                    h.edit_wait("R0002", "W0000", |w| {
                        w.observation.detail = detail.into();
                        Ok(())
                    })?;
                }
                Ok(())
            },
            TaskPart::Run,
            "000002",
        ),
    ];
    let mut failures = Vec::new();
    for (label, step, part, key) in cases {
        // Given the container's original obligations revision.
        let (_dir, store) = store();
        let task = rich(&store, 2, 2, 2, 1);
        let row_key = (part, key.to_owned());
        let before = revision(&task_rows(&store)[&row_key], "obligations_rev");
        // When two edits of that document are committed in one operation.
        store.update_head("T-001", |h| step(h, &task)).unwrap();
        // Then its revision advances once, regardless of whether it is still current.
        let after = revision(&task_rows(&store)[&row_key], "obligations_rev");
        println!("{label}: obligations_rev={before}->{after}");
        if after != before + 1 {
            failures.push(label);
        }
    }
    assert!(
        failures.is_empty(),
        "revisions advanced per helper: {failures:?}"
    );
}

#[test]
fn the_header_inventory_and_page_lookahead_decode_no_documents() {
    // Given two tasks, the second with an undecodable stored head document.
    let (dir, store) = store();
    store.create("first", None, Status::Backlog, "").unwrap();
    store.create("second", None, Status::Backlog, "").unwrap();
    let backend = corral_core::sqlite::SqliteBackend::open(
        &dir.path().join(".corral"),
        corral_core::sqlite::SqliteOptions::default(),
    )
    .unwrap();
    backend
        .fixture_raw_task_state(corral_core::backend::TaskNum(2), "{")
        .unwrap();
    // When a one-head page stops before it and the id inventory lists both tasks.
    let page = store
        .view()
        .unwrap()
        .heads(
            &corral_core::backend::HeadQuery::Open { project: None },
            corral_core::backend::PageReq::first(1),
        )
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert!(page.next.is_some(), "a further page is still reported");
    let before = work::snapshot();
    let created = {
        let _hot = HotScope::enter();
        store.create("third", None, Status::Backlog, "").unwrap()
    };
    // Then neither decoded the stored document: the lookahead row and the inventory are
    // header-only, so nothing went uncounted.
    assert_eq!(created.id, "T-003");
    let delta = work::snapshot().since(&before);
    delta.check_identity().unwrap();
    assert_eq!(delta.get(Counter::TaskHeadDecodes), 0);
}

fn both_backends() -> Vec<(tempfile::TempDir, corral_core::Store)> {
    use corral_core::backend::fake::FakeBackend;
    use corral_core::backend::{BackendConfig, FaultInjector, ManualClock};
    use std::sync::Arc;
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
fn every_usage_change_is_settled_before_its_run_is_folded() {
    let cases: [(&str, Step); 4] = [
        ("set_usage", |h, _| {
            h.set_usage(
                "R0002",
                None,
                corral_core::Usage::from_entries(vec![
                    serde_json::from_value(usage_entry(9, true)).unwrap(),
                    serde_json::from_value(usage_entry(10, true)).unwrap(),
                ]),
            )
        }),
        ("append_usage_entries", |h, _| {
            h.append_usage_entries(
                "R0002",
                None,
                vec![serde_json::from_value(usage_entry(11, true)).unwrap()],
            )
        }),
        ("child usage edit", |h, t| {
            let id = &t.runs[1].review_runs[1].id;
            h.append_usage_entries(
                "R0002",
                Some(id),
                vec![serde_json::from_value(usage_entry(12, true)).unwrap()],
            )
        }),
        ("pushed child with usage", |h, _| {
            let mut child: corral_core::Run =
                serde_json::from_value(review_child("T-001", "R0002", 77, 1, true, "approve"))
                    .unwrap();
            child.usage = Some(corral_core::Usage::from_entries(vec![
                serde_json::from_value(usage_entry(13, true)).unwrap(),
            ]));
            h.push_review_child("R0002", child)
        }),
    ];
    for (label, step) in cases {
        for (_dir, store) in both_backends() {
            // Given a run whose usage changes in the same operation that demotes it.
            let task = rich(&store, 2, 2, 2, 1);
            store
                .update_head("T-001", |h| {
                    step(h, &task)?;
                    h.start_run(coding_run("T-001", "R0003", 1000, 1, 1, false))
                })
                .unwrap();
            // Then the folded summary matches the final stored history.
            let stored = store.head("T-001").unwrap().summary;
            let expected = HistorySummary::of_record(&store.task_record("T-001").unwrap()).unwrap();
            assert_eq!(stored, expected, "{label}");
        }
    }
}

#[test]
fn a_child_pushed_and_demoted_in_one_operation_is_stored() {
    for (_dir, store) in both_backends() {
        // Given a task with a current plan and aggregate attempt.
        rich(&store, 2, 2, 2, 1);
        let review: corral_core::PlanReview =
            serde_json::from_value(plan_review("PR-new", "rev", None)).unwrap();
        let aggregate: corral_core::AggregateReview =
            serde_json::from_value(aggregate_review("A0002-R0009", true)).unwrap();
        let plan: corral_core::PlanRecord =
            serde_json::from_value(store_support::plan("T-001", "P0003", 1)).unwrap();
        let attempt: corral_core::AggregateAttempt =
            serde_json::from_value(store_support::attempt("T-001", "A0003", 1)).unwrap();
        // When a review is pushed and its container demoted in the same operation.
        store
            .update_head("T-001", |h| {
                h.push_plan_review("P0002", review.clone())?;
                h.push_plan(plan.clone())?;
                h.push_aggregate_review("A0002", aggregate.clone())?;
                h.push_aggregate_attempt(attempt.clone())
            })
            .unwrap();
        // Then the demoted containers keep the pushed reviews, and the summary agrees.
        let task = store.get("T-001").unwrap();
        assert_eq!(task.plans[1].reviews.last().unwrap(), &review);
        assert_eq!(
            task.aggregate_attempts[1].reviews.last().unwrap(),
            &aggregate
        );
        assert_eq!(
            store.head("T-001").unwrap().summary,
            HistorySummary::of_record(&store.task_record("T-001").unwrap()).unwrap()
        );
    }
}
