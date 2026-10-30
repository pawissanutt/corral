//! Explicit complete receipt output stays cold; owning sync selects only indexed debt.
use super::*;

fn one(ctx: &Ctx, home: &Home, receipt: RequestReceipt) -> Result<RequestReceipt> {
    let mut receipt = match (receipt.op.as_str(), receipt.state) {
        ("gate", ReceiptState::Reserved | ReceiptState::Queued) => match &receipt.executor {
            Some(running) if !executor_gone(running)? => receipt,
            seen => crate::review_budget::settle(ctx, &receipt, seen.as_ref(), false, None)?,
        },
        ("gate", ReceiptState::Uncertain) => {
            crate::review_budget::settle(ctx, &receipt, None, false, None)?
        }
        _ => receipt,
    };
    complete(ctx, home, &mut receipt)?;
    Ok(receipt)
}

/// Resume/complete output: preserve the full public receipt vector in its original home order.
pub fn reconcile_task(ctx: &Ctx, task_id: &str) -> Result<Vec<RequestReceipt>> {
    crate::agent::ops::planning::reconcile(ctx, task_id)?;
    let task = ctx.store.get(task_id)?;
    let mut homes = Vec::new();
    for run in &task.runs {
        let (id, run_id) = (task.id.clone(), run.id.clone());
        if run.agent_host.is_some() {
            homes.push(Home::Container(ContainerRef::Worker {
                task: id.clone(),
                run: run_id.clone(),
            }));
        }
        for child in run
            .review_runs
            .iter()
            .filter(|child| child.agent_host.is_some())
        {
            homes.push(Home::Container(ContainerRef::Reviewer {
                task: id.clone(),
                run: run_id.clone(),
                review: child.id.clone(),
            }));
        }
    }
    if !task.agent_pm_receipts.is_empty() {
        homes.push(Home::PmTask(task.id.clone()));
    }
    let mut settled = Vec::new();
    for home in homes {
        for receipt in receipts(&task, &home).to_vec() {
            settled.push(one(ctx, &home, receipt)?);
        }
    }
    Ok(settled)
}

/// Owning sync does not request complete receipt history or construct compatibility homes.
pub(crate) fn reconcile_task_hot(ctx: &Ctx, task_id: &str) -> Result<()> {
    crate::agent::ops::planning::reconcile(ctx, task_id)?;
    let task = ctx.store.head(task_id)?;
    for container in task.unfinished_native_containers()? {
        if !matches!(
            container,
            ContainerRef::Worker { .. } | ContainerRef::Reviewer { .. }
        ) {
            continue;
        }
        for receipt in task.pending_agent_receipts(&container)? {
            one(ctx, &Home::Container(container.clone()), receipt)?;
        }
    }
    let home = Home::PmTask(task.id.clone());
    for receipt in task.pending_pm_receipts()? {
        one(ctx, &home, receipt)?;
    }
    Ok(())
}
