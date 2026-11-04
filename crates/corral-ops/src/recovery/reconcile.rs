use super::*;

/// Background work may finalize existing effects, never create or refresh an authorization.
pub(crate) fn reconcile_pending(ctx: &Ctx, id: &str) -> Result<bool> {
    let records = records(ctx, id)?;
    if records.is_empty() {
        return Ok(false);
    }
    let task = ctx.store.head(id)?;
    for record in records {
        let pending = matches!(
            state(&record)?,
            RecoveryState::Authorized | RecoveryState::Applying | RecoveryState::Unknown
        );
        let repair = task.last_plan.as_ref().is_some_and(|plan| plan.approval.as_ref().is_some_and(|approval| approval.by == "owner" && approval.policy_digest == record.preview.contract_digest && !record.links.iter().any(|l| matches!(&l.receipt, RecoveryReceipt::PlanApproval { plan:p, at } if p == &plan.id && *at == approval.at))));
        if pending || repair {
            let result = recover_reconcile(ctx, id, &record.id)?;
            if matches!(
                state(&result)?,
                RecoveryState::Authorized | RecoveryState::Applying | RecoveryState::Unknown
            ) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}
