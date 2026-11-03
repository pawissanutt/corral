//! Inputs shared by the read-only PM components on their pinned view.
use std::collections::BTreeSet;
use std::sync::Arc;

use anyhow::Result;
use corral_core::backend::{HeadQuery, PageReq, RowQuery, TaskPart};
use corral_core::work::{self, Counter, Subject};
use corral_core::{Store, Task, TaskHead};

use crate::Ctx;

pub(crate) fn events(
    ctx: &Ctx,
    task: Option<&str>,
    kinds: &[&str],
    run: Option<&str>,
    limit: u32,
) -> Result<Vec<corral_core::Event>> {
    let query = corral_core::backend::EventQuery {
        task: task.map(str::to_owned),
        kinds: kinds.iter().map(|kind| (*kind).to_owned()).collect(),
        run: run.map(str::to_owned),
        newest_first: true,
        valid_only: true,
        ..Default::default()
    };
    ctx.store
        .view()?
        .events(&query, PageReq::first(limit))?
        .items
        .into_iter()
        .map(event_of)
        .collect()
}

pub(crate) fn event_of(row: corral_core::backend::StoredEvent) -> Result<corral_core::Event> {
    let event = row.event;
    Ok(corral_core::Event {
        seq: row.seq.0,
        ts: event.ts,
        task: event.task,
        kind: event.kind,
        actor: event
            .actor
            .map(|actor| serde_json::from_value(actor.into()))
            .transpose()?,
        via: event.via,
        detail: event.detail,
    })
}

pub(crate) fn open(store: &Store) -> Result<Arc<Vec<TaskHead>>> {
    store.pass_derived("pm-open-heads", || {
        let heads = store.cached_board_checked()?.heads;
        crate::pm_workset::after_open_heads();
        Ok(heads)
    })
}

pub(crate) fn inputs(ctx: &Ctx) -> Result<Arc<Vec<TaskHead>>> {
    ctx.store.pass_derived("pm-input-heads", || {
        let mut heads = open(&ctx.store)?.as_ref().clone();
        let policy = ctx.store.effective_operating_policy()?;
        let mut seen: BTreeSet<_> = heads.iter().map(|h| h.id.clone()).collect();
        let mut ids: BTreeSet<_> = policy.tasks.keys().cloned().collect();
        loop {
            for head in &heads {
                ids.extend(head.depends_on.iter().cloned());
                ids.extend(head.children.iter().cloned());
                ids.extend(head.parent.iter().cloned());
            }
            ids.retain(|id| !seen.contains(id));
            if ids.is_empty() {
                break;
            }
            let found = ctx
                .store
                .heads(&HeadQuery::Ids(ids.iter().cloned().collect()))?;
            seen.extend(ids.iter().cloned());
            ids.clear();
            heads.extend(found);
        }
        heads.sort_by(|a, b| a.created.cmp(&b.created).then(a.id.cmp(&b.id)));
        Ok(heads)
    })
}

pub(crate) fn shell(head: &TaskHead) -> Result<Task> {
    let mut task = head.to_shell_task()?;
    if let (Some(run), Some(current)) = (task.runs.last_mut(), &head.current_run) {
        run.obligations
            .decisions
            .extend(current.facts.pending_decision.clone());
    }
    Ok(task)
}

/// The review requirement may retain one superseded stronger-review source.
/// Read that exact child, not the run's accumulated review history.
pub(crate) fn review_task(head: &TaskHead, task: &Task, sha: &str) -> Result<Task> {
    let mut task = task.clone();
    if let Some(run) = task.runs.last_mut()
        && run
            .review
            .as_ref()
            .and_then(|review| review.route.as_deref())
            .is_some()
    {
        let exclude = run
            .review
            .as_ref()
            .and_then(|review| review.route.as_deref())
            .and_then(|route| route.attempt.as_deref())
            .or_else(|| run.review_runs.last().map(|child| child.id.as_str()));
        if let Some(source) = head.stronger_source(&run.id, sha, exclude)?
            && !run.review_runs.iter().any(|child| child.id == source)
            && let Some(child) = head.review_child(&run.id, &source)?
        {
            run.review_runs
                .insert(0, serde_json::from_value(serde_json::to_value(child)?)?);
        }
    }
    Ok(task)
}

pub(crate) fn observed_task(ctx: &Ctx, head: &TaskHead) -> Result<Task> {
    let mut task = shell(head)?;
    if let Some(current) = &head.current_run
        && task.awaiting_owner()
    {
        let paths: BTreeSet<_> = task
            .current_run()
            .into_iter()
            .flat_map(|run| &run.completion.evidence)
            .flat_map(|evidence| &evidence.observation.artifacts)
            .collect();
        task.artifacts = paths
            .into_iter()
            .map(|path| head.artifact_in_run(&current.id, path))
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect();
    }
    if let Some(plan_run) = head.last_plan.as_ref().and_then(|plan| plan.run.as_deref())
        && head
            .last_plan_run
            .as_ref()
            .is_none_or(|run| run.id != plan_run)
    {
        let records = rows(
            ctx,
            head,
            &RowQuery::part(TaskPart::PlanRun).with_index(format!("id:{plan_run}")),
        )?;
        if let Some(record) = records.into_iter().next() {
            task.plan_runs
                .insert(0, serde_json::from_value(record.doc)?);
        }
    }
    if let (Some(run), Some(current)) = (task.runs.last_mut(), &head.current_run) {
        let ordinal = head.summary.runs + 1;
        let mut owners = vec![(
            format!("{ordinal:06}/000000/questions/"),
            &current.facts,
            run.agent_host.as_deref_mut(),
        )];
        if let (Some(child), Some(child_head)) =
            (run.review_runs.last_mut(), current.current_child.as_deref())
        {
            owners.push((
                format!("{ordinal:06}/{:06}/questions/", current.facts.children),
                &child_head.facts,
                child.agent_host.as_deref_mut(),
            ));
        }
        for (prefix, facts, host) in owners {
            if facts.pending_questions == 0 {
                continue;
            }
            if let Some(host) = host {
                host.questions = rows(
                    ctx,
                    head,
                    &RowQuery::under(TaskPart::AgentRecord, prefix).open(),
                )?
                .into_iter()
                .map(|row| Ok(serde_json::from_value(row.doc)?))
                .collect::<Result<_>>()?;
            }
        }
    }
    Ok(task)
}

pub(crate) fn rows(
    ctx: &Ctx,
    head: &TaskHead,
    query: &RowQuery,
) -> Result<Vec<corral_core::backend::HistoryRow>> {
    let rows = ctx
        .store
        .view()?
        .rows(head.num, query, PageReq::first(u32::MAX))?
        .items;
    for row in &rows {
        let counter = match row.part {
            TaskPart::Run
            | TaskPart::PlanRun
            | TaskPart::Plan
            | TaskPart::AggregateAttempt
            | TaskPart::Artifact
            | TaskPart::Comment => Counter::HistoryRowDecodes,
            _ => Counter::NestedRowDecodes,
        };
        work::decoded(
            counter,
            u64::try_from(serde_json::to_vec(&row.doc)?.len())?,
            Subject::Plain,
        );
    }
    Ok(rows)
}
