//! Existing sync owns roll-forward. The unfinished index stores only journal IDs, not
//! another authority/state store; completed history never enters this polling path.
use super::{JOURNALS, Journal, lifecycle::Disposition};
use crate::Ctx;
use anyhow::{Context, Result};
use corral_core::Store;

const WORK: &str = "assistant-handoff-work";

fn pending(journal: &Journal) -> bool {
    if journal.retreat.as_ref().is_some_and(|r| !r.complete) {
        return true;
    }
    !journal.cancelled
        && journal.selection.is_some()
        && journal.transfer.as_ref().is_none_or(|t| {
            !t.ownership_committed
                || t.notification_pending
                || matches!(
                    t.disposition,
                    Disposition::Saving | Disposition::Starting | Disposition::TakingBack
                )
                || (t.disposition == Disposition::Continuing && t.candidate_pending)
        })
}

pub(super) fn save(store: &Store, journal: &Journal) -> Result<()> {
    let mut journal = journal.clone();
    if journal.selection.is_some() || journal.retreat.is_some() {
        journal.version = 2;
    }
    let key = crate::verification::digest(journal.id.as_bytes());
    store.records_transaction(|| {
        if pending(&journal) {
            store.save_record(WORK, &key, &journal.id)?;
        } else {
            journal.waiting = None;
            store.delete_record(WORK, &key)?;
        }
        store.save_record(JOURNALS, &journal.id, &journal)
    })
}

pub(crate) fn reconcile(ctx: &Ctx) -> Result<()> {
    for row in ctx.store.record_rows(WORK)? {
        let id: String =
            serde_json::from_value(row.doc).context("decode unfinished transfer index")?;
        let Some(journal) = super::read_journal(&ctx.store, &id)? else {
            continue;
        };
        if let Err(error) = advance(ctx, &journal) {
            ctx.store.records_transaction(|| {
                let current =
                    super::read_journal(&ctx.store, &id)?.context("transfer disappeared")?;
                if pending(&current)
                    && current.transfer.as_ref().map(|t| t.revision)
                        == journal.transfer.as_ref().map(|t| t.revision)
                {
                    let mut current = current;
                    current.waiting = Some(format!("{error:#}"));
                    save(&ctx.store, &current)?;
                }
                Ok(())
            })?;
        }
    }
    Ok(())
}

fn advance(ctx: &Ctx, journal: &Journal) -> Result<()> {
    if let Some(retreat) = journal.retreat.as_ref().filter(|r| !r.complete) {
        return super::retreat::run(
            ctx,
            &retreat.choice,
            &retreat.digest,
            retreat.instruction.clone(),
        )
        .map(drop);
    }
    let selection = journal
        .selection
        .as_ref()
        .context("handoff_reconciling: explicit selection missing")?;
    let Some(transfer) = journal.transfer.as_ref().filter(|t| t.ownership_committed) else {
        return super::transfer::commit(
            ctx,
            &selection.choice,
            &selection.digest,
            selection.instruction.clone(),
        )
        .map(drop);
    };
    match transfer.disposition {
        Disposition::Starting | Disposition::Saving => {
            let _pipeline = ctx
                .store
                .try_lock(&format!("autopilot-{}", transfer.task))?
                .context("Waiting for controller")?;
            let _launch = ctx
                .store
                .try_lock(&format!("launch-{}", transfer.task))?
                .context("Waiting for launch")?;
            let _gate = ctx
                .store
                .try_lock(&format!("gate-{}", transfer.task))?
                .context("Waiting for gate admission")?;
            if transfer.disposition == Disposition::Saving {
                super::continuation::finish_save(ctx, &transfer.task)?;
            } else {
                super::continuation::finish_start(ctx, &transfer.task)?;
            }
        }
        Disposition::TakingBack => {
            let instruction = transfer
                .history
                .last()
                .context("take_back: selected instruction missing")?
                .instruction
                .clone();
            super::takeback::apply(
                ctx,
                &transfer.task,
                &transfer.contract_revision,
                instruction,
            )?;
        }
        Disposition::Continuing if transfer.candidate_pending => {
            super::adopt::advance_candidate(ctx, &transfer.task)?;
        }
        _ => {}
    }
    let fresh = super::read_journal(&ctx.store, &journal.id)?.context("transfer disappeared")?;
    if fresh
        .transfer
        .as_ref()
        .is_some_and(|t| t.notification_pending)
    {
        super::transfer::notify(ctx, &journal.id)?;
    } else {
        ctx.store.records_transaction(|| {
            let current =
                super::read_journal(&ctx.store, &journal.id)?.context("transfer disappeared")?;
            save(&ctx.store, &current)
        })?;
    }
    Ok(())
}
