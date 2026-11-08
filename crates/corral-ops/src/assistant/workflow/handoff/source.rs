//! Resolve existing source holders, including historical run locations, before filing.
//! A source/provenance link is never interpreted as current edit authority.
use super::{
    Content, Package, Prepared,
    lifecycle::{self, Disposition},
};
use crate::Ctx;
use anyhow::{Context, Result, ensure};
use corral_core::{Task, TaskRef};
use std::collections::BTreeSet;

pub(super) fn task(ctx: &Ctx, prepared: &Prepared) -> Result<Option<String>> {
    let p = prepared.package();
    let container = ctx.store.get_assistant_container(&p.assistant)?;
    let mut reference = super::super::current(&container)?.handoff.clone();
    if reference
        .as_ref()
        .is_some_and(|r| r.blob == prepared.publication.package.blob)
    {
        // Reservation replaced the visible pointer; preserve the earlier returned
        // lineage from THIS journal rather than cold-filing another task.
        let journal = super::read_journal(&ctx.store, prepared.id())?
            .context("source reservation missing")?;
        ensure!(
            journal.previous_handoff_known,
            "handoff_source_lineage: earlier scope is unknown; reconcile instead of filing a rival"
        );
        reference = journal.previous_handoff;
    }
    if let Some(reference) = reference
        && reference.blob != prepared.publication.package.blob
    {
        let old: Package = serde_json::from_slice(
            &Content {
                blob: reference.blob.clone(),
                bytes: reference.bytes as usize,
            }
            .load(&ctx.store)?,
        )?;
        let id = format!(
            "{}/{}",
            super::marker(&p.assistant, &old.expected),
            reference.blob.hash()
        );
        if let Some(journal) = super::read_journal(&ctx.store, &id)?
            && let Some(transfer) = journal
                .transfer
                .as_ref()
                .filter(|t| t.disposition == Disposition::Returned)
        {
            ensure!(
                old.project == p.project,
                "handoff_scope: this host still owns another scope; open a new assistant for unrelated work"
            );
            let expected_path = if let Some(slug) = &transfer.returned_scratch {
                ctx.store
                    .get_assistant(&p.assistant)?
                    .scratch
                    .into_iter()
                    .find(|s| &s.slug == slug)
                    .map(|s| s.path)
                    .context("handoff_source_changed: latest returned scratch is missing")?
                    .into()
            } else {
                old.source_path.clone()
            };
            ensure!(
                p.source_path == expected_path,
                "handoff_source_changed: historical/unrelated scratch is not the current scope; use the latest returned work or a new assistant"
            );
            return Ok(Some(transfer.task.clone()));
        }
    }
    let Some(path) = p.source_path.as_ref() else {
        return Ok(None);
    };
    // Existing scalar projection reads no task/container history payloads. This cold
    // compatibility lookup also finds tasks created before their host acknowledged them.
    let view = ctx.store.view()?;
    let owners: BTreeSet<_> = view
        .run_worktrees(false)?
        .into_iter()
        .filter(|r| &r.worktree == path)
        .map(|r| r.task)
        .collect();
    ensure!(
        owners.len() <= 1,
        "handoff_ambiguous: source belongs to multiple task histories; reconcile, never file a rival"
    );
    owners
        .into_iter()
        .next()
        .map(|num| -> Result<String> {
            Ok(view
                .task(num, &corral_core::backend::TaskParts::Only(Vec::new()))?
                .context("handoff_source_changed: source task disappeared")?
                .head
                .header
                .id)
        })
        .transpose()
}

pub(super) fn driver(ctx: &Ctx, task: &Task, prepared: &Prepared) -> Result<bool> {
    let p = prepared.package();
    let Some((session, scratch)) = task.cowork.as_ref().and_then(|c| c.assistant_driver()) else {
        return Ok(false);
    };
    if session != p.assistant {
        return Ok(false);
    }
    if let Some(path) = &p.source_path
        && ctx
            .store
            .get_assistant(session)?
            .scratch
            .iter()
            .any(|s| &s.path == path && s.slug == scratch)
    {
        return Ok(true);
    }
    if scratch.is_empty() {
        return Ok(lifecycle::for_task(&ctx.store, task)?
            .and_then(|j| j.transfer)
            .is_some_and(|t| {
                t.disposition == Disposition::Returned && lifecycle::plan_only(t.goal)
            }));
    }
    Ok(false)
}

/// Once reservation closes the assistant writer, its existing tracking task must also
/// stop future checks/repairs/landing before task-index publication completes.
pub(super) fn require_not_pending<T: TaskRef + ?Sized>(
    store: &corral_core::Store,
    task: &T,
) -> Result<()> {
    let Some((session, scratch)) = task.cowork().as_ref().and_then(|c| c.assistant_driver()) else {
        return Ok(());
    };
    if !store.assistant_exists(session)? {
        return Ok(());
    }
    let container = store.get_assistant_container(session)?;
    container.validate_protocol()?;
    let Some(w) = container.workflow.as_ref().map(|i| &i.current) else {
        return Ok(());
    };
    if matches!(
        w.phase,
        corral_core::assistant_workflow::WorkflowPhase::HandoffPreparing
            | corral_core::assistant_workflow::WorkflowPhase::HandoffReconciling
    ) && let Some(reference) = &w.handoff
    {
        let p: Package = serde_json::from_slice(
            &Content {
                blob: reference.blob.clone(),
                bytes: reference.bytes as usize,
            }
            .load(store)?,
        )?;
        let selected_driver = if scratch.is_empty() {
            p.source_path.is_none()
        } else {
            let assistant = store.get_assistant(session)?;
            let entry = assistant
                .scratch
                .iter()
                .find(|s| s.slug == scratch)
                .context("handoff_pending: current tracking scratch identity is missing")?;
            p.source_path.as_ref() == Some(&entry.path)
        };
        ensure!(
            !selected_driver
                && task
                    .current_run_head()?
                    .is_none_or(|r| Some(&r.worktree) != p.source_path.as_ref()),
            "handoff_pending: source reservation is reconciling; no new pipeline effect"
        );
    }
    Ok(())
}
