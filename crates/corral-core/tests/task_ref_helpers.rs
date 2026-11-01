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

fn assert_costs_close(headed: TaskCost, expected: TaskCost) {
    assert_eq!(
        (headed.unpriced_tokens, headed.ran),
        (expected.unpriced_tokens, expected.ran)
    );
    for (a, b) in [
        (headed.reported, expected.reported),
        (headed.estimated, expected.estimated),
    ] {
        match (a, b) {
            (Some(a), Some(b)) => assert!(
                (a - b).abs() <= 1e-9 * 1.0_f64.max(a.abs()).max(b.abs()),
                "{a:?} vs {b:?}"
            ),
            (None, None) => {}
            _ => panic!("cost presence differs: {a:?} vs {b:?}"),
        }
    }
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
    assert_costs_close(TaskCost::of_head(head), TaskCost::of(full));
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
                let mut full = random_task(&store, &mut rng);
                for run in &mut full.runs {
                    let amount = (1 + rng.below(90)) as f64 / 100.0;
                    run.usage = Some(corral_core::Usage::from_entries(vec![
                        corral_core::UsageEntry {
                            reported_usd: Some(amount),
                            estimated_usd: Some(9.0),
                            ..Default::default()
                        },
                        corral_core::UsageEntry {
                            estimated_usd: Some(amount),
                            ..Default::default()
                        },
                        corral_core::UsageEntry {
                            input: 7,
                            ..Default::default()
                        },
                    ]));
                }
                store.fixture_put_task(&full).unwrap();
                let full = store.get(&full.id).unwrap();
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

#[test]
fn cost_original_last_bit_difference_is_within_tolerance_on_both_backends() {
    for (backend, _dir, store) in backends() {
        let full = owner_changes(&store, 3, 3, 3);
        let headed = TaskCost::of_head(&store.head(&full.id).unwrap());
        let expected = TaskCost::of(&full);
        let (a, b) = (headed.estimated.unwrap(), expected.estimated.unwrap());
        assert_ne!(
            a.to_bits(),
            b.to_bits(),
            "{backend} must reproduce the original difference"
        );
        assert_costs_close(headed, expected);
    }
}

#[test]
fn cost_half_cent_boundary_grouping_is_within_tolerance_on_both_backends() {
    for (backend, _dir, store) in backends() {
        let mut full = store
            .create("Half cent", Some("app"), Status::Running, "Spec\n")
            .unwrap();
        grow(&mut full, 2, 0, 0, 0);
        for (run, amounts) in full.runs.iter_mut().zip([vec![0.01], vec![0.01, 0.025]]) {
            run.review_runs.clear();
            run.usage = Some(corral_core::Usage::from_entries(
                amounts
                    .into_iter()
                    .map(|amount| corral_core::UsageEntry {
                        estimated_usd: Some(amount),
                        ..Default::default()
                    })
                    .collect(),
            ));
        }
        store.fixture_put_task(&full).unwrap();
        let full = store.get(&full.id).unwrap();
        let headed = TaskCost::of_head(&store.head(&full.id).unwrap());
        let expected = TaskCost::of(&full);
        assert_ne!(
            headed.estimated.unwrap().to_bits(),
            expected.estimated.unwrap().to_bits(),
            "{backend}"
        );
        assert_costs_close(headed, expected);
    }
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

const PLAN: &str = "---\ngoal: Bounded regression\nnon_goals: []\ndecisions: []\naffected_paths: [candidate.txt]\ncontracts: []\ndependencies: []\nacceptance: [{scenario: bounded, expected: verified}]\nverification: [check]\nowner_decisions: []\n---\n";

#[derive(Clone, Copy, Debug, PartialEq)]
enum Approval {
    Owner,
    Automatic,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Break {
    None,
    NoIndependentReview,
    UnresolvedDecision,
    Unsubmitted,
    PolicyChanged,
}

fn approval_store(fake: bool) -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("app")).unwrap();
    let store = Store::init(dir.path()).unwrap();
    store
        .edit_config(|doc| {
            doc.set_str("projects.app.path", "app")?;
            doc.set_str("review.reviewer", "omo")
        })
        .unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    if fake {
        store = store.with_backend(Box::new(corral_core::backend::fake::FakeBackend::new(
            corral_core::backend::BackendConfig::default(),
            std::sync::Arc::new(corral_core::backend::ManualClock::new(at(0))),
            std::sync::Arc::new(corral_core::backend::FaultInjector::default()),
        )));
    }
    (dir, store)
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(bytes))
}

/// An approved plan on a task with `history` earlier runs, plan runs and plan reviews; the
/// planner is the first, historical plan run.
fn approved(
    fake: bool,
    approval: Approval,
    broken: Break,
    history: usize,
) -> (tempfile::TempDir, Store, String) {
    let (dir, store) = approval_store(fake);
    let mut task = store
        .create("Approval", Some("app"), Status::Running, "Spec\n")
        .unwrap();
    grow(&mut task, history + 1, 2, history + 1, 1);
    (task.created, task.updated) = (at(0), at(0));
    if approval == Approval::Automatic {
        task.runs
            .last_mut()
            .unwrap()
            .obligations
            .decisions
            .retain(|d| !d.pending());
    }
    let id = task.id.clone();
    let path = format!("artifacts/{id}/P0001/plan.md");
    std::fs::create_dir_all(store.dir().join(format!("artifacts/{id}/P0001"))).unwrap();
    std::fs::write(store.dir().join(&path), PLAN).unwrap();
    let source = dir.path().join("plan.md");
    std::fs::write(&source, PLAN).unwrap();
    let mut plan: corral_core::PlanRecord = serde_json::from_value(serde_json::json!({
        "id": "P1",
        "artifact": {"run": "P0001", "kind": "plan", "title": "Plan", "path": path,
                     "source": source, "sha256": sha256(PLAN.as_bytes()),
                     "bytes": PLAN.len(), "added": at(0)},
        "spec_revision": corral_core::plan_contract_revision(&task).unwrap(),
        "document": corral_core::PlanDocument::parse(PLAN).unwrap(),
        "run": "P0001",
    }))
    .unwrap();
    task.plans = vec![plan.clone()];
    task.plan = Some(format!("{id}@P1"));
    let digest = corral_core::plan_policy::policy_digest(&store.config, &task).unwrap();
    let review = |n: usize, reviewer: &str, notes: Option<&str>, digest: &str| {
        let mut r = plan_review(&format!("P1-PR{n:04}"), reviewer, Some("approve"));
        r["policy_digest"] = digest.into();
        r["notes"] = notes.into();
        serde_json::from_value::<corral_core::PlanReview>(r).unwrap()
    };
    plan.reviews = (0..history)
        .map(|n| review(n, "unnamed", None, "old"))
        .collect();
    let independent = if broken == Break::NoIndependentReview {
        "unnamed"
    } else {
        "omo"
    };
    plan.reviews.push(review(900, independent, None, &digest));
    let approval_by = match approval {
        Approval::Owner => {
            let notes = if broken == Break::UnresolvedDecision {
                "looks good"
            } else {
                "Resolved: Ship? => yes"
            };
            plan.reviews
                .push(review(901, "owner", Some(notes), &digest));
            "owner"
        }
        Approval::Automatic => "omo",
    };
    plan.approval = Some(corral_core::PlanApproval {
        by: approval_by.into(),
        at: at(0),
        policy_digest: digest.clone(),
        authorization: None,
    });
    task.plans = vec![plan];
    store.fixture_put_task(&task).unwrap();
    if approval == Approval::Automatic {
        let preview = store
            .preview_operating_policy(&corral_core::OperatingSelection {
                tasks: std::collections::BTreeSet::from([id.clone()]),
                authority: corral_core::OperatingAuthority {
                    plan_accept: true,
                    ..Default::default()
                },
                ..Default::default()
            })
            .unwrap();
        store
            .confirm_operating_policy(&preview, "owner", at(0))
            .unwrap();
        let grant = store.operating_state().unwrap().approval.unwrap();
        let authorization = store
            .authorize_operating_action(
                corral_core::OperatingActionIdentity {
                    task: id.clone(),
                    run: "P0001".into(),
                    attempt: "accept-P1-PR0900".into(),
                    kind: corral_core::OperatingActionKind::AcceptPlan,
                    candidate: None,
                    plan: Some("P1".into()),
                    round: None,
                },
                corral_core::OperatingOrigin::Automatic {
                    authorization: grant.authorization,
                    revision: grant.revision,
                },
            )
            .unwrap();
        store
            .update(&id, |t| {
                t.plans[0].approval.as_mut().unwrap().authorization =
                    Some(authorization.id.clone());
                Ok(())
            })
            .unwrap();
        if broken != Break::Unsubmitted {
            let outcome = store
                .submit_operating_action(
                    &authorization,
                    &authorization.action,
                    |_, _| Ok(()),
                    || Ok(()),
                )
                .unwrap();
            assert!(matches!(
                outcome,
                corral_core::SubmissionOutcome::Submitted(())
            ));
        }
    }
    if broken == Break::PolicyChanged {
        store
            .edit_config(|doc| doc.set_str("review.reviewer", "claude"))
            .unwrap();
        let mut store = store;
        store.config = Store::open(dir.path()).unwrap().config;
        return (dir, store, id);
    }
    (dir, store, id)
}

#[test]
fn approval_forms_agree_on_successful_and_refused_owner_and_automatic_approvals() {
    for fake in [false, true] {
        for (approval, broken, refusal) in [
            (Approval::Owner, Break::None, None),
            (Approval::Automatic, Break::None, None),
            (
                Approval::Owner,
                Break::NoIndependentReview,
                Some("independent review missing"),
            ),
            (
                Approval::Owner,
                Break::UnresolvedDecision,
                Some("explicit resolutions"),
            ),
            (
                Approval::Automatic,
                Break::Unsubmitted,
                Some("submitted acceptance action"),
            ),
            (
                Approval::Owner,
                Break::PolicyChanged,
                Some("reviewer policy changed"),
            ),
            (
                Approval::Automatic,
                Break::PolicyChanged,
                Some("reviewer policy changed"),
            ),
        ] {
            let (_dir, store, id) = approved(fake, approval, broken, 3);
            let full = store.get(&id).unwrap();
            let expected =
                validate_approval(&store, &store.config, &full, full.plans.last().unwrap())
                    .map_err(|e| format!("{e:#}"));
            let head = store.head(&id).unwrap();
            let headed =
                validate_approval_ref(&store, &store.config, &head).map_err(|e| format!("{e:#}"));
            assert_eq!(headed, expected, "fake={fake} {approval:?} {broken:?}");
            assert_eq!(
                validate_approval_ref(&store, &store.config, &full).map_err(|e| format!("{e:#}")),
                expected
            );
            match refusal {
                None => assert_eq!(expected, Ok(()), "fake={fake} {approval:?}"),
                Some(text) => {
                    let error = expected.unwrap_err();
                    assert!(
                        error.contains(text),
                        "fake={fake} {approval:?} {broken:?}: {error}"
                    );
                }
            }
        }
    }
}

#[test]
fn successful_approval_on_heads_costs_the_same_as_history_grows() {
    for fake in [false, true] {
        for approval in [Approval::Owner, Approval::Automatic] {
            let mut costs = Vec::new();
            for history in [1, 10] {
                let (_dir, store, id) = approved(fake, approval, Break::None, history);
                let head = store.head(&id).unwrap();
                let _hot = HotScope::enter();
                let before = work::snapshot();
                validate_approval_ref(&store, &store.config, &head).unwrap();
                let delta = work::snapshot().since(&before);
                delta.check_identity().unwrap();
                costs.push(delta);
            }
            for counter in [
                Counter::TaskHeadDecodes,
                Counter::HistoryRowDecodes,
                Counter::NestedRowDecodes,
                Counter::RecordDecodes,
                Counter::ViewsOpened,
            ] {
                assert_eq!(
                    costs[0].get(counter),
                    costs[1].get(counter),
                    "fake={fake} {approval:?} {counter:?}"
                );
            }
            assert!(
                costs[0].get(Counter::HistoryRowDecodes) + costs[0].get(Counter::NestedRowDecodes)
                    > 0
            );
            assert_eq!(costs[0].full_task_hydrations(), 0);
            assert_eq!(costs[1].full_task_hydrations(), 0);
        }
    }
}
