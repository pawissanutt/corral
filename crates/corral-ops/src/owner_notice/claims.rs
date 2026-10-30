//! Claim construction reads typed task fields/current metadata; no Task shell is manufactured.
use super::{PendingNotice, prepare, refresh_fact, reserve_task};
use crate::Ctx;
use anyhow::{Context, Result};
use corral_core::attention::{EntryNotice, NoticeClaim, NoticeCoverage};
use corral_core::{AttentionLedger, Task, TaskHeadMut, TaskRef};
use jiff::Timestamp;

fn entry_claim<T: TaskRef + ?Sized>(
    ctx: &Ctx,
    task: &T,
    producer: &str,
    now: Timestamp,
    exact: Vec<String>,
) -> Result<NoticeClaim> {
    let current = task.current_run_head()?;
    let source = crate::verification::digest(&serde_json::to_vec(&(
        current.as_ref().map(|run| run.id.as_str()),
        task.status_since(),
        now,
    ))?);
    let mut coverage = vec![NoticeCoverage::Episode {
        task: task.id().clone(),
        run: current.as_ref().map(|run| run.id.clone()),
        stage: "needs_me".into(),
        at: now,
    }];
    coverage.extend(
        exact
            .into_iter()
            .map(|condition_key| NoticeCoverage::Exact { condition_key }),
    );
    let mut claim = prepare(ctx, task, producer, &source, now, coverage)?;
    let pm_entry = current
        .as_ref()
        .is_some_and(|run| run.agent_state.as_deref() == Some("gone") || run.idle_no_report)
        || producer == "native_review_host_lost";
    if !pm_entry {
        claim.exemption_basis = Some("owner_responsible_entry".into());
    }
    refresh_fact(&mut claim)?;
    Ok(claim)
}

fn insert_entry(
    ctx: &Ctx,
    id: &str,
    ledger: &mut AttentionLedger,
    claim: NoticeClaim,
    now: Timestamp,
) -> Result<PendingNotice> {
    let batch = claim.batch_id.clone();
    let pending = reserve_task(ctx, id, batch.clone())?;
    ledger.entry_notice = Some(EntryNotice {
        batch_id: batch,
        at: now,
        status_since: None,
    });
    ledger.push_notice(claim)?;
    Ok(pending)
}

pub(crate) fn record_entry_head(
    ctx: &Ctx,
    head: &mut TaskHeadMut,
    producer: &str,
    now: Timestamp,
    exact: Vec<String>,
) -> Result<PendingNotice> {
    let claim = entry_claim(ctx, head, producer, now, exact)?;
    let id = head.id.clone();
    insert_entry(ctx, &id, &mut head.attention, claim, now)
}

pub(crate) fn record_entry(
    ctx: &Ctx,
    task: &mut Task,
    producer: &str,
    now: Timestamp,
    exact: Vec<String>,
) -> Result<PendingNotice> {
    let claim = entry_claim(ctx, task, producer, now, exact)?;
    let id = task.id.clone();
    insert_entry(ctx, &id, &mut task.attention, claim, now)
}

fn stage_claim<T: TaskRef + ?Sized>(
    ctx: &Ctx,
    task: &T,
    producer: &str,
    now: Timestamp,
) -> Result<NoticeClaim> {
    let run = task
        .current_run_head()?
        .context("stage notice has no run")?;
    let stage = run
        .autopilot
        .as_ref()
        .context("stage notice has no stage")?;
    let source = format!("{}/{}/{now}", run.id, stage.stage.as_str());
    prepare(
        ctx,
        task,
        producer,
        &source,
        now,
        vec![NoticeCoverage::Episode {
            task: task.id().clone(),
            run: Some(run.id.clone()),
            stage: stage.stage.as_str().into(),
            at: stage.at,
        }],
    )
}

pub(crate) fn record_stage_head(
    ctx: &Ctx,
    head: &mut TaskHeadMut,
    producer: &str,
    now: Timestamp,
) -> Result<PendingNotice> {
    let claim = stage_claim(ctx, head, producer, now)?;
    let pending = reserve_task(ctx, &head.id, claim.batch_id.clone())?;
    head.attention.push_notice(claim)?;
    Ok(pending)
}

pub(crate) fn record_stage(
    ctx: &Ctx,
    task: &mut Task,
    producer: &str,
    now: Timestamp,
) -> Result<PendingNotice> {
    let claim = stage_claim(ctx, task, producer, now)?;
    let pending = reserve_task(ctx, &task.id, claim.batch_id.clone())?;
    task.attention.push_notice(claim)?;
    Ok(pending)
}
