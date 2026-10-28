//! A plan of at most one scope projected onto its own task (plan T-253 section 4, automatic
//! plan acceptance with its decomposition): the task is read and written as a head, its
//! contract revision is the head's, and nothing else of the board is read.
use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};
use corral_core::{OperatingActionKind, TaskMut as _, TaskRef as _};

use super::{ChildScope, Journal, PUBLICATIONS, check_attachment, check_scopes, validate};
use crate::Ctx;

pub(super) fn apply(
    ctx: &Ctx,
    id: &str,
    plan_id: &str,
    origin: crate::operating::Origin,
) -> Result<()> {
    let automatic = origin == crate::operating::Origin::Automatic;
    ctx.store.dispatch_update_head(id, |store, parent| {
        crate::operating::require_origin(store, &*parent, OperatingActionKind::AcceptPlan, origin)?;
        let plan = parent
            .head()
            .last_plan
            .clone()
            .filter(|plan| plan.id == plan_id)
            .context("stale plan revision: decomposition requires the latest plan")?;
        if automatic {
            check_scopes(store, &*parent, plan_id)?;
        }
        check_attachment(store, &*parent, plan_id)?;
        ensure!(plan.approval.is_some(), "plan has no final acceptance");
        let document = plan.document.clone();
        validate(&document)?;
        ensure!(
            document.dependencies.len() <= 1,
            "stale plan revision: decomposition requires the latest plan"
        );
        let backlink = format!("{}@{}", parent.id, plan.id);
        if automatic {
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
        }
        ensure!(parent.children.is_empty(), "task already has children");
        let current_contract = parent.contract_revision()?;
        let source_contract = if let Some(previous) = PUBLICATIONS.load(store, &parent.id)? {
            if previous.backlink == backlink {
                ensure!(
                    previous.document == document
                        && previous.artifact_sha256 == plan.artifact.sha256
                        && (previous.source_contract == current_contract
                            || previous.projected_contract == current_contract),
                    "self-projection identity changed"
                );
                previous.source_contract
            } else {
                ensure!(
                    parent.plan.as_ref() == Some(&previous.backlink)
                        && previous.document.dependencies.len() <= 1
                        && previous.children.is_empty(),
                    "cannot replace a published child plan"
                );
                current_contract
            }
        } else {
            current_contract
        };
        let write_scope = document
            .dependencies
            .first()
            .filter(|scope| !scope.write_scope.is_empty())
            .map(|scope| scope.write_scope.clone());
        parent.edit_head_fields(|fields| {
            fields.plan = Some(backlink.clone());
            fields.queued = false;
            fields.dispatch_plan = None;
            if let Some(write_scope) = write_scope {
                fields.write_scope = write_scope;
            }
            Ok(())
        })?;
        let projected_contract = parent.contract_revision()?;
        let scopes = document
            .dependencies
            .iter()
            .map(|scope| {
                Ok((
                    scope.task.clone(),
                    ChildScope {
                        task: parent.id.clone(),
                        scope_digest: crate::verification::digest(&serde_json::to_vec(scope)?),
                        contract_revision: projected_contract.clone(),
                    },
                ))
            })
            .collect::<Result<_>>()?;
        let journal = Journal {
            backlink,
            document,
            children: Vec::new(),
            tracker: None,
            previous_contracts: BTreeMap::new(),
            source_contract,
            projected_contract,
            artifact_sha256: plan.artifact.sha256.clone(),
            scopes,
        };
        PUBLICATIONS.save(store, &parent.id, &journal)
    })?;
    Ok(())
}
