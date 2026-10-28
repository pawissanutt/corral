//! `TaskRef` on a head read from a view against the same method on the full task (T-331
//! action-heads), on the fake and SQLite backends: equal results everywhere, keyed lookups
//! decoding at most one history row, child reads only the rows they return.

mod store_support;
#[path = "task_ref_support/mod.rs"]
mod task_ref_support;

use corral_core::agent::{ContainerRef, RequestBinding, check_live, check_live_ref};
use corral_core::inbox::Recipient;
use corral_core::work::{self, Counter, HotScope, WorkSnapshot};
use corral_core::{FindingSource, Task, TaskFieldsRef, TaskHead, TaskRef};
use store_support::Rng;
use task_ref_support::*;

fn measure<T>(f: impl FnOnce() -> T) -> (T, WorkSnapshot) {
    let _hot = HotScope::enter();
    let before = work::snapshot();
    let value = f();
    let delta = work::snapshot().since(&before);
    delta.check_identity().unwrap();
    (value, delta)
}

fn recipients(task: &Task) -> Vec<Recipient> {
    let id = task.id.clone();
    let mut out = vec![Recipient::Run {
        task: id.clone(),
        run: "missing".into(),
    }];
    for run in &task.runs {
        out.push(Recipient::Run {
            task: id.clone(),
            run: run.id.clone(),
        });
        for child in &run.review_runs {
            out.push(Recipient::ReviewRun {
                task: id.clone(),
                run: run.id.clone(),
                review: child.id.clone(),
            });
        }
    }
    for run in &task.plan_runs {
        out.push(Recipient::PlanRun {
            task: id.clone(),
            plan_run: run.id.clone(),
        });
    }
    for plan in &task.plans {
        for review in &plan.reviews {
            out.push(Recipient::PlanReview {
                task: id.clone(),
                plan: plan.id.clone(),
                review: review.id.clone(),
            });
        }
    }
    out
}

fn containers(task: &Task) -> Vec<ContainerRef> {
    let mut out = vec![ContainerRef::Pm];
    for run in &task.runs {
        out.push(ContainerRef::Worker {
            task: task.id.clone(),
            run: run.id.clone(),
        });
        for child in run
            .review_runs
            .iter()
            .map(|c| c.id.clone())
            .chain(["x".into()])
        {
            out.push(ContainerRef::Reviewer {
                task: task.id.clone(),
                run: run.id.clone(),
                review: child,
            });
        }
    }
    out.push(ContainerRef::Worker {
        task: "T-999".into(),
        run: "R0001".into(),
    });
    out
}

fn same_projections(full: &Task, head: &TaskHead) {
    let run = |t: &dyn Fn() -> Option<corral_core::RunHead>| t().map(norm_run);
    assert_eq!(
        run(&|| full.current_run_head().unwrap().map(|c| c.into_owned())),
        run(&|| head.current_run_head().unwrap().map(|c| c.into_owned())),
        "current run"
    );
    assert_eq!(
        full.last_plan_run_head()
            .unwrap()
            .map(|c| norm_plan_run(c.into_owned())),
        head.last_plan_run_head()
            .unwrap()
            .map(|c| norm_plan_run(c.into_owned()))
    );
    assert_eq!(
        full.last_plan_head()
            .unwrap()
            .map(|c| norm_plan(c.into_owned())),
        head.last_plan_head()
            .unwrap()
            .map(|c| norm_plan(c.into_owned()))
    );
    assert_eq!(
        full.last_aggregate_attempt_head()
            .unwrap()
            .map(|c| norm_attempt(c.into_owned())),
        head.last_aggregate_attempt_head()
            .unwrap()
            .map(|c| norm_attempt(c.into_owned()))
    );
    assert_eq!(
        full.history_summary().unwrap(),
        head.history_summary().unwrap()
    );
    assert_eq!(full.head_fields().unwrap(), head.head_fields().unwrap());
    assert_eq!(
        full.contract_revision().unwrap(),
        head.contract_revision().unwrap()
    );
    for (a, b) in [
        (full.runs_len(), head.runs_len()),
        (full.plan_runs_len(), head.plan_runs_len()),
        (full.plans_len(), head.plans_len()),
        (full.aggregate_attempts_len(), head.aggregate_attempts_len()),
        (full.comments_len(), head.comments_len()),
    ] {
        assert_eq!(a.unwrap(), b.unwrap());
    }
    assert_eq!(full.id(), head.id());
    assert_eq!(full.status(), head.status());
    assert_eq!(full.body(), head.body());
    assert_eq!(full.shell_task().unwrap(), head.shell_task().unwrap());
}

/// Asserts equal results, and that the head's read decoded at most `history` history rows and
/// exactly the nested rows `nested` says it returned.
fn keyed<T: PartialEq + std::fmt::Debug>(
    what: &str,
    full: impl Fn() -> T,
    head: impl Fn() -> T,
    history: u64,
    nested: impl Fn(&T) -> u64,
) {
    let expected = full();
    let (got, delta) = measure(head);
    assert_eq!(expected, got, "{what}");
    assert!(
        delta.get(Counter::HistoryRowDecodes) <= history,
        "{what}: {} history rows",
        delta.get(Counter::HistoryRowDecodes)
    );
    assert_eq!(
        delta.get(Counter::NestedRowDecodes),
        nested(&got),
        "{what}: nested rows"
    );
    assert_eq!(delta.full_task_hydrations(), 0, "{what}: hydrated");
}

fn none(_: &impl std::fmt::Debug) -> u64 {
    0
}

fn count<T>(v: &Option<T>) -> u64 {
    u64::from(v.is_some())
}

fn same_keyed_reads(full: &Task, head: &TaskHead) {
    let current = full.runs.last().map(|r| r.id.clone()).unwrap_or_default();
    let mut runs: Vec<String> = full.runs.iter().map(|r| r.id.clone()).collect();
    runs.push("missing".into());
    for run in &runs {
        keyed(
            "run_head",
            || full.run_head(run).unwrap().map(norm_run),
            || head.run_head(run).unwrap().map(norm_run),
            1,
            none,
        );
        keyed(
            "artifacts_of",
            || full.artifacts_of(run).unwrap(),
            || head.artifacts_of(run).unwrap(),
            full.artifacts.iter().filter(|a| a.run == *run).count() as u64,
            none,
        );
        let history = u64::from(*run != current);
        let source = full.runs.iter().find(|r| r.id == *run);
        let mut children: Vec<String> = source
            .map(|r| r.review_runs.iter().map(|c| c.id.clone()).collect())
            .unwrap_or_default();
        children.push("missing".into());
        for child in &children {
            keyed(
                "review_child",
                || full.review_child(run, child).unwrap().map(norm_run),
                || head.review_child(run, child).unwrap().map(norm_run),
                history,
                count,
            );
        }
        for at in source.into_iter().flat_map(|r| {
            r.review_runs
                .iter()
                .filter_map(|c| c.review.as_ref().map(|r| r.at))
        }) {
            keyed(
                "review_child_by_review_at",
                || {
                    full.review_child_by_review_at(run, at)
                        .unwrap()
                        .map(norm_run)
                },
                || {
                    head.review_child_by_review_at(run, at)
                        .unwrap()
                        .map(norm_run)
                },
                history,
                count,
            );
        }
        for exclude in [None, children.first().map(String::as_str)] {
            let expected = full.stronger_source(run, "abc", exclude).unwrap();
            let (got, delta) = measure(|| head.stronger_source(run, "abc", exclude).unwrap());
            assert_eq!(expected, got, "stronger_source");
            assert!(delta.get(Counter::NestedRowDecodes) <= 2);
        }
        keyed(
            "open_review_children",
            || {
                full.open_review_children(run)
                    .unwrap()
                    .into_iter()
                    .map(norm_run)
                    .collect()
            },
            || {
                head.open_review_children(run)
                    .unwrap()
                    .into_iter()
                    .map(norm_run)
                    .collect()
            },
            history,
            |v: &Vec<_>| v.len() as u64,
        );
        let ids: Vec<String> = source
            .into_iter()
            .flat_map(|r| {
                let waits = r.obligations.waits.iter().map(|w| w.request.id.clone());
                let decisions = r.obligations.decisions.iter().map(|d| d.request.id.clone());
                waits.chain(decisions).collect::<Vec<_>>()
            })
            .chain(["missing".to_owned()])
            .collect();
        for id in &ids {
            keyed(
                "wait",
                || full.wait(run, id).unwrap(),
                || head.wait(run, id).unwrap(),
                history,
                count,
            );
            keyed(
                "decision",
                || full.decision(run, id).unwrap(),
                || head.decision(run, id).unwrap(),
                history,
                count,
            );
            let receipt = format!("rc-{id}");
            keyed(
                "receipt",
                || full.receipt(run, &receipt).unwrap(),
                || head.receipt(run, &receipt).unwrap(),
                history,
                count,
            );
        }
        keyed(
            "open_obligations",
            || full.open_obligations(run).unwrap(),
            || head.open_obligations(run).unwrap(),
            history,
            |o| (o.waits.len() + o.decisions.len()) as u64,
        );
        let artifacts: Vec<String> = source
            .into_iter()
            .flat_map(|r| r.checkpoints.iter().map(|c| c.artifact.clone()))
            .chain(["missing".to_owned()])
            .collect();
        for artifact in &artifacts {
            keyed(
                "checkpoint_by_artifact",
                || full.checkpoint_by_artifact(run, artifact).unwrap(),
                || head.checkpoint_by_artifact(run, artifact).unwrap(),
                history,
                count,
            );
        }
        let metas = |t: &dyn TaskRef| -> Vec<_> {
            t.checkpoint_metas_newest_first(run)
                .unwrap()
                .map(Result::unwrap)
                .collect()
        };
        keyed(
            "checkpoint_metas",
            || metas(full),
            || metas(head),
            history,
            |v: &Vec<_>| v.len() as u64,
        );
        let ordinals = source.map_or(0, |r| r.checkpoints.len()) as u64 + 1;
        for ordinal in 1..=ordinals {
            keyed(
                "checkpoint_payload",
                || full.checkpoint_payload(run, ordinal).unwrap(),
                || head.checkpoint_payload(run, ordinal).unwrap(),
                history,
                |p| 2 * count(p),
            );
        }
        assert_eq!(
            full.emitted_obligations(run).unwrap(),
            head.emitted_obligations(run).unwrap()
        );
    }
    let mut plans: Vec<String> = full.plans.iter().map(|p| p.id.clone()).collect();
    plans.push("missing".into());
    let current_plan = full.plans.last().map(|p| p.id.clone()).unwrap_or_default();
    for plan in &plans {
        keyed(
            "plan_head",
            || full.plan_head(plan).unwrap().map(norm_plan),
            || head.plan_head(plan).unwrap().map(norm_plan),
            1,
            none,
        );
        assert_eq!(full.has_plan(plan).unwrap(), head.has_plan(plan).unwrap());
        let history = u64::from(*plan != current_plan);
        let source = full.plans.iter().find(|p| p.id == *plan);
        let reviews = source.map_or(0, |p| p.reviews.len());
        for review in source
            .into_iter()
            .flat_map(|p| p.reviews.iter().map(|r| r.id.clone()))
            .chain(["missing".to_owned()])
        {
            keyed(
                "plan_review",
                || full.plan_review(plan, &review).unwrap(),
                || head.plan_review(plan, &review).unwrap(),
                history,
                count,
            );
        }
        for n in 0..=reviews + 1 {
            keyed(
                "plan_review_at",
                || full.plan_review_at(plan, n).unwrap(),
                || head.plan_review_at(plan, n).unwrap(),
                history,
                count,
            );
        }
        for reviewer in ["rev", "unnamed", "owner"] {
            keyed(
                "plan_review_approved_by",
                || full.plan_review_approved_by(plan, reviewer).unwrap(),
                || head.plan_review_approved_by(plan, reviewer).unwrap(),
                history,
                count,
            );
        }
        keyed(
            "pending_plan_reviews",
            || full.pending_plan_reviews(plan).unwrap(),
            || head.pending_plan_reviews(plan).unwrap(),
            history,
            |v: &Vec<_>| v.len() as u64,
        );
    }
    for run in full
        .plan_runs
        .iter()
        .map(|r| r.id.clone())
        .chain(["missing".into()])
    {
        keyed(
            "plan_run_head",
            || full.plan_run_head(&run).unwrap().map(norm_plan_run),
            || head.plan_run_head(&run).unwrap().map(norm_plan_run),
            1,
            none,
        );
    }
    let current_attempt = full
        .aggregate_attempts
        .last()
        .map(|a| a.id.clone())
        .unwrap_or_default();
    for attempt in full
        .aggregate_attempts
        .iter()
        .map(|a| a.id.clone())
        .chain(["missing".into()])
    {
        keyed(
            "aggregate_attempt_head",
            || {
                full.aggregate_attempt_head(&attempt)
                    .unwrap()
                    .map(norm_attempt)
            },
            || {
                head.aggregate_attempt_head(&attempt)
                    .unwrap()
                    .map(norm_attempt)
            },
            1,
            none,
        );
        let history = u64::from(attempt != current_attempt);
        let source = full.aggregate_attempts.iter().find(|a| a.id == attempt);
        for review in source
            .into_iter()
            .flat_map(|a| a.reviews.iter().map(|r| r.id.clone()))
            .chain(["missing".to_owned()])
        {
            keyed(
                "aggregate_review",
                || full.aggregate_review(&attempt, &review).unwrap(),
                || head.aggregate_review(&attempt, &review).unwrap(),
                history,
                count,
            );
        }
        keyed(
            "pending_aggregate_reviews",
            || full.pending_aggregate_reviews(&attempt).unwrap(),
            || head.pending_aggregate_reviews(&attempt).unwrap(),
            history,
            |v: &Vec<_>| v.len() as u64,
        );
    }
    keyed(
        "previous_run_head",
        || full.previous_run_head().unwrap().map(norm_run),
        || head.previous_run_head().unwrap().map(norm_run),
        1,
        none,
    );
}

fn same_inbox_and_agent_reads(full: &Task, head: &TaskHead) {
    for recipient in recipients(full) {
        assert_eq!(
            full.inbox_head(&recipient).unwrap(),
            head.inbox_head(&recipient).unwrap(),
            "{recipient:?}"
        );
        for seq in [1, 2, 7] {
            assert_eq!(
                full.effect_applied(&recipient, seq).unwrap(),
                head.effect_applied(&recipient, seq).unwrap()
            );
        }
        for session in ["s-0", "s-1", "none"] {
            let binding = corral_core::inbox::SessionBinding {
                session: session.into(),
                endpoint: None,
                transcript: None,
            };
            assert_eq!(
                full.observe_cursor(&recipient, "pi", &binding).unwrap(),
                head.observe_cursor(&recipient, "pi", &binding).unwrap()
            );
        }
    }
    for container in containers(full) {
        // A review child that is not the current one is first found by its keyed row.
        let resolve = u64::from(
            matches!(&container, ContainerRef::Reviewer { run, review, .. }
            if full.runs.iter().any(|r| r.id == *run)
                && full.runs.last().and_then(|r| r.review_runs.last()).is_none_or(|c| c.id != *review)),
        );
        for n in 0..=3 {
            let expected = full.agent_generation(&container, n).unwrap();
            let (got, delta) = measure(|| head.agent_generation(&container, n).unwrap());
            assert_eq!(expected, got, "agent_generation {container:?} {n}");
            assert!(delta.get(Counter::HistoryRowDecodes) <= 1);
            assert!(delta.get(Counter::NestedRowDecodes) <= count(&got) + resolve);
            for seq in 0..=2 {
                let expected = full.agent_turn(&container, n, seq).unwrap();
                let (got, delta) = measure(|| head.agent_turn(&container, n, seq).unwrap());
                assert_eq!(expected, got, "agent_turn {container:?} {n} {seq}");
                assert!(delta.get(Counter::NestedRowDecodes) <= count(&got) + resolve);
            }
        }
    }
}

fn same_emitted_pages(full: &Task, head: &TaskHead) {
    let (comments, delta) = measure(|| head.emitted_comments().unwrap());
    assert_eq!(full.emitted_comments().unwrap(), comments);
    assert_eq!(delta.emitted_rows(), comments.len() as u64);
    let excludes: Vec<Option<String>> = std::iter::once(None)
        .chain(
            full.runs
                .iter()
                .flat_map(|r| r.review_runs.iter().map(|c| Some(c.id.clone()))),
        )
        .chain(
            full.plans
                .iter()
                .flat_map(|p| p.reviews.iter().map(|r| Some(r.id.clone()))),
        )
        .collect();
    for source in [FindingSource::Review, FindingSource::Plan] {
        for exclude in &excludes {
            let (found, delta) =
                measure(|| head.emitted_findings(source, exclude.as_deref()).unwrap());
            assert_eq!(
                full.emitted_findings(source, exclude.as_deref()).unwrap(),
                found,
                "{source:?} findings"
            );
            assert_eq!(delta.emitted_rows(), found.len() as u64);
            assert!(delta.emitted_doc_bytes() > 0 || found.is_empty());
        }
    }
}

fn bindings(full: &Task, rng: &mut Rng) -> Vec<RequestBinding> {
    let mut out = Vec::new();
    for container in containers(full) {
        for _ in 0..4 {
            out.push(RequestBinding {
                container: container.clone(),
                generation: rng.below(4) as u32,
                callback: [None, Some(CALLBACK.to_owned()), Some("other".into())][rng.below(3)]
                    .clone(),
                settles_turn: [None, Some(1), Some(2)][rng.below(3)],
            });
        }
    }
    out
}

#[test]
fn every_task_ref_read_on_a_head_equals_the_full_task_on_both_backends() {
    let mut refusals = std::collections::BTreeSet::new();
    for seed in 1..=12u64 {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ (seed * 0x2545_f491));
        for (backend, _dir, store) in backends() {
            for _ in 0..2 {
                let full = random_task(&store, &mut rng);
                let head = store.head(&full.id).unwrap();
                same_projections(&full, &head);
                same_keyed_reads(&full, &head);
                same_inbox_and_agent_reads(&full, &head);
                same_emitted_pages(&full, &head);
                for binding in bindings(&full, &mut rng) {
                    let expected = check_live(&full, &binding);
                    assert_eq!(
                        check_live_ref(&head, &binding).unwrap(),
                        expected,
                        "{backend} {binding:?}"
                    );
                    assert_eq!(check_live_ref(&full, &binding).unwrap(), expected);
                    refusals.insert(
                        expected
                            .map_err(|r| r.code.as_str())
                            .err()
                            .unwrap_or("live"),
                    );
                }
            }
        }
    }
    for code in ["live", "agent_fenced", "run_ended", "review_closed"] {
        assert!(
            refusals.contains(code),
            "{code} never decided: {refusals:?}"
        );
    }
}
