//! Automatic multi-scope publication (plan T-253 section 4, automatic plan acceptance with its
//! decomposition): the parent is read and written as a head, its children and tracker are
//! staged from its head shell, its contract revisions are the head's, and published children
//! are read as heads. The owner's decomposition keeps the full-parent form.
use anyhow::{Context, Result, ensure};
use corral_core::{OperatingActionKind, Status, TaskMut as _, TaskRef as _};

use super::{
    PUBLICATIONS, check_attachment, check_publication_identity, check_scopes, publication,
    scoped_child, validate,
};
use crate::Ctx;

pub(super) fn apply(ctx: &Ctx, id: &str, plan_id: &str) -> Result<()> {
    let origin = crate::operating::Origin::Automatic;
    ctx.store.dispatch_update_head(id, |store, parent| {
        crate::operating::require_origin(store, &*parent, OperatingActionKind::AcceptPlan, origin)?;
        let plan = parent
            .head()
            .last_plan
            .clone()
            .filter(|plan| plan.id == plan_id)
            .context("stale plan revision: decomposition requires the latest plan")?;
        check_scopes(store, &*parent, plan_id)?;
        check_attachment(store, &*parent, plan_id)?;
        ensure!(plan.approval.is_some(), "plan has no final acceptance");
        let document = plan.document.clone();
        validate(&document)?;
        ensure!(
            document.dependencies.len() > 1,
            "stale plan revision: decomposition requires the latest plan"
        );
        let backlink = format!("{}@{}", parent.id, plan.id);
        let mut attached = corral_core::plan_policy::approval_shell(&*parent)?;
        attached.plan = Some(backlink.clone());
        let record = attached.plans.last().context("plan missing")?.clone();
        corral_core::plan_policy::validate_approval(store, &store.config, &attached, &record)?;
        ensure!(
            plan.approval
                .as_ref()
                .is_some_and(|approval| approval.by != "pm"),
            "coordinating PM cannot supply automatic plan acceptance"
        );
        if parent.plan.as_ref() == Some(&backlink)
            && parent.status == Status::Done
            && !parent.children.is_empty()
        {
            for child in &parent.children {
                scoped_child(store, &store.head(child)?)?
                    .context("published child authorization is missing")?;
            }
            return Ok(());
        }
        ensure!(
            parent.children.is_empty(),
            "task already has a decomposition"
        );
        ensure!(
            !matches!(parent.status, Status::Done | Status::Dropped),
            "cannot decompose a terminal task"
        );
        ensure!(
            parent.runs_len()? == 0 && parent.launch.is_none(),
            "cannot decompose a task with implementation runs or a launch"
        );
        let shell = parent.shell_task()?;
        // Existing prerequisites are inherited by every child. Validate their
        // entire reachable graph, not just names directly listed by the parent.
        let _waiting = crate::queue::dependency_check(store, &shell)?;
        let previous = if let Some(journal) = PUBLICATIONS.load(store, &parent.id)? {
            if journal.backlink != backlink && parent.plan.as_ref() == Some(&journal.backlink) {
                // check_attachment already verified replacement legality. The
                // old backlink proves that this self-projection completed.
                None
            } else {
                ensure!(
                    journal.backlink == backlink && journal.document == document,
                    "unfinished decomposition belongs to a different plan"
                );
                check_publication_identity(&journal, &*parent, plan_id)?;
                Some(journal)
            }
        } else {
            None
        };
        let source = parent.contract_revision()?;
        let mut journal = publication::stage(
            store,
            &shell,
            previous,
            document,
            &backlink,
            &plan.artifact.sha256,
            source,
        )?;
        let ids = publication::child_ids(&journal)?;
        // Every published child depends on this parent. The head write commits after the
        // publication, so if any earlier write failed or the process died, none can dispatch.
        parent.edit_head_fields(|fields| {
            fields.plan = Some(backlink.clone());
            fields.children = ids.clone();
            fields.status = Status::Done;
            fields.queued = false;
            fields.dispatch_plan = None;
            fields.dispatch_hold = false;
            Ok(())
        })?;
        journal.projected_contract = parent.contract_revision()?;
        publication::publish(store, &parent.id, &journal)?;
        store.log_event(
            Some(&parent.id),
            "plan_decomposed",
            serde_json::json!({
                "target":"plan", "plan":plan_id, "children":ids,
            }),
        )?;
        Ok(())
    })?;
    Ok(())
}
