//! T-331 review round 2 regressions on both backends, inside one `update_head` operation:
//! keyed lookups and the checkpoint cursor keep their bounds after a mutation (F4.1), emitted
//! readers count exactly the documents they return (F3.1), and the facts and digests a
//! `TaskRef` read serves agree with the full task right after each edit (F4.2).

mod store_support;
#[path = "task_ref_support/mod.rs"]
mod task_ref_support;

use anyhow::Result;
use corral_core::work::{self, Counter, HotScope, WorkSnapshot};
use corral_core::{
    FindingSource, Status, Store, Task, TaskMut, TaskRef, Verdict, implementation_review_round_ref,
    plan_review_round_ref,
};
use serde_json::json;
use store_support::*;
use task_ref_support::{backends, norm_plan, norm_run};

fn measure<T>(f: impl FnOnce() -> Result<T>) -> Result<(T, WorkSnapshot)> {
    let _hot = HotScope::enter();
    let before = work::snapshot();
    let value = f()?;
    let delta = work::snapshot().since(&before);
    delta.check_identity().map_err(anyhow::Error::msg)?;
    Ok((value, delta))
}

fn with_history(store: &Store, history: usize) -> Task {
    let mut task = store
        .create("Reads", Some("app"), Status::Running, "Spec\n")
        .unwrap();
    grow(&mut task, 2, history, 1, 1);
    task.plans[0] = serde_json::from_value(plan(&task.id, "P0001", history)).unwrap();
    let run = task.runs.last().unwrap().id.clone();
    task.runs.last_mut().unwrap().checkpoints = (1..=history)
        .map(|n| serde_json::from_value(checkpoint(&task.id, &run, n, 1)).unwrap())
        .collect();
    store.fixture_put_task(&task).unwrap();
    store.get(&task.id).unwrap()
}

/// The mutations a bounded read follows: none of them touches the history it looks up.
fn mutate<M: TaskMut>(task: &mut M, variant: usize) -> Result<()> {
    let run = task.current_run_head()?.unwrap().id.clone();
    match variant {
        0 => task.edit_head_fields(|fields| {
            fields.title = "changed".into();
            Ok(())
        }),
        1 => task.push_plan_review(
            "P0001",
            serde_json::from_value(plan_review("P0001-PR0900", "rev", None))?,
        ),
        2 => task.push_review_child(
            &run,
            serde_json::from_value(review_child("T-001", &run, 900, 1, false, "changes"))?,
        ),
        _ => task.push_checkpoint(
            &run,
            serde_json::from_value(checkpoint("T-001", &run, 900, 1))?,
        ),
    }
}

fn bounded_reads<T: TaskRef>(task: &T, at: jiff::Timestamp) -> Result<Vec<(String, u64)>> {
    let run = task.current_run_head()?.unwrap().id.clone();
    let mut reads = Vec::new();
    let (found, delta) = measure(|| task.plan_review_approved_by("P0001", "unnamed"))?;
    reads.push((format!("{found:?}"), delta.get(Counter::NestedRowDecodes)));
    let (found, delta) = measure(|| task.review_child_by_review_at(&run, at))?;
    let found = found.map(norm_run);
    reads.push((format!("{found:?}"), delta.get(Counter::NestedRowDecodes)));
    let (found, delta) = measure(|| {
        let mut metas = task.checkpoint_metas_newest_first(&run)?;
        metas.next().transpose()
    })?;
    reads.push((format!("{found:?}"), delta.get(Counter::NestedRowDecodes)));
    Ok(reads)
}

#[test]
fn bounded_lookups_keep_their_bound_after_a_mutation() {
    for variant in 0..4 {
        let mut costs: std::collections::BTreeMap<&str, Vec<Vec<u64>>> = Default::default();
        for history in [3, 30] {
            for (backend, _dir, store) in backends() {
                let full = with_history(&store, history);
                let at = full.runs.last().unwrap().review_runs[0]
                    .review
                    .as_ref()
                    .unwrap()
                    .at;
                let mut expected = full.clone();
                mutate(&mut expected, variant).unwrap();
                let want: Vec<String> = bounded_reads(&expected, at)
                    .unwrap()
                    .into_iter()
                    .map(|(text, _)| text)
                    .collect();
                let mut got = Vec::new();
                store
                    .update_head(&full.id, |head| {
                        mutate(head, variant)?;
                        got = bounded_reads(head, at)?;
                        Ok(())
                    })
                    .unwrap();
                let texts: Vec<String> = got.iter().map(|(text, _)| text.clone()).collect();
                assert_eq!(texts, want, "{backend} variant {variant} history {history}");
                costs
                    .entry(backend)
                    .or_default()
                    .push(got.into_iter().map(|(_, n)| n).collect());
            }
        }
        for (backend, costs) in costs {
            assert_eq!(costs[0], costs[1], "{backend} variant {variant}: 3 vs 30");
            if variant == 0 {
                assert_eq!(costs[0], vec![1, 1, 1], "{backend}: a head-field edit");
            }
        }
    }
}

fn verdict_edits<M: TaskMut>(task: &mut M) -> Result<Vec<String>> {
    let mut seen = Vec::new();
    let run = task.current_run_head()?.unwrap().into_owned();
    let child = run.current_child.as_ref().unwrap().id.clone();
    task.edit_review_child(&run.id, &child, |c| {
        c.review.as_mut().unwrap().verdict = Some(Verdict::Changes);
        c.harness = Some("other-harness".into());
        Ok(())
    })?;
    seen.push(format!("{}", implementation_review_round_ref(task)?));
    seen.push(format!(
        "{:?}",
        task.current_run_head()?.map(|r| norm_run(r.into_owned()))
    ));
    let plan = task.last_plan_head()?.unwrap().into_owned();
    let review = plan.current_review.as_ref().unwrap().id.clone();
    for verdict in [Some(Verdict::Changes), None] {
        task.edit_plan_review(&plan.id, &review, |r| {
            r.verdict = verdict;
            Ok(())
        })?;
        seen.push(format!("{}", plan_review_round_ref(task)?));
        seen.push(format!(
            "{:?}",
            task.last_plan_head()?.map(|p| norm_plan(p.into_owned()))
        ));
        let pending: Vec<String> = task
            .pending_plan_reviews(&plan.id)?
            .into_iter()
            .map(|r| r.id)
            .collect();
        seen.push(format!("{pending:?}"));
    }
    task.edit_head_fields(|fields| {
        fields.title = "a new contract".into();
        fields.kind = Some("bug".into());
        Ok(())
    })?;
    seen.push(task.contract_revision()?);
    task.append_usage_entries(
        &run.id,
        None,
        vec![serde_json::from_value(usage_entry(77, true))?],
    )?;
    seen.push(format!(
        "{:?}",
        task.current_run_head()?.map(|r| norm_run(r.into_owned()))
    ));
    Ok(seen)
}

#[test]
fn facts_agree_with_the_full_task_right_after_each_edit() {
    for (backend, _dir, store) in backends() {
        let task = rich(&store, 1, 1, 1, 1);
        let mut full = task.clone();
        let expected = verdict_edits(&mut full).unwrap();
        let mut got = Vec::new();
        store
            .update_head(&task.id, |head| {
                got = verdict_edits(head)?;
                Ok(())
            })
            .unwrap();
        for (i, (want, got)) in expected.iter().zip(&got).enumerate() {
            assert_eq!(want, got, "{backend} read {i}");
        }
        assert_eq!(expected.len(), got.len());
    }
}

fn comment(text: &str) -> corral_core::TaskComment {
    serde_json::from_value(json!({"author": "owner", "at": at(9), "text": text})).unwrap()
}

#[test]
fn emitted_comments_count_each_returned_document_once() {
    for (backend, _dir, store) in backends() {
        let task = rich(&store, 1, 1, 1, 1);
        let mut full = task.clone();
        full.add_comment(comment("newly queued comment")).unwrap();
        let expected = full.emitted_comments().unwrap();
        store
            .update_head(&task.id, |head| {
                TaskMut::add_comment(head, comment("newly queued comment"))?;
                let (comments, delta) = measure(|| TaskRef::emitted_comments(head))?;
                assert_eq!(comments, expected, "{backend}");
                assert_eq!(delta.emitted_rows(), comments.len() as u64, "{backend}");
                let bytes: usize = comments
                    .iter()
                    .map(|c| serde_json::to_vec(c).unwrap().len())
                    .sum();
                assert_eq!(delta.emitted_doc_bytes(), bytes as u64, "{backend}");
                Ok(())
            })
            .unwrap();
    }
}

#[test]
fn emitted_findings_count_replacements_removals_and_exclusions_exactly() {
    for (backend, _dir, store) in backends() {
        let mut task = store
            .create("Findings", Some("app"), Status::Running, "Spec\n")
            .unwrap();
        grow(&mut task, 2, 3, 1, 1);
        for run in &mut task.runs {
            for child in &mut run.review_runs {
                let review = child.review.as_mut().unwrap();
                review.verdict = Some(Verdict::Changes);
                review.notes = Some(format!("{} notes", child.id));
            }
        }
        for review in &mut task.plans.last_mut().unwrap().reviews {
            review.verdict = Some(Verdict::Changes);
            review.notes = Some(format!("{} notes", review.id));
        }
        store.fixture_put_task(&task).unwrap();
        let task = store.get(&task.id).unwrap();
        let run = task.runs.last().unwrap().id.clone();
        let current = task
            .runs
            .last()
            .unwrap()
            .review_runs
            .last()
            .unwrap()
            .id
            .clone();
        let plan = task.plans.last().unwrap().id.clone();
        let mut full = task.clone();
        full.apply(&run, &current, &plan).unwrap();
        let cases = [
            (FindingSource::Review, None),
            (FindingSource::Review, Some(current.clone())),
            (FindingSource::Plan, None),
        ];
        store
            .update_head(&task.id, |head| {
                head.apply(&run, &current, &plan)?;
                for (source, exclude) in &cases {
                    let expected = full.emitted_findings(*source, exclude.as_deref())?;
                    let (found, delta) =
                        measure(|| TaskRef::emitted_findings(head, *source, exclude.as_deref()))?;
                    assert_eq!(found, expected, "{backend} {source:?} {exclude:?}");
                    assert_eq!(delta.emitted_rows(), found.len() as u64);
                    let bytes: usize = found
                        .iter()
                        .map(|f| serde_json::to_vec(f).unwrap().len())
                        .sum();
                    assert_eq!(
                        delta.emitted_doc_bytes(),
                        bytes as u64,
                        "{backend} {source:?} {exclude:?}"
                    );
                }
                Ok(())
            })
            .unwrap();
    }
}

/// Replaces the current review child's finding, removes the current plan review's and adds
/// a new plan review finding, all in one operation.
trait MutFindings {
    fn apply(&mut self, run: &str, child: &str, plan: &str) -> Result<()>;
}

impl<M: TaskMut> MutFindings for M {
    fn apply(&mut self, run: &str, child: &str, plan: &str) -> Result<()> {
        self.edit_review_child(run, child, |c| {
            c.review.as_mut().unwrap().notes = Some("replaced notes, longer than before".into());
            Ok(())
        })?;
        self.pop_current_plan_review(plan)?;
        let mut review = plan_review(&format!("{plan}-PR0900"), "rev", Some("changes"));
        review["notes"] = "a new plan finding".into();
        self.push_plan_review(plan, serde_json::from_value(review)?)
    }
}
