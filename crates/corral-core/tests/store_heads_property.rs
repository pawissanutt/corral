//! Random head and full-update sequences on the fake and SQLite backends (T-320 store-tasks-db):
//! after every step the stored summary, every container's facts and revision counters, the
//! pane references, the checkpoint metadata and payloads and the head-path revisions agree with
//! the assembled task, which equals an independently maintained expected task.

mod store_support;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use corral_core::backend::fake::FakeBackend;
use corral_core::backend::{BackendConfig, FaultInjector, ManualClock, TaskNum, TaskPart};
use corral_core::inbox::{ObserveCursor, Recipient, SessionBinding, SessionCursor};
use corral_core::{
    AggregateAttempt, AggregateReview, CheckpointMeta, CheckpointRecord, CommentAuthor,
    ContainerFacts, DecisionRecord, HistorySummary, PlanRecord, PlanReview, PlanRun, ProfileSwitch,
    Run, Status, Store, Task, TaskComment, Usage, UsageEntry, WaitRecord,
};
use store_support::*;

fn seq(n: usize) -> String {
    format!("{n:06}")
}

type Container = (
    TaskPart,
    String,
    ContainerFacts,
    Option<(TaskPart, String)>,
    Option<String>,
);

/// Each container row, the facts its assembled container derives, and the families whose
/// changes advance its `children_rev` and `obligations_rev`.
fn containers(task: &Task) -> Vec<Container> {
    let mut out = Vec::new();
    for (i, run) in task.runs.iter().enumerate() {
        let key = seq(i + 1);
        out.push((
            TaskPart::Run,
            key.clone(),
            ContainerFacts::of_run(&key, run).unwrap(),
            Some((TaskPart::ReviewRun, format!("{key}/"))),
            Some(format!("{key}/000000/")),
        ));
        for (c, child) in run.review_runs.iter().enumerate() {
            let child_key = format!("{key}/{}", seq(c + 1));
            out.push((
                TaskPart::ReviewRun,
                child_key.clone(),
                ContainerFacts::of_run(&child_key, child).unwrap(),
                None,
                Some(format!("{child_key}/")),
            ));
        }
    }
    for (i, run) in task.plan_runs.iter().enumerate() {
        let key = seq(i + 1);
        out.push((
            TaskPart::PlanRun,
            key.clone(),
            ContainerFacts::of_plan_run(&key, run),
            None,
            None,
        ));
    }
    for (i, plan) in task.plans.iter().enumerate() {
        let key = seq(i + 1);
        out.push((
            TaskPart::Plan,
            key.clone(),
            ContainerFacts::of_plan(&key, plan),
            Some((TaskPart::PlanReview, format!("{key}/"))),
            None,
        ));
    }
    for (i, attempt) in task.aggregate_attempts.iter().enumerate() {
        let key = seq(i + 1);
        out.push((
            TaskPart::AggregateAttempt,
            key.clone(),
            ContainerFacts::of_attempt(&key, attempt),
            Some((TaskPart::AggregateReview, format!("{key}/"))),
            None,
        ));
    }
    out
}

fn check_checkpoints(after: &Rows, owner: &str, run: &Run, step: &str) {
    for (i, record) in run.checkpoints.iter().enumerate() {
        let meta = CheckpointMeta::of((i + 1) as u64, record).unwrap();
        let row = &after[&(TaskPart::RunCheckpoint, format!("{owner}/{}", seq(i + 1)))];
        assert_eq!(
            row.doc,
            serde_json::to_value(&meta).unwrap(),
            "{step} meta {owner}/{i}"
        );
        let payload = &after[&(TaskPart::CheckpointPayload, meta.payload_digest.clone())];
        assert_eq!(
            payload.doc,
            serde_json::to_value(&record.checkpoint).unwrap(),
            "{step} payload {owner}/{i}"
        );
    }
}

fn check(store: &Store, expected: &mut Task, before: &Rows, step: &str) {
    let task = store.get("T-001").unwrap();
    expected.updated = task.updated;
    assert_eq!(&task, expected, "{step}");
    let raw = store.view().unwrap().head(TaskNum(1)).unwrap().unwrap();
    let summary: HistorySummary = serde_json::from_value(raw.summary).unwrap();
    assert_eq!(summary, HistorySummary::of(&task).unwrap(), "{step}");
    assert_eq!(
        raw.header.fields.spec_rev,
        corral_core::spec_revision(&task).unwrap(),
        "{step} spec revision"
    );
    assert_eq!(
        store.head("T-001").unwrap().operating_contract_revision(),
        corral_core::operating::operating_contract_revision(&task).unwrap(),
        "{step} operating contract revision"
    );
    let after = task_rows(store);
    for (part, key, derived, children, obligations) in containers(&task) {
        let row = &after[&(part, key.clone())];
        let stored: ContainerFacts = serde_json::from_value(row.doc["_facts"].clone()).unwrap();
        assert_eq!(
            stored.without_revisions(),
            derived.without_revisions(),
            "{step} {part:?} {key} facts"
        );
        let Some(old) = before.get(&(part, key.clone())) else {
            continue;
        };
        if let Some((child_part, prefix)) = children {
            let changed = changed_under(before, &after, child_part, &prefix);
            assert_eq!(
                revision(row, "children_rev"),
                revision(old, "children_rev") + u64::from(changed),
                "{step} {part:?} {key} children_rev"
            );
        }
        if let Some(prefix) = obligations {
            let changed = changed_under(before, &after, TaskPart::RunObligation, &prefix);
            assert_eq!(
                revision(row, "obligations_rev"),
                revision(old, "obligations_rev") + u64::from(changed),
                "{step} {part:?} {key} obligations_rev"
            );
        }
    }
    let panes: BTreeSet<String> = after
        .keys()
        .filter(|(part, _)| *part == TaskPart::PaneRef)
        .map(|(_, key)| key.clone())
        .collect();
    let derived: BTreeSet<String> = corral_core::task_panes(&task)
        .iter()
        .map(|p| p.key())
        .collect();
    assert_eq!(panes, derived, "{step} pane_refs");
    let mut expected_owners = BTreeMap::<String, u64>::new();
    for pane in corral_core::task_panes(&task) {
        *expected_owners.entry(pane.socket).or_default() += 1;
    }
    let owners = store
        .view()
        .unwrap()
        .aggregates(&[corral_core::backend::AggregateName::PaneOwners])
        .unwrap()
        .pane_owners
        .into_iter()
        .map(|owner| (owner.socket, owner.n))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(owners, expected_owners, "{step} ownership aggregate");
    for (i, run) in task.runs.iter().enumerate() {
        check_checkpoints(&after, &format!("{}/000000", seq(i + 1)), run, step);
        for (c, child) in run.review_runs.iter().enumerate() {
            check_checkpoints(
                &after,
                &format!("{}/{}", seq(i + 1), seq(c + 1)),
                child,
                step,
            );
        }
    }
    let referenced: BTreeSet<String> = after
        .iter()
        .filter(|((part, _), _)| *part == TaskPart::RunCheckpoint)
        .map(|(_, row)| row.doc["payload_digest"].as_str().unwrap().to_owned())
        .collect();
    let stored: BTreeSet<String> = after
        .keys()
        .filter(|(part, _)| *part == TaskPart::CheckpointPayload)
        .map(|(_, key)| key.clone())
        .collect();
    assert_eq!(
        stored, referenced,
        "{step} payloads are exactly the referenced ones"
    );
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

fn recipient(task: &Task) -> Recipient {
    Recipient::Run {
        task: "T-001".into(),
        run: current_id(task),
    }
}

fn cursor(session: &str, offset: u64) -> SessionCursor {
    SessionCursor {
        adapter: "pi".into(),
        binding: SessionBinding {
            session: session.into(),
            endpoint: None,
            transcript: None,
        },
        cursor: ObserveCursor {
            offset,
            ..ObserveCursor::default()
        },
    }
}

#[path = "store_heads_property/basic.rs"]
mod basic;
use basic::basic_step;

fn step(store: &Store, expected: &mut Task, rng: &mut Rng, n: usize) -> String {
    let choice = rng.below(33);
    if choice < 11 {
        return basic_step(store, expected, rng, n, choice);
    }
    let run = current_id(expected);
    let plan = expected.plans.last().unwrap().id.clone();
    let attempt = expected.aggregate_attempts.last().unwrap().id.clone();
    match choice {
        11 => {
            let id = format!("R{:04}", 100 + n);
            let new = coding_run("T-001", &id, 1000 + n as i64, 1, 1, false);
            store
                .update_head("T-001", |h| h.start_run(new.clone()))
                .unwrap();
            expected.runs.push(new);
            "start_run".into()
        }
        12 => {
            let new: PlanRun =
                serde_json::from_value(plan_run(&format!("P{:04}", 100 + n), false)).unwrap();
            store
                .update_head("T-001", |h| h.start_plan_run(new.clone()))
                .unwrap();
            expected.plan_runs.push(new);
            "start_plan_run".into()
        }
        13 => {
            let new: PlanRecord = serde_json::from_value(store_support::plan(
                "T-001",
                &format!("P{:04}", 100 + n),
                2,
            ))
            .unwrap();
            store
                .update_head("T-001", |h| h.push_plan(new.clone()))
                .unwrap();
            expected.plans.push(new);
            "push_plan".into()
        }
        14 => {
            let new: AggregateAttempt = serde_json::from_value(store_support::attempt(
                "T-001",
                &format!("A{:04}", 100 + n),
                2,
            ))
            .unwrap();
            store
                .update_head("T-001", |h| h.push_aggregate_attempt(new.clone()))
                .unwrap();
            expected.aggregate_attempts.push(new);
            "push_aggregate_attempt".into()
        }
        15 => {
            let review: PlanReview = serde_json::from_value(plan_review(
                &format!("{plan}-PR{:04}", 100 + n),
                "rev",
                None,
            ))
            .unwrap();
            store
                .update_head("T-001", |h| h.push_plan_review(&plan, review.clone()))
                .unwrap();
            expected.plans.last_mut().unwrap().reviews.push(review);
            "push_plan_review".into()
        }
        16 => {
            let reviews = &expected.plans.last().unwrap().reviews;
            if reviews.is_empty() {
                return "no plan review to edit".into();
            }
            let k = rng.below(reviews.len());
            let id = reviews[k].id.clone();
            let digest = format!("policy {n}");
            store
                .update_head("T-001", |h| {
                    h.edit_plan_review(&plan, &id, |r| {
                        r.policy_digest = digest.clone();
                        Ok(())
                    })
                })
                .unwrap();
            expected.plans.last_mut().unwrap().reviews[k].policy_digest = digest;
            "edit_plan_review".into()
        }
        17 => {
            if expected.plans.last().unwrap().reviews.is_empty() {
                return "no plan review to pop".into();
            }
            store
                .update_head("T-001", |h| h.pop_current_plan_review(&plan))
                .unwrap();
            expected.plans.last_mut().unwrap().reviews.pop();
            "pop_current_plan_review".into()
        }
        18 => {
            let reason = format!("terminal {n}");
            store
                .update_head("T-001", |h| {
                    h.terminalize_pending_plan_reviews(&plan, &reason)
                })
                .unwrap();
            let stored = store.get("T-001").unwrap();
            let stored = &stored.plans.last().unwrap().reviews;
            for (k, review) in expected
                .plans
                .last_mut()
                .unwrap()
                .reviews
                .iter_mut()
                .enumerate()
            {
                if review.verdict.is_none() {
                    review.verdict = Some(corral_core::Verdict::Escalate);
                    review.notes = Some(reason.clone());
                    review.pending_reply = None;
                    review.at = stored[k].at;
                }
            }
            "terminalize_pending_plan_reviews".into()
        }
        19 => {
            let review: AggregateReview = serde_json::from_value(aggregate_review(
                &format!("{attempt}-R{:04}", 100 + n),
                true,
            ))
            .unwrap();
            store
                .update_head("T-001", |h| {
                    h.push_aggregate_review(&attempt, review.clone())
                })
                .unwrap();
            expected
                .aggregate_attempts
                .last_mut()
                .unwrap()
                .reviews
                .push(review);
            "push_aggregate_review".into()
        }
        20 => {
            let reviews = &expected.aggregate_attempts.last().unwrap().reviews;
            if reviews.is_empty() {
                return "no aggregate review to edit".into();
            }
            let k = rng.below(reviews.len());
            let id = reviews[k].id.clone();
            let digest = ["e", "B"][rng.below(2)].to_owned();
            store
                .update_head("T-001", |h| {
                    h.edit_aggregate_review(&attempt, &id, |r| {
                        r.evidence_digest = digest.clone();
                        Ok(())
                    })
                })
                .unwrap();
            expected.aggregate_attempts.last_mut().unwrap().reviews[k].evidence_digest = digest;
            "edit_aggregate_review".into()
        }
        21 => {
            let by = format!("{attempt}-R{:04}", 500 + n);
            store
                .update_head("T-001", |h| {
                    h.supersede_pending_aggregate_reviews(&attempt, &by)
                })
                .unwrap();
            for review in &mut expected.aggregate_attempts.last_mut().unwrap().reviews {
                if review.pending() {
                    review.abandoned = Some(format!("superseded by {by}"));
                }
            }
            "supersede_pending_aggregate_reviews".into()
        }
        22 => {
            let k = expected.runs.last().unwrap().checkpoints.len();
            let record: CheckpointRecord =
                serde_json::from_value(checkpoint("T-001", &run, 50 + k + n, 1)).unwrap();
            store
                .update_head("T-001", |h| h.push_checkpoint(&run, record.clone()))
                .unwrap();
            expected.runs.last_mut().unwrap().checkpoints.push(record);
            "push_checkpoint".into()
        }
        23 => {
            let records: Vec<CheckpointRecord> = expected.runs[0]
                .checkpoints
                .iter()
                .take(2)
                .cloned()
                .collect();
            store
                .update_head("T-001", |h| h.restore_checkpoints(&run, records.clone()))
                .unwrap();
            expected
                .runs
                .last_mut()
                .unwrap()
                .checkpoints
                .extend(records);
            "restore_checkpoints (shared payloads)".into()
        }
        24 => {
            let decision: DecisionRecord =
                serde_json::from_value(decision(&run, 7000 + n, false)).unwrap();
            store
                .update_head("T-001", |h| h.push_decision(&run, decision.clone()))
                .unwrap();
            expected
                .runs
                .last_mut()
                .unwrap()
                .obligations
                .decisions
                .push(decision);
            "push_decision".into()
        }
        25 => {
            let decisions = &expected.runs.last().unwrap().obligations.decisions;
            if decisions.is_empty() {
                return "no decision to edit".into();
            }
            let k = rng.below(decisions.len());
            let id = decisions[k].request.id.clone();
            let detail = ["asked", "B"][rng.below(2)].to_owned();
            store
                .update_head("T-001", |h| {
                    h.edit_decision(&run, &id, |d| {
                        d.observation.detail = detail.clone();
                        Ok(())
                    })
                })
                .unwrap();
            expected.runs.last_mut().unwrap().obligations.decisions[k]
                .observation
                .detail = detail;
            "edit_decision".into()
        }
        26 => {
            let waits = &expected.runs.last().unwrap().obligations.waits;
            if waits.is_empty() {
                return "no wait to edit".into();
            }
            let k = rng.below(waits.len());
            let id = waits[k].request.id.clone();
            let detail = ["observed", "B"][rng.below(2)].to_owned();
            store
                .update_head("T-001", |h| {
                    h.edit_wait(&run, &id, |w| {
                        w.observation.detail = detail.clone();
                        Ok(())
                    })
                })
                .unwrap();
            expected.runs.last_mut().unwrap().obligations.waits[k]
                .observation
                .detail = detail;
            "edit_wait".into()
        }
        27 => {
            let effect = (rng.below(3) + 1) as u64 * 1000;
            let to = recipient(expected);
            store
                .update_head("T-001", |h| h.mark_effect_applied(&to, effect))
                .unwrap();
            expected
                .runs
                .last_mut()
                .unwrap()
                .inbox
                .effects_applied
                .insert(effect);
            "mark_effect_applied".into()
        }
        28 => {
            let new = cursor(["s-0", "fresh"][rng.below(2)], n as u64);
            let to = recipient(expected);
            store
                .update_head("T-001", |h| h.set_observe_cursor(&to, new.clone()))
                .unwrap();
            let cursors = &mut expected.runs.last_mut().unwrap().inbox.observe_cursors;
            match cursors
                .iter_mut()
                .find(|c| c.adapter == new.adapter && c.binding == new.binding)
            {
                Some(existing) => *existing = new,
                None => cursors.push(new),
            }
            "set_observe_cursor".into()
        }
        30 => {
            let id = format!("R{:04}", 300 + n);
            let new = coding_run("T-001", &id, 3000 + n as i64, 1, 1, false);
            store
                .update_head("T-001", |h| {
                    h.append_usage_entries(&run, None, vec![entry(n)])?;
                    h.start_run(new.clone())
                })
                .unwrap();
            let current = expected.runs.last_mut().unwrap();
            let mut all = current.usage.take().map(|u| u.entries).unwrap_or_default();
            all.push(entry(n));
            current.usage = Some(Usage::from_entries(all));
            expected.runs.push(new);
            "usage then start_run in one write".into()
        }
        31 => {
            let waits = &expected.runs.last().unwrap().obligations.waits;
            if waits.is_empty() {
                return "no wait for A-B-A".into();
            }
            let id = waits[0].request.id.clone();
            let original = waits[0].observation.detail.clone();
            let seq = store.view().unwrap().seq();
            store
                .update_head("T-001", |h| {
                    h.edit_wait(&run, &id, |w| {
                        w.observation.detail = format!("{original} B");
                        Ok(())
                    })?;
                    h.edit_wait(&run, &id, |w| {
                        w.observation.detail = original.clone();
                        Ok(())
                    })
                })
                .unwrap();
            assert_eq!(
                store.view().unwrap().seq(),
                seq,
                "an A-B-A in one write writes nothing"
            );
            "wait A-B-A in one write".into()
        }
        32 => {
            let k = expected.runs.last().unwrap().review_runs.len() + 1;
            let child: Run =
                serde_json::from_value(review_child("T-001", &run, k + 200, 1, true, "approve"))
                    .unwrap();
            let summary = Some(format!("pushed {n}"));
            store
                .update_head("T-001", |h| {
                    h.push_review_child(&run, child.clone())?;
                    h.edit_current_review_child(&run, |c| {
                        c.summary = summary.clone();
                        Ok(())
                    })
                })
                .unwrap();
            let mut child = child;
            child.summary = summary;
            expected.runs.last_mut().unwrap().review_runs.push(child);
            "push and edit child in one write".into()
        }
        _ => {
            let usage = Usage::from_entries(vec![entry(n)]);
            let child = expected
                .runs
                .last()
                .unwrap()
                .review_runs
                .last()
                .map(|c| c.id.clone())
                .filter(|_| rng.below(2) == 0);
            store
                .update_head("T-001", |h| {
                    h.set_usage(&run, child.as_deref(), usage.clone())
                })
                .unwrap();
            let current = expected.runs.last_mut().unwrap();
            match child {
                Some(id) => {
                    current
                        .review_runs
                        .iter_mut()
                        .find(|c| c.id == id)
                        .unwrap()
                        .usage = Some(usage)
                }
                None => current.usage = Some(usage),
            }
            "set_usage".into()
        }
    }
}

fn run_sequence(store: &Store, seed: u64) -> BTreeSet<String> {
    let mut expected = store
        .create("Property", Some("app"), Status::Running, "Spec\n")
        .unwrap();
    grow(&mut expected, 3, 2, 2, 2);
    store.fixture_put_task(&expected).unwrap();
    let mut rng = Rng(seed);
    let mut before = task_rows(store);
    check(store, &mut expected, &before, "seed");
    let mut labels = BTreeSet::new();
    for n in 0..60 {
        let label = step(store, &mut expected, &mut rng, n);
        labels.insert(label.clone());
        check(
            store,
            &mut expected,
            &before,
            &format!("seed {seed} step {n}: {label}"),
        );
        before = task_rows(store);
    }
    labels
}

#[test]
fn random_head_and_full_updates_keep_every_derived_record_exact() {
    let mut ran = BTreeSet::new();
    for seed in [
        0x9e37_79b9_7f4a_7c15,
        0x2545_f491_4f6c_dd1d,
        0x1234_5678_9abc_def1,
        0x0bad_c0de_dead_beef,
    ] {
        let (_sqlite_dir, sqlite) = store();
        ran.extend(run_sequence(&sqlite, seed));

        let (_fake_dir, base) = store();
        let backend = FakeBackend::new(
            BackendConfig::default(),
            Arc::new(ManualClock::new(jiff::Timestamp::now())),
            Arc::new(FaultInjector::default()),
        );
        let fake = base.with_backend(Box::new(backend));
        ran.extend(run_sequence(&fake, seed));
    }
    for op in [
        "head title",
        "add_comment",
        "push_review_child",
        "push_wait",
        "push_switch",
        "append_usage_entries",
        "edit_review_child",
        "full old-run edit",
        "full old-child edit",
        "refused id edit",
        "start_run",
        "start_plan_run",
        "push_plan",
        "push_aggregate_attempt",
        "push_plan_review",
        "edit_plan_review",
        "pop_current_plan_review",
        "terminalize_pending_plan_reviews",
        "push_aggregate_review",
        "edit_aggregate_review",
        "supersede_pending_aggregate_reviews",
        "push_checkpoint",
        "restore_checkpoints (shared payloads)",
        "push_decision",
        "edit_decision",
        "edit_wait",
        "mark_effect_applied",
        "set_observe_cursor",
        "set_usage",
        "usage then start_run in one write",
        "wait A-B-A in one write",
        "push and edit child in one write",
    ] {
        assert!(ran.contains(op), "{op} never ran: {ran:?}");
    }
}
