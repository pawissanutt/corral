//! Stopped planner context is preserved in the one transfer journal and ordinary R0
//! artifacts. A draft checkpoint is neither a submission, approval nor a stop request.
use super::{Content, Journal, lifecycle};
use crate::Ctx;
use anyhow::{Context, Result, ensure};
use corral_core::{PlanRunHead, Store, Task, TaskRef};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pin {
    run: String,
    spec_revision: String,
    output_dir: PathBuf,
    submitted: Option<String>,
    rounds: u32,
    generation: Option<u32>,
    ended: Option<jiff::Timestamp>,
}
impl Pin {
    fn of(run: &PlanRunHead) -> Self {
        Self {
            run: run.id.clone(),
            spec_revision: run.spec_revision.clone(),
            output_dir: run.output_dir.clone(),
            submitted: run.submitted.clone(),
            rounds: run.rounds,
            generation: run.agent_host.as_ref().map(|s| s.binding.generation),
            ended: run.ended,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Draft {
    pin: Pin,
    predecessor: Option<String>,
    body: Option<Content>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Checkpoints {
    latest: Option<String>,
    rows: BTreeMap<String, Draft>,
}

fn predecessor<T: TaskRef + ?Sized>(task: &T) -> Result<Option<String>> {
    Ok(task.last_plan_head()?.map(|p| p.id.clone()))
}
fn synthetic(ctx: &Ctx, task: &str, pin: &Pin) -> bool {
    let root = ctx.store.dir().join("handoff-plan").join(task);
    pin.output_dir == root || pin.output_dir == root.join(&pin.run)
}

/// Only the registered canonical planner directory is eligible, never an arbitrary
/// path from a late reply. All components and the final regular file are no-follow.
fn read(ctx: &Ctx, task: &str, pin: &Pin, archived: bool) -> Result<Option<Vec<u8>>> {
    if synthetic(ctx, task, pin) {
        return Ok(None);
    } // provenance never launched a planner
    let expected = crate::scratch_reclaim::Attempt::Planner {
        task,
        run: &pin.run,
    }
    .dir(ctx)?;
    ensure!(
        pin.output_dir == expected,
        "handoff_draft_source: unsupported/unconfined planner directory"
    );
    match expected.symlink_metadata() {
        Ok(meta) => ensure!(
            meta.file_type().is_dir(),
            "handoff_draft_source: planner directory changed type"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            ensure!(
                archived,
                "handoff_draft_missing: current planner source disappeared before checkpoint"
            );
            return Ok(None);
        }
        Err(error) => return Err(error).context("inspect planner directory"),
    }
    let guard = corral_core::fs_guard::DirHandle::open(&expected)?;
    let path = expected.join("plan.md");
    let bytes = match path.symlink_metadata() {
        Ok(_) => {
            let cap = ctx
                .store
                .config
                .artifacts
                .max_bytes
                .min(corral_core::assistant_workflow::DOCUMENT_MAX as u64);
            let bytes = super::content::regular(&path, usize::try_from(cap)?)?.0;
            std::str::from_utf8(&bytes)
                .context("handoff_draft_utf8: retain/reconcile incomplete planner output")?;
            Some(bytes)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).context("inspect canonical planner draft"),
    };
    guard.revalidate()?;
    Ok(bytes)
}

/// Caller has settled all conflicting resources and holds the pipeline/launch/gate
/// leases. Archive before source cleanup, another planner, or local ownership return.
pub(super) fn capture(ctx: &Ctx, id: &str) -> Result<()> {
    let task = ctx.store.get(id)?;
    let Some(run) = task.last_plan_run_head()? else {
        return Ok(());
    };
    ensure!(
        !run.holds_slot(),
        "handoff_draft_busy: planner must settle before capture"
    );
    let pin = Pin::of(&run);
    let journal = lifecycle::for_task(&ctx.store, &task)?.context("transfer missing")?;
    let transfer = journal.transfer.as_ref().context("transfer missing")?;
    let old = transfer
        .planner_drafts
        .rows
        .get(&pin.run)
        .filter(|d| d.pin == pin);
    let observed = read(ctx, id, &pin, old.is_some())?;
    let body = match &observed {
        Some(bytes) => Some(Content::put(&ctx.store, bytes)?),
        None => old.and_then(|d| d.body.clone()), // committed checkpoint survives source loss
    };
    let draft = Draft {
        pin: pin.clone(),
        predecessor: predecessor(&task)?,
        body,
    };
    if old == Some(&draft) {
        return Ok(());
    }
    let contract = task.contract_revision()?;
    ctx.store.update(id, |task| {
        let current = task.last_plan_run_head()?.context("planner disappeared")?;
        ensure!(
            !current.holds_slot(),
            "handoff_draft_busy: planner reopened"
        );
        ensure!(
            Pin::of(&current) == pin
                && task.contract_revision()? == contract
                && predecessor(task)? == draft.predecessor
                && read(ctx, id, &pin, old.is_some())? == observed,
            "handoff_draft_changed: source/run/spec changed while checkpointing"
        );
        let mut journal = lifecycle::for_task(&ctx.store, task)?.context("transfer missing")?;
        let transfer = journal.transfer.as_mut().context("transfer missing")?;
        ensure!(
            transfer.planner_drafts.rows.len() < 64
                || transfer.planner_drafts.rows.contains_key(&pin.run),
            "handoff_draft_history_full: preserve/reconcile before another planner checkpoint"
        );
        if draft.body.is_some() {
            transfer.planner_drafts.latest = Some(pin.run.clone());
        }
        transfer
            .planner_drafts
            .rows
            .insert(pin.run.clone(), draft.clone());
        transfer.advance()?; // an old exact choice never silently binds a newer draft
        if let Some(body) = &draft.body {
            let bytes = body.load(&ctx.store)?;
            let stable = crate::verification::digest(journal.id.as_bytes());
            super::attachments::attach(
                ctx,
                task,
                &stable,
                &format!("planner-{}-draft", pin.run),
                &bytes,
            )?;
        }
        super::recovery::save(&ctx.store, &journal)
    })?;
    Ok(())
}

pub(super) fn require_current<T: TaskRef + ?Sized>(
    ctx: &Ctx,
    task: &T,
    journal: &Journal,
) -> Result<()> {
    let Some(run) = task.last_plan_run_head()? else {
        return Ok(());
    };
    let pin = Pin::of(&run);
    let checkpoints = &journal
        .transfer
        .as_ref()
        .context("transfer missing")?
        .planner_drafts;
    let Some(saved) = checkpoints.rows.get(&pin.run) else {
        // A normal same-task formal provenance record has no writer/draft to drain.
        ensure!(
            synthetic(ctx, task.id(), &pin),
            "handoff_draft_checkpoint: Save/settle the current planner before changing goals"
        );
        return Ok(());
    };
    ensure!(
        saved.pin == pin && !run.holds_slot(),
        "handoff_draft_changed: planner changed/reopened; Save and refresh"
    );
    if let Some(bytes) = read(ctx, task.id(), &pin, true)? {
        ensure!(
            saved.body.as_ref() == Some(&Content::of(&bytes)),
            "handoff_draft_changed: canonical draft changed after checkpoint; Save and refresh"
        );
    }
    Ok(())
}

pub(super) fn latest<T: TaskRef + ?Sized>(
    store: &Store,
    task: &T,
    journal: &Journal,
) -> Result<Option<Vec<u8>>> {
    let checkpoints = &journal
        .transfer
        .as_ref()
        .context("transfer missing")?
        .planner_drafts;
    let Some(draft) = checkpoints
        .latest
        .as_ref()
        .and_then(|id| checkpoints.rows.get(id))
    else {
        return Ok(None);
    };
    if draft.predecessor != predecessor(task)? {
        return Ok(None);
    } // a newer submission superseded this context
    draft.body.as_ref().map(|c| {
        ensure!(c.bytes as u64 <= store.config.artifacts.max_bytes.min(corral_core::assistant_workflow::DOCUMENT_MAX as u64),
            "handoff_draft_oversized: archived planner draft exceeds current artifact/workflow cap");
        c.load(store)
    }).transpose()
}

pub(super) fn pending<T: TaskRef + ?Sized>(
    task: &T,
    transfer: &lifecycle::Transfer,
) -> Result<bool> {
    let checkpoints = &transfer.planner_drafts;
    let Some(draft) = checkpoints
        .latest
        .as_ref()
        .and_then(|id| checkpoints.rows.get(id))
    else {
        return Ok(false);
    };
    let current = task.last_plan_head()?;
    Ok(draft.predecessor == current.as_ref().map(|p| p.id.clone())
        && draft.body.as_ref().is_some_and(|b| {
            current
                .as_ref()
                .is_none_or(|p| p.artifact.sha256 != b.blob.hash().as_str())
        }))
}

pub(super) fn returned(ctx: &Ctx, task: &Task, journal: &Journal) -> Result<Option<Vec<u8>>> {
    if let Some(bytes) = latest(&ctx.store, task, journal)? {
        ensure!(
            !bytes.is_empty(),
            "handoff_draft_empty: empty latest draft retained; reconcile before local plan publication"
        );
        return Ok(Some(bytes));
    }
    let transfer = journal.transfer.as_ref().context("transfer missing")?;
    if super::plan_adoption::draft_pending(task, transfer)? {
        return Ok(None);
    } // preserve newer local raw source
    task.plans
        .last()
        .map(|p| {
            crate::artifact_storage::artifact_read_confined_limited(
                ctx,
                &p.artifact,
                ctx.store
                    .config
                    .artifacts
                    .max_bytes
                    .min(corral_core::assistant_workflow::DOCUMENT_MAX as u64),
            )
        })
        .transpose()
}

/// Existing scheduler still owns route/capacity/dependency/authorization. This only
/// settles the previous C3 writer and preserves its context before another D identity.
pub(crate) fn before_plan(ctx: &Ctx, task: &impl TaskRef) -> Result<()> {
    if !task
        .cowork()
        .as_ref()
        .is_some_and(|c| c.review == corral_core::CoworkReview::TransferV1)
    {
        return Ok(());
    }
    let _pipeline = ctx
        .store
        .try_lock(&format!("autopilot-{}", task.id()))?
        .context("Waiting for prior controller")?;
    let _gate = ctx
        .store
        .try_lock(&format!("gate-{}", task.id()))?
        .context("Waiting for prior gate")?;
    let holders = super::settlement::settle(ctx, task.id())?;
    ensure!(
        holders.is_empty(),
        "handoff_draft_busy: {}",
        holders.join("; ")
    );
    capture(ctx, task.id())
}

/// GC never drops an uncheckpointed latest source while Save/return waits. Prior
/// attempts were checkpointed before their replacement; inspect one exact run only.
pub(crate) fn retention(
    ctx: &Ctx,
    row: &corral_core::backend::ScratchRow,
) -> Result<Option<String>> {
    if row.kind != corral_core::backend::ScratchKind::PlannerOutput {
        return Ok(None);
    }
    let (Some(task), Some(run)) = (&row.task, &row.attempt) else {
        return Ok(None);
    };
    let Some(task) = ctx.store.head_if_exists(task)? else {
        return Ok(None);
    };
    if !task
        .cowork
        .as_ref()
        .is_some_and(|c| c.review == corral_core::CoworkReview::TransferV1)
    {
        return Ok(None);
    }
    let journal = lifecycle::for_task(&ctx.store, &task)?.context("transfer missing")?;
    let transfer = journal.transfer.as_ref().context("transfer missing")?;
    let Some(source) = task.plan_run_head(run)? else {
        return Ok(Some("Co-work planner provenance unavailable".into()));
    };
    let pin = Pin::of(&source);
    let archived = transfer
        .planner_drafts
        .rows
        .get(run)
        .filter(|d| d.pin == pin);
    if let Some(archive) = archived
        && read(ctx, &task.id, &pin, true)?
            .as_ref()
            .is_none_or(|b| archive.body.as_ref() == Some(&Content::of(b)))
    {
        return Ok(None);
    }
    Ok(Some(
        "Co-work planner draft not checkpointed; retain until settled Save/return".into(),
    ))
}
