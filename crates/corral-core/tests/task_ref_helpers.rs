//! Core helpers that read nested history, in their `TaskRef` forms (T-331 action-heads): equal
//! to the full-task functions on random tasks, and on heads reading only facts, the summary,
//! projections and keyed child rows, with identical counters as history grows.

mod store_support;
#[path = "task_ref_support/mod.rs"]
mod task_ref_support;

use corral_core::plan_policy::{
    revision_resolved, revision_resolved_ref, validate_approval, validate_approval_ref,
};
use corral_core::work::{self, Counter, HotScope, WorkSnapshot};
use corral_core::{
    Status, Store, Task, TaskCost, TaskRef, implementation_review_round,
    implementation_review_round_ref, plan_review_round, plan_review_round_ref,
};
use store_support::*;
use task_ref_support::*;

fn approval(store: &Store, full: &Task, task: &impl TaskRef) -> Result<(), String> {
    let result = match full.plans.last() {
        Some(_) => validate_approval_ref(store, &store.config, task),
        None => return Ok(()),
    };
    result.map_err(|e| format!("{e:#}"))
}

fn same_helpers(store: &Store, full: &Task, head: &corral_core::TaskHead) {
    let round = implementation_review_round(full);
    assert_eq!(implementation_review_round_ref(head).unwrap(), round);
    assert_eq!(implementation_review_round_ref(full).unwrap(), round);
    let round = plan_review_round(full);
    assert_eq!(plan_review_round_ref(head).unwrap(), round);
    assert_eq!(plan_review_round_ref(full).unwrap(), round);
    assert_eq!(
        revision_resolved_ref(head).unwrap(),
        revision_resolved(full)
    );
    assert_eq!(
        revision_resolved_ref(full).unwrap(),
        revision_resolved(full)
    );
    if let Some(plan) = full.plans.last() {
        let expected =
            validate_approval(store, &store.config, full, plan).map_err(|e| format!("{e:#}"));
        assert_eq!(approval(store, full, head), expected);
        assert_eq!(approval(store, full, full), expected);
    }
    // T-524: `of_head` adds per-run subtotals, `of` every entry in order; f64 sums can differ
    // in the last bit, so costs compare within rounding and the card text exactly.
    let (headed, expected) = (TaskCost::of_head(head), TaskCost::of(full));
    assert_eq!(headed.text(), expected.text());
    assert_eq!(
        (headed.unpriced_tokens, headed.ran),
        (expected.unpriced_tokens, expected.ran)
    );
    for (a, b) in [
        (headed.reported, expected.reported),
        (headed.estimated, expected.estimated),
    ] {
        assert_eq!(a.is_some(), b.is_some());
        assert!((a.unwrap_or(0.0) - b.unwrap_or(0.0)).abs() <= 1e-9);
    }
    let usage = full
        .current_run()
        .and_then(|r| r.total_usage())
        .map(corral_core::UsageTotals::of);
    assert_eq!(
        head.current_run
            .as_ref()
            .and_then(|r| r.facts.usage_total.clone()),
        usage
    );
}

#[test]
fn helper_forms_equal_the_full_task_functions_on_random_tasks() {
    let mut resolved = std::collections::BTreeSet::new();
    for seed in 1..=10u64 {
        let mut rng = Rng(0x5151_5151_0f0f_0f0f ^ (seed * 0x9e37_79b9));
        for (_backend, _dir, store) in backends() {
            for _ in 0..2 {
                let full = random_task(&store, &mut rng);
                let head = store.head(&full.id).unwrap();
                same_helpers(&store, &full, &head);
                resolved.insert(revision_resolved(&full));
            }
            let full = owner_changes(&store, 3, 3, 3);
            same_helpers(&store, &full, &store.head(&full.id).unwrap());
            resolved.insert(revision_resolved(&full));
        }
    }
    assert_eq!(
        resolved.len(),
        2,
        "revision_resolved took only {resolved:?}"
    );
}

/// A task whose current plan carries an owner `changes` review resolving every decision, its
/// history `runs`, review children and plan history grown as asked, nested collections fixed.
fn owner_changes(store: &Store, runs: usize, children: usize, history: usize) -> Task {
    let mut task = store
        .create("Helpers", Some("app"), Status::Running, "Spec\n")
        .unwrap();
    grow(&mut task, runs, children, history, 1);
    (task.created, task.updated) = (at(0), at(0));
    let plan = task.plans.last_mut().unwrap();
    let review = plan.reviews.last_mut().unwrap();
    review.reviewer = "owner".into();
    review.verdict = Some(corral_core::Verdict::Changes);
    review.notes = Some("Resolved: Ship? => yes".into());
    for run in &mut task.runs[..runs - 1] {
        for child in &mut run.review_runs {
            if let Some(review) = child.review.as_mut() {
                review.verdict = Some(corral_core::Verdict::Changes);
            }
        }
    }
    store.fixture_put_task(&task).unwrap();
    store.get(&task.id).unwrap()
}

fn measure(store: &Store, id: &str) -> WorkSnapshot {
    let head = store.head(id).unwrap();
    let _hot = HotScope::enter();
    let before = work::snapshot();
    assert!(revision_resolved_ref(&head).unwrap());
    implementation_review_round_ref(&head).unwrap();
    plan_review_round_ref(&head).unwrap();
    validate_approval_ref(store, &store.config, &head).unwrap_err();
    TaskCost::of_head(&head);
    let delta = work::snapshot().since(&before);
    delta.check_identity().unwrap();
    delta
}

#[test]
fn helper_forms_on_heads_cost_the_same_as_history_grows() {
    for (backend, _dir, store) in backends() {
        let base = owner_changes(&store, 2, 2, 2);
        let grown = owner_changes(&store, 20, 20, 20);
        assert!(implementation_review_round(&grown) > implementation_review_round(&base));
        let (small, large) = (measure(&store, &base.id), measure(&store, &grown.id));
        for counter in [
            Counter::TaskHeadDecodes,
            Counter::HistoryRowDecodes,
            Counter::NestedRowDecodes,
            Counter::RunDecodes,
            Counter::EventDecodes,
            Counter::RecordDecodes,
            Counter::ViewsOpened,
        ] {
            assert_eq!(
                small.get(counter),
                large.get(counter),
                "{backend} {counter:?}"
            );
        }
        assert!(
            small.get(Counter::NestedRowDecodes) > 0,
            "the open decision page was not read"
        );
        assert_eq!(small.full_task_hydrations(), 0);
        assert_eq!(large.full_task_hydrations(), 0);
        assert_eq!(
            small.unattributed_byte_remainder(),
            large.unattributed_byte_remainder(),
            "{backend} bytes"
        );
    }
}
