//! Random sequences of `TaskMut` mutations applied to a stored head and to an independently
//! kept full task, plus full updates, on both backends (T-331 action-heads): after every step
//! the stored task equals the kept one and the summary, findings, facts and pane rows equal
//! what the full task derives; a keyed edit writes only its row, and an identity edit nothing.

mod store_support;
mod task_mut_refusals;
#[path = "task_ref_support/mod.rs"]
mod task_ref_support;

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use corral_core::backend::{TaskNum, TaskPart};
use corral_core::inbox::{ObserveCursor, Recipient, SessionBinding, SessionCursor};
use corral_core::{
    AggregateReview, CheckpointRecord, ContainerFacts, DecisionRecord, HistorySummary, PlanReview,
    ProfileSwitch, Run, Status, Store, Task, TaskMut, Usage, UsageEntry, Verdict, WaitRecord,
};
use store_support::*;
use task_mut_refusals::{Refusal, refusal, refusal_name, refuse};
use task_ref_support::backends;

#[derive(Debug, Clone)]
enum Step {
    EditRun {
        id: String,
        failures: u32,
        summary: String,
        herdr: bool,
    },
    EditRunIdentity {
        id: String,
    },
    EditPlanRun {
        id: String,
        rounds: u32,
    },
    EditPlan {
        id: String,
        spec: String,
    },
    EditAttempt {
        id: String,
        code: bool,
    },
    Inbox {
        run: String,
    },
    PushChild {
        run: String,
        child: Run,
    },
    EditChild {
        run: String,
        id: String,
        notes: Option<String>,
        verdict: Verdict,
    },
    PushPlanReview {
        plan: String,
        review: PlanReview,
    },
    EditPlanReview {
        plan: String,
        id: String,
        verdict: Option<Verdict>,
    },
    PopPlanReview {
        plan: String,
    },
    StartRun {
        run: Run,
    },
    Title {
        title: String,
    },
    PushAggregateReview {
        attempt: String,
        review: AggregateReview,
    },
    EditAggregateReview {
        attempt: String,
        id: String,
        digest: String,
    },
    SupersedeAggregate {
        attempt: String,
        by: String,
    },
    TerminalizePlan {
        plan: String,
        reason: String,
    },
    PushCheckpoint {
        run: String,
        record: CheckpointRecord,
    },
    RestoreCheckpoints {
        run: String,
        records: Vec<CheckpointRecord>,
    },
    PushSwitch {
        run: String,
        switch: ProfileSwitch,
    },
    PushWait {
        run: String,
        wait: WaitRecord,
    },
    PushDecision {
        run: String,
        decision: DecisionRecord,
    },
    EditWait {
        run: String,
        id: String,
        detail: String,
    },
    EditDecision {
        run: String,
        id: String,
        detail: String,
    },
    MarkEffect {
        run: String,
        effect: u64,
    },
    SetCursor {
        run: String,
        session: String,
        offset: u64,
    },
    SetUsage {
        run: String,
        child: Option<String>,
        usage: Usage,
    },
    AppendUsage {
        run: String,
        child: Option<String>,
        entries: Vec<UsageEntry>,
    },
    Refused(Refusal),
}

fn mutate<M: TaskMut>(task: &mut M, step: &Step) -> Result<()> {
    match step.clone() {
        Step::EditRun {
            id,
            failures,
            summary,
            herdr,
        } => task.edit_run(&id, |run| {
            run.failures = failures;
            run.summary = Some(summary);
            if herdr && let Some(pane) = run.herdr.as_mut() {
                pane.pane_id = format!("{}-moved", pane.pane_id);
            }
            Ok(())
        }),
        Step::EditRunIdentity { id } => task.edit_run(&id, |run| {
            run.started = at(-5);
            Ok(())
        }),
        Step::EditPlanRun { id, rounds } => task.edit_plan_run(&id, |run| {
            run.rounds = rounds;
            Ok(())
        }),
        Step::EditPlan { id, spec } => task.edit_plan(&id, |plan| {
            plan.spec_revision = spec;
            Ok(())
        }),
        Step::EditAttempt { id, code } => task.edit_aggregate_attempt(&id, |attempt| {
            attempt.code = code;
            Ok(())
        }),
        Step::Inbox { run } => {
            let to = Recipient::Run {
                task: "T-001".into(),
                run,
            };
            task.inbox_state_mut(&to, |inbox| {
                inbox.next_seq += 1;
                Ok(())
            })
        }
        Step::PushChild { run, child } => task.push_review_child(&run, child),
        Step::EditChild {
            run,
            id,
            notes,
            verdict,
        } => task.edit_review_child(&run, &id, |c| {
            if let Some(review) = c.review.as_mut() {
                review.notes = notes;
                review.verdict = Some(verdict);
            }
            Ok(())
        }),
        Step::PushPlanReview { plan, review } => task.push_plan_review(&plan, review),
        Step::EditPlanReview { plan, id, verdict } => task.edit_plan_review(&plan, &id, |r| {
            r.verdict = verdict;
            r.notes = verdict.map(|v| format!("{v:?} notes"));
            Ok(())
        }),
        Step::PopPlanReview { plan } => task.pop_current_plan_review(&plan),
        Step::StartRun { run } => task.start_run(run),
        Step::Title { title } => task.edit_head_fields(|fields| {
            fields.title = title;
            Ok(())
        }),
        Step::PushAggregateReview { attempt, review } => {
            task.push_aggregate_review(&attempt, review)
        }
        Step::EditAggregateReview {
            attempt,
            id,
            digest,
        } => task.edit_aggregate_review(&attempt, &id, |r| {
            r.evidence_digest = digest;
            Ok(())
        }),
        Step::SupersedeAggregate { attempt, by } => {
            task.supersede_pending_aggregate_reviews(&attempt, &by)
        }
        Step::TerminalizePlan { plan, reason } => {
            task.terminalize_pending_plan_reviews(&plan, &reason)
        }
        Step::PushCheckpoint { run, record } => task.push_checkpoint(&run, record),
        Step::RestoreCheckpoints { run, records } => task.restore_checkpoints(&run, records),
        Step::PushSwitch { run, switch } => task.push_switch(&run, switch),
        Step::PushWait { run, wait } => task.push_wait(&run, wait),
        Step::PushDecision { run, decision } => task.push_decision(&run, decision),
        Step::EditWait { run, id, detail } => task.edit_wait(&run, &id, |w| {
            w.observation.detail = detail;
            Ok(())
        }),
        Step::EditDecision { run, id, detail } => task.edit_decision(&run, &id, |d| {
            d.observation.detail = detail;
            Ok(())
        }),
        Step::MarkEffect { run, effect } => task.mark_effect_applied(&recipient(&run), effect),
        Step::SetCursor {
            run,
            session,
            offset,
        } => task.set_observe_cursor(
            &recipient(&run),
            SessionCursor {
                adapter: "pi".into(),
                binding: SessionBinding {
                    session,
                    endpoint: None,
                    transcript: None,
                },
                cursor: ObserveCursor {
                    offset,
                    ..ObserveCursor::default()
                },
            },
        ),
        Step::SetUsage { run, child, usage } => task.set_usage(&run, child.as_deref(), usage),
        Step::AppendUsage {
            run,
            child,
            entries,
        } => task.append_usage_entries(&run, child.as_deref(), entries),
        Step::Refused(refusal) => refuse(task, refusal),
    }
}

fn recipient(run: &str) -> Recipient {
    Recipient::Run {
        task: "T-001".into(),
        run: run.into(),
    }
}

fn pick<'a, T>(rng: &mut Rng, items: &'a [T]) -> &'a T {
    &items[rng.below(items.len())]
}

fn step(task: &Task, rng: &mut Rng, n: usize) -> Step {
    let runs: Vec<String> = task.runs.iter().map(|r| r.id.clone()).collect();
    let current = runs.last().unwrap().clone();
    let plan = task.plans.last().unwrap().id.clone();
    let attempt = task.aggregate_attempts.last().unwrap().id.clone();
    let verdicts = [Verdict::Changes, Verdict::Approve, Verdict::Escalate];
    if rng.below(5) == 0 {
        return Step::Refused(refusal(task, rng));
    }
    match rng.below(26) {
        0 => Step::EditRun {
            id: pick(rng, &runs).clone(),
            failures: rng.below(3) as u32,
            summary: format!("edited {n}"),
            herdr: rng.below(2) == 0,
        },
        1 => Step::EditRunIdentity {
            id: pick(rng, &runs).clone(),
        },
        2 => Step::EditPlanRun {
            id: pick(rng, &task.plan_runs).id.clone(),
            rounds: n as u32,
        },
        3 => Step::EditPlan {
            id: pick(rng, &task.plans).id.clone(),
            spec: format!("spec {n}"),
        },
        4 => Step::EditAttempt {
            id: pick(rng, &task.aggregate_attempts).id.clone(),
            code: rng.below(2) == 0,
        },
        5 => Step::Inbox { run: current },
        6 => {
            let mut child: Run = serde_json::from_value(review_child(
                "T-001",
                &current,
                500 + n,
                1,
                true,
                "changes",
            ))
            .unwrap();
            if let Some(review) = child.review.as_mut() {
                review.notes = Some(format!("pushed {n}"));
            }
            Step::PushChild {
                run: current,
                child,
            }
        }
        7 => {
            let children = &task.runs.last().unwrap().review_runs;
            if children.is_empty() {
                return Step::Title {
                    title: format!("t {n}"),
                };
            }
            Step::EditChild {
                run: current,
                id: pick(rng, children).id.clone(),
                notes: (rng.below(3) != 0).then(|| format!("notes {n}")),
                verdict: *pick(rng, &verdicts),
            }
        }
        8 => Step::PushPlanReview {
            plan: plan.clone(),
            review: serde_json::from_value(plan_review(
                &format!("{plan}-PR{:04}", 600 + n),
                "rev",
                Some("changes"),
            ))
            .unwrap(),
        },
        9 => {
            let reviews = &task.plans.last().unwrap().reviews;
            if reviews.is_empty() {
                return Step::Title {
                    title: format!("t {n}"),
                };
            }
            Step::EditPlanReview {
                plan,
                id: pick(rng, reviews).id.clone(),
                verdict: [None, Some(Verdict::Changes), Some(Verdict::Approve)][rng.below(3)],
            }
        }
        10 if !task.plans.last().unwrap().reviews.is_empty() => Step::PopPlanReview { plan },
        11 => Step::StartRun {
            run: coding_run(
                "T-001",
                &format!("R{:04}", 700 + n),
                700 + n as i64,
                2,
                1,
                false,
            ),
        },
        13 => Step::PushAggregateReview {
            attempt: attempt.clone(),
            review: serde_json::from_value(aggregate_review(
                &format!("{attempt}-R{:04}", 600 + n),
                rng.below(2) == 0,
            ))
            .unwrap(),
        },
        14 => Step::EditAggregateReview {
            id: pick(rng, &task.aggregate_attempts.last().unwrap().reviews)
                .id
                .clone(),
            attempt,
            digest: format!("digest {n}"),
        },
        15 => Step::SupersedeAggregate {
            by: format!("{attempt}-R{:04}", 700 + n),
            attempt,
        },
        16 => Step::TerminalizePlan {
            plan,
            reason: format!("terminal {n}"),
        },
        17 => Step::PushCheckpoint {
            record: serde_json::from_value(checkpoint("T-001", &current, 300 + n, 2)).unwrap(),
            run: current,
        },
        18 => Step::RestoreCheckpoints {
            records: task.runs[0].checkpoints.iter().take(2).cloned().collect(),
            run: current,
        },
        19 => Step::PushSwitch {
            run: current,
            switch: serde_json::from_value(switch(400 + n)).unwrap(),
        },
        20 => Step::PushWait {
            wait: serde_json::from_value(wait(&current, 5000 + n, rng.below(2) == 0)).unwrap(),
            run: current,
        },
        21 => Step::PushDecision {
            decision: serde_json::from_value(decision(&current, 6000 + n, rng.below(2) == 0))
                .unwrap(),
            run: current,
        },
        22 => {
            let obligations = &task.runs.last().unwrap().obligations;
            if rng.below(2) == 0 {
                Step::EditWait {
                    id: pick(rng, &obligations.waits).request.id.clone(),
                    run: current,
                    detail: format!("wait {n}"),
                }
            } else {
                Step::EditDecision {
                    id: pick(rng, &obligations.decisions).request.id.clone(),
                    run: current,
                    detail: format!("decision {n}"),
                }
            }
        }
        23 => Step::MarkEffect {
            run: current,
            effect: (rng.below(4) + 1) as u64 * 100,
        },
        24 => Step::SetCursor {
            run: current,
            session: ["s-0", "fresh"][rng.below(2)].into(),
            offset: n as u64,
        },
        25 => {
            let child = task
                .runs
                .last()
                .unwrap()
                .review_runs
                .last()
                .map(|c| c.id.clone())
                .filter(|_| rng.below(2) == 0);
            let entry: UsageEntry = serde_json::from_value(usage_entry(n, true)).unwrap();
            if rng.below(2) == 0 {
                Step::SetUsage {
                    run: current,
                    child,
                    usage: Usage::from_entries(vec![entry]),
                }
            } else {
                Step::AppendUsage {
                    run: current,
                    child,
                    entries: vec![entry],
                }
            }
        }
        _ => Step::Title {
            title: format!("title {n}"),
        },
    }
}

fn seq(n: usize) -> String {
    format!("{n:06}")
}

fn facts(task: &Task) -> BTreeMap<(TaskPart, String), ContainerFacts> {
    let mut out = BTreeMap::new();
    for (i, run) in task.runs.iter().enumerate() {
        let key = seq(i + 1);
        out.insert(
            (TaskPart::Run, key.clone()),
            ContainerFacts::of_run(&format!("{key}/000000"), run).unwrap(),
        );
        for (c, child) in run.review_runs.iter().enumerate() {
            let child_key = format!("{key}/{}", seq(c + 1));
            out.insert(
                (TaskPart::ReviewRun, child_key.clone()),
                ContainerFacts::of_run(&child_key, child).unwrap(),
            );
        }
    }
    for (i, run) in task.plan_runs.iter().enumerate() {
        let key = seq(i + 1);
        out.insert(
            (TaskPart::PlanRun, key.clone()),
            ContainerFacts::of_plan_run(&key, run),
        );
    }
    for (i, plan) in task.plans.iter().enumerate() {
        let key = seq(i + 1);
        out.insert(
            (TaskPart::Plan, key.clone()),
            ContainerFacts::of_plan(&key, plan),
        );
    }
    for (i, attempt) in task.aggregate_attempts.iter().enumerate() {
        let key = seq(i + 1);
        out.insert(
            (TaskPart::AggregateAttempt, key.clone()),
            ContainerFacts::of_attempt(&key, attempt),
        );
    }
    out.into_iter()
        .map(|(key, facts)| (key, facts.without_revisions()))
        .collect()
}

fn check(store: &Store, expected: &mut Task, step: &str) -> Rows {
    let task = store.get("T-001").unwrap();
    expected.updated = task.updated;
    for (kept, stored) in expected.plans.iter_mut().zip(&task.plans) {
        for (k, s) in kept.reviews.iter_mut().zip(&stored.reviews) {
            k.at = s.at;
        }
    }
    assert_eq!(&task, expected, "{step}");
    let raw = store.view().unwrap().head(TaskNum(1)).unwrap().unwrap();
    let summary: HistorySummary = serde_json::from_value(raw.summary).unwrap();
    assert_eq!(
        summary,
        HistorySummary::of(&task).unwrap(),
        "{step} summary"
    );
    let rows = task_rows(store);
    let findings: BTreeMap<String, serde_json::Value> = rows
        .iter()
        .filter(|((part, _), _)| *part == TaskPart::Finding)
        .map(|((_, key), row)| (key.clone(), row.doc.clone()))
        .collect();
    let derived: BTreeMap<String, serde_json::Value> = corral_core::task_findings(&task)
        .into_iter()
        .map(|(key, f)| (key, serde_json::to_value(f).unwrap()))
        .collect();
    assert_eq!(findings, derived, "{step} findings");
    for (key, derived) in facts(&task) {
        let stored: ContainerFacts =
            serde_json::from_value(rows[&key].doc["_facts"].clone()).unwrap();
        assert_eq!(stored.without_revisions(), derived, "{step} {key:?} facts");
    }
    let panes: BTreeSet<String> = rows
        .keys()
        .filter(|(part, _)| *part == TaskPart::PaneRef)
        .map(|(_, key)| key.clone())
        .collect();
    let derived: BTreeSet<String> = corral_core::task_panes(&task)
        .iter()
        .map(|p| p.key())
        .collect();
    assert_eq!(panes, derived, "{step} pane_refs");
    rows
}

fn changed(before: &Rows, after: &Rows) -> BTreeSet<(TaskPart, String)> {
    before
        .keys()
        .chain(after.keys())
        .filter(|key| before.get(*key) != after.get(*key))
        .cloned()
        .collect()
}

fn run_sequence(store: &Store, seed: u64) -> BTreeSet<&'static str> {
    let mut expected = store
        .create("Sequence", Some("app"), Status::Running, "Spec\n")
        .unwrap();
    grow(&mut expected, 3, 3, 2, 1);
    store.fixture_put_task(&expected).unwrap();
    let mut expected = store.get(&expected.id).unwrap();
    let mut rng = Rng(seed);
    let mut before = check(store, &mut expected, "seed");
    let mut ran = BTreeSet::new();
    for n in 0..120 {
        if rng.below(6) == 0 {
            let run = rng.below(expected.runs.len());
            let notes = format!("full {n}");
            let edit = |t: &mut Task| {
                if let Some(review) = t.runs[run]
                    .review_runs
                    .first_mut()
                    .and_then(|c| c.review.as_mut())
                {
                    review.notes = Some(notes.clone());
                    review.verdict = Some(Verdict::Changes);
                }
            };
            store
                .update("T-001", |t| {
                    edit(t);
                    Ok(())
                })
                .unwrap();
            edit(&mut expected);
            ran.insert("full history child edit");
            before = check(store, &mut expected, &format!("{seed} {n} full"));
            continue;
        }
        let step = step(&expected, &mut rng, n);
        let label = format!("{seed} {n} {step:?}");
        let mut kept = expected.clone();
        let full = mutate(&mut kept, &step);
        let feed = store.view().unwrap().seq();
        let head = store.update_head("T-001", |h| mutate(h, &step));
        assert_eq!(full.is_ok(), head.is_ok(), "{label}: {full:?} / {head:?}");
        if full.is_err() {
            assert!(
                matches!(step, Step::EditRunIdentity { .. } | Step::Refused(_)),
                "{label}: unexpected refusal {full:?}"
            );
            assert_eq!(
                store.view().unwrap().seq(),
                feed,
                "{label}: a refusal wrote"
            );
            ran.insert(match &step {
                Step::Refused(r) => refusal_name(r),
                _ => "refused identity edit",
            });
            continue;
        }
        expected = kept;
        let after = check(store, &mut expected, &label);
        if let Step::EditRun {
            id, herdr: false, ..
        } = &step
        {
            let i = expected.runs.iter().position(|r| r.id == *id).unwrap();
            let written = changed(&before, &after);
            if i + 1 < expected.runs.len() {
                let own = BTreeSet::from([(TaskPart::Run, seq(i + 1))]);
                assert!(written.is_subset(&own), "{label}: wrote {written:?}");
                ran.insert("keyed history run edit");
            }
        }
        ran.insert(match step {
            Step::EditRun { .. } => "edit_run",
            Step::EditRunIdentity { .. } => "identity",
            Step::EditPlanRun { .. } => "edit_plan_run",
            Step::EditPlan { .. } => "edit_plan",
            Step::EditAttempt { .. } => "edit_aggregate_attempt",
            Step::Inbox { .. } => "inbox_state_mut",
            Step::PushChild { .. } => "push_review_child",
            Step::EditChild { .. } => "edit_review_child",
            Step::PushPlanReview { .. } => "push_plan_review",
            Step::EditPlanReview { .. } => "edit_plan_review",
            Step::PopPlanReview { .. } => "pop_current_plan_review",
            Step::StartRun { .. } => "start_run",
            Step::Title { .. } => "edit_head_fields",
            Step::PushAggregateReview { .. } => "push_aggregate_review",
            Step::EditAggregateReview { .. } => "edit_aggregate_review",
            Step::SupersedeAggregate { .. } => "supersede_pending_aggregate_reviews",
            Step::TerminalizePlan { .. } => "terminalize_pending_plan_reviews",
            Step::PushCheckpoint { .. } => "push_checkpoint",
            Step::RestoreCheckpoints { .. } => "restore_checkpoints",
            Step::PushSwitch { .. } => "push_switch",
            Step::PushWait { .. } => "push_wait",
            Step::PushDecision { .. } => "push_decision",
            Step::EditWait { .. } => "edit_wait",
            Step::EditDecision { .. } => "edit_decision",
            Step::MarkEffect { .. } => "mark_effect_applied",
            Step::SetCursor { .. } => "set_observe_cursor",
            Step::SetUsage { .. } => "set_usage",
            Step::AppendUsage { .. } => "append_usage_entries",
            Step::Refused(_) => "refusal accepted",
        });
        before = after;
    }
    ran
}

#[test]
fn random_task_mut_sequences_keep_summary_findings_facts_and_panes_exact() {
    let mut ran = BTreeSet::new();
    for seed in [
        0x9e37_79b9_7f4a_7c15,
        0x2545_f491_4f6c_dd1d,
        0x1234_5678_9abc_def1,
        0x0bad_c0de_dead_beef,
        0x5151_5151_0f0f_0f0f,
    ] {
        for (_backend, _dir, store) in backends() {
            ran.extend(run_sequence(&store, seed));
        }
    }
    for op in [
        "edit_run",
        "keyed history run edit",
        "refused identity edit",
        "edit_plan_run",
        "edit_plan",
        "edit_aggregate_attempt",
        "inbox_state_mut",
        "push_review_child",
        "edit_review_child",
        "push_plan_review",
        "edit_plan_review",
        "pop_current_plan_review",
        "start_run",
        "edit_head_fields",
        "full history child edit",
        "push_aggregate_review",
        "edit_aggregate_review",
        "supersede_pending_aggregate_reviews",
        "terminalize_pending_plan_reviews",
        "push_checkpoint",
        "restore_checkpoints",
        "push_switch",
        "push_wait",
        "push_decision",
        "edit_wait",
        "edit_decision",
        "mark_effect_applied",
        "set_observe_cursor",
        "set_usage",
        "append_usage_entries",
        "refused run id",
        "refused run started",
        "refused run ended",
        "refused run usage",
        "refused plan run id",
        "refused plan run ended",
        "refused plan id",
        "refused attempt id",
        "refused attempt started",
        "refused child id",
        "refused child started",
        "refused plan review id",
        "refused plan review inbox",
        "refused aggregate review id",
        "refused wait id",
        "refused decision id",
        "refused duplicate child",
        "refused duplicate wait",
    ] {
        assert!(ran.contains(op), "{op} never ran: {ran:?}");
    }
}
