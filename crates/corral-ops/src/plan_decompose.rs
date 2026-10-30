//! Recoverable approval-to-task projection. The parent is the dispatch barrier.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, ensure};
use corral_core::records::RecordArea;
use corral_core::{
    OperatingActionKind, PlanDocument, ScopedTask, Status, Store, Task, TaskRef,
    operating_contract_revision,
};
use serde::{Deserialize, Serialize};

use crate::Ctx;

mod publication;
mod publish_head;
mod self_projection;

#[cfg(test)]
use corral_core::PlanAcceptance;
#[cfg(test)]
use publication::remap_collisions;

#[cfg(test)]
#[path = "plan_decompose_tests.rs"]
mod tests;

#[path = "plan_tracker.rs"]
mod tracker_plan;

#[derive(Serialize, Deserialize)]
struct Journal {
    backlink: String,
    document: PlanDocument,
    /// Rendered task files retain bodies, unlike Task's serde representation.
    children: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tracker: Option<String>,
    /// Exact earlier staged contracts, retained across interrupted ID remapping.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    previous_contracts: BTreeMap<String, BTreeSet<String>>,
    /// Source and projected parent contracts distinguish this reviewed projection
    /// from arbitrary edits after publication without rewriting the scope grant.
    #[serde(default)]
    source_contract: String,
    #[serde(default)]
    projected_contract: String,
    #[serde(default)]
    artifact_sha256: String,
    #[serde(default)]
    scopes: BTreeMap<String, ChildScope>,
}

const PUBLICATIONS: RecordArea<Journal> = RecordArea::new("plan-decompose");

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChildScope {
    task: String,
    scope_digest: String,
    contract_revision: String,
}

/// Automatic acceptance cannot promise implementation scopes the owner never included.
pub(crate) fn check_scopes<T: TaskRef + ?Sized>(
    store: &Store,
    task: &T,
    plan_id: &str,
) -> Result<()> {
    let plan = task
        .last_plan_head()?
        .filter(|plan| plan.id == plan_id)
        .context("scoped plan is no longer current")?;
    if plan.document.dependencies.is_empty() {
        return Ok(());
    }
    let grant = store
        .config
        .automation
        .scope
        .plans
        .iter()
        .find(|grant| {
            grant.task == *task.id()
                && grant.revision == plan_id
                && grant.artifact_sha256 == plan.artifact.sha256
        })
        .context("plan implementation scopes require an explicit scope amendment")?;
    for scope in &plan.document.dependencies {
        let digest = crate::verification::digest(&serde_json::to_vec(scope)?);
        ensure!(
            grant.scopes.get(&scope.task) == Some(&digest),
            "plan scope {} requires an explicit scope amendment",
            scope.task
        );
    }
    Ok(())
}

/// Read the exact publication binding; core must consume the same binding when
/// resolving automatic child membership. A parent backlink alone is not consent.
pub(crate) fn scoped_child<T: TaskRef + ?Sized>(
    store: &Store,
    task: &T,
) -> Result<Option<ScopedTask>> {
    let Some((parent, _)) = task.plan().as_deref().and_then(|link| link.split_once('@')) else {
        return Ok(None);
    };
    if parent == task.id() {
        return Ok(None);
    }
    let member = store.published_operating_member(task)?;
    ensure!(
        member.is_some(),
        "child has no exact confirmed publication binding"
    );
    Ok(member)
}

/// Check replacement legality under the task lock, before final acceptance.
pub(crate) fn check_attachment<T: TaskRef + ?Sized>(
    store: &Store,
    task: &T,
    plan_id: &str,
) -> Result<()> {
    let backlink = format!("{}@{plan_id}", task.id());
    let Some(previous) = task.plan().as_ref().filter(|link| *link != &backlink) else {
        return Ok(());
    };
    let own = previous
        .strip_prefix(&format!("{}@", task.id()))
        .map(|revision| task.has_plan(revision))
        .transpose()?;
    ensure!(
        own == Some(true),
        "task already belongs to another approved plan"
    );
    ensure!(
        task.runs_len()? == 0 && task.launch().is_none(),
        "cannot replace a plan after implementation admission"
    );
    ensure!(
        task.children().is_empty(),
        "cannot replace a plan after child publication has started"
    );
    if let Some(publication) = PUBLICATIONS.load(store, task.id())? {
        if publication.backlink == backlink {
            // Publication writes the replacement journal before its children,
            // but keeps the original parent attachment until all writes finish.
            // The source contract includes that exact attachment. A self-projection's
            // projected contract takes the scope history as the head's chain.
            check_publication_identity(&publication, task, plan_id)?;
        } else {
            ensure!(
                publication.backlink == *previous
                    && publication.document.dependencies.len() <= 1
                    && publication.children.is_empty(),
                "cannot replace a plan after child publication has started"
            );
        }
    }
    ensure!(
        !matches!(task.status(), Status::Done | Status::Dropped),
        "cannot replace a terminal task's plan"
    );
    Ok(())
}

/// The unfinished publication still belongs to `parent`'s exact accepted latest plan. A
/// self-projection's projected contract is computed on the parent's shell with the head's
/// scope-history chain, so no scope revision is read.
fn check_publication_identity<T: TaskRef + ?Sized>(
    journal: &Journal,
    parent: &T,
    plan_id: &str,
) -> Result<()> {
    let plan = parent
        .last_plan_head()?
        .filter(|plan| plan.id == plan_id && plan.approval.is_some())
        .context("publication requires the exact accepted latest plan")?;
    ensure!(
        journal.backlink == format!("{}@{plan_id}", parent.id())
            && journal.document == plan.document
            && journal.artifact_sha256 == plan.artifact.sha256
            && journal.source_contract == parent.contract_revision()?,
        "unfinished decomposition publication identity changed"
    );
    if plan.document.dependencies.len() <= 1 {
        // A self-projection has no rendered children: its scope binds the
        // projected parent, while the source retains the previous attachment.
        ensure!(
            journal.children.is_empty() && journal.scopes.len() == plan.document.dependencies.len(),
            "self-projection journal scope count changed"
        );
        let mut projected = parent.shell_task()?;
        projected.plan = Some(journal.backlink.clone());
        if let Some(scope) = plan.document.dependencies.first()
            && !scope.write_scope.is_empty()
        {
            projected.write_scope = scope.write_scope.clone();
        }
        ensure!(
            journal.projected_contract
                == corral_core::operating_contract_revision_chained(
                    &projected,
                    &parent.history_summary()?.scope_history_chain
                )?,
            "self-projection journal parent identity changed"
        );
        for scope in &plan.document.dependencies {
            let binding = journal
                .scopes
                .get(&scope.task)
                .context("self-projection journal scope binding missing")?;
            ensure!(
                binding.task == *parent.id()
                    && binding.scope_digest
                        == crate::verification::digest(&serde_json::to_vec(scope)?)
                    && binding.contract_revision == journal.projected_contract,
                "self-projection journal scope identity changed"
            );
        }
        return Ok(());
    }
    ensure!(
        journal.children.len() == plan.document.dependencies.len()
            && journal.scopes.len() == journal.children.len(),
        "decomposition journal scope count changed"
    );
    tracker_plan::check(journal, parent.id())?;
    let mut ids = BTreeSet::new();
    for (scope, text) in plan.document.dependencies.iter().zip(&journal.children) {
        let child = Store::parse_task_markdown(text)?;
        let binding = journal
            .scopes
            .get(&scope.task)
            .context("decomposition journal scope binding missing")?;
        ensure!(
            ids.insert(child.id.clone())
                && child.title == scope.task
                && child.write_scope == scope.write_scope
                && child.plan.as_ref() == Some(&journal.backlink)
                && binding.task == child.id
                && binding.scope_digest == crate::verification::digest(&serde_json::to_vec(scope)?)
                && binding.contract_revision == operating_contract_revision(&child)?,
            "decomposition journal child identity changed"
        );
    }
    Ok(())
}

/// Projects a final owner/auto acceptance, never an independent review verdict.
///
/// Must be called outside any store write callback: dispatch_update owns the
/// board lock. Child writes use rendered files directly under that same lock,
/// not create/save/update (which would acquire it again).
pub(crate) fn apply(ctx: &Ctx, id: &str, plan_id: &str) -> Result<()> {
    apply_refreshing(ctx, id, plan_id).map(drop)
}

/// Like `apply`, returning the already published children whose approval binding
/// this re-acceptance refreshed (empty for a first publication).
pub(crate) fn apply_refreshing(ctx: &Ctx, id: &str, plan_id: &str) -> Result<Vec<String>> {
    apply_with_origin(ctx, id, plan_id, crate::operating::Origin::Owner)
}

pub(crate) fn apply_auto(ctx: &Ctx, id: &str, plan_id: &str) -> Result<()> {
    apply_with_origin(ctx, id, plan_id, crate::operating::Origin::Automatic).map(drop)
}

fn apply_with_origin(
    ctx: &Ctx,
    id: &str,
    plan_id: &str,
    origin: crate::operating::Origin,
) -> Result<Vec<String>> {
    // A plan of at most one scope projects onto its own task, and an automatic multi-scope
    // publication stages its children from the parent's head shell: both read and write the
    // parent as a head. Only the owner's decomposition reads the full parent.
    let scopes = ctx
        .store
        .head(id)?
        .last_plan
        .as_ref()
        .filter(|plan| plan.id == plan_id)
        .map(|plan| plan.document.dependencies.len());
    if scopes.is_some_and(|n| n <= 1) {
        return self_projection::apply(ctx, id, plan_id, origin).map(|()| Vec::new());
    }
    if scopes.is_some() && origin == crate::operating::Origin::Automatic {
        return publish_head::apply(ctx, id, plan_id).map(|()| Vec::new());
    }
    let (_, refreshed) = ctx.store.dispatch_update(id, |store, parent| {
        crate::operating::require_origin(store, parent, OperatingActionKind::AcceptPlan, origin)?;
        ensure!(
            parent.plans.last().is_some_and(|plan| plan.id == plan_id),
            "stale plan revision: decomposition requires the latest plan"
        );
        if origin == crate::operating::Origin::Automatic {
            check_scopes(store, parent, plan_id)?;
        }
        check_attachment(store, parent, plan_id)?;
        let plan = parent
            .plans
            .last()
            .filter(|plan| plan.id == plan_id)
            .context("decomposition requires the latest plan")?;
        ensure!(plan.approval.is_some(), "plan has no final acceptance");
        let document = plan.document.clone();
        validate(&document)?;
        let backlink = format!("{}@{}", parent.id, plan.id);
        if origin == crate::operating::Origin::Automatic {
            let mut attached = parent.clone();
            attached.plan = Some(backlink.clone());
            corral_core::plan_policy::validate_approval(store, &store.config, &attached, plan)?;
            ensure!(
                plan.approval
                    .as_ref()
                    .is_some_and(|approval| approval.by != "pm"),
                "coordinating PM cannot supply automatic plan acceptance"
            );
        }
        if document.dependencies.len() <= 1 {
            ensure!(parent.children.is_empty(), "task already has children");
            let current_contract = operating_contract_revision(parent)?;
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
            let mut projected = parent.clone();
            projected.plan = Some(backlink.clone());
            projected.queued = false;
            projected.dispatch_plan = None;
            if let Some(scope) = document.dependencies.first()
                && !scope.write_scope.is_empty()
            {
                projected.write_scope = scope.write_scope.clone();
            }
            let projected_contract = operating_contract_revision(&projected)?;
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
            PUBLICATIONS.save(store, &parent.id, &journal)?;
            *parent = projected;
            return Ok(Vec::new());
        }
        if parent.plan.as_ref() == Some(&backlink)
            && parent.status == Status::Done
            && !parent.children.is_empty()
        {
            if origin == crate::operating::Origin::Automatic {
                for child in &parent.children {
                    scoped_child(store, &store.get(child)?)?
                        .context("published child authorization is missing")?;
                }
            }
            // Children read the parent's current approval, so a re-accepted
            // plan needs no child rewrite: report who is bound by it again.
            return Ok(parent.children.clone());
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
            parent.runs.is_empty() && parent.launch.is_none(),
            "cannot decompose a task with implementation runs or a launch"
        );
        // Existing prerequisites are inherited by every child. Validate their
        // entire reachable graph, not just names directly listed by the parent.
        let _waiting = crate::queue::dependency_check(store, parent)?;
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
                check_publication_identity(&journal, parent, plan_id)?;
                Some(journal)
            }
        } else {
            None
        };
        let source = operating_contract_revision(parent)?;
        let mut journal = publication::stage(
            store,
            parent,
            previous,
            document,
            &backlink,
            &plan.artifact.sha256,
            source,
        )?;
        let ids = publication::child_ids(&journal)?;
        let mut projected = parent.clone();
        projected.plan = Some(backlink.clone());
        projected.children = ids.clone();
        projected.status = Status::Done;
        projected.queued = false;
        projected.dispatch_plan = None;
        projected.dispatch_hold = false;
        journal.projected_contract = operating_contract_revision(&projected)?;
        publication::publish(store, &parent.id, &journal)?;
        // Every published child depends on this parent. If any earlier write
        // failed or the process died, none can dispatch, even explicitly.
        // dispatch_update saves this final parent atomically before unlocking.
        parent.plan = Some(backlink);
        parent.children = ids;
        parent.status = Status::Done;
        parent.queued = false;
        parent.dispatch_plan = None;
        parent.dispatch_hold = false;
        store.log_event(
            Some(&parent.id),
            "plan_decomposed",
            serde_json::json!({
                "target":"plan", "plan":plan_id, "children":parent.children,
            }),
        )?;
        Ok(Vec::new())
    })?;
    Ok(refreshed)
}

pub(crate) fn validate(document: &PlanDocument) -> Result<()> {
    if let Some(target) = &document.delivery_target {
        crate::tracker::validate_target(target)?;
    }
    let mut names = BTreeSet::new();
    for scope in &document.dependencies {
        ensure!(!scope.task.trim().is_empty(), "blank dependency task name");
        ensure!(names.insert(&scope.task), "duplicate task {}", scope.task);
        ensure!(
            scope.write_scope.iter().all(|p| !p.trim().is_empty()),
            "blank write scope for {}",
            scope.task
        );
        let mut criteria = BTreeSet::new();
        for acceptance in &scope.acceptance {
            ensure!(
                !acceptance.scenario.trim().is_empty() && !acceptance.expected.trim().is_empty(),
                "blank acceptance for {}",
                scope.task
            );
            if let Some(id) = &acceptance.criterion {
                ensure!(
                    !id.trim().is_empty() && criteria.insert(id),
                    "blank or duplicate criterion for {}",
                    scope.task
                );
            }
        }
    }
    for scope in &document.dependencies {
        let mut dependencies = BTreeSet::new();
        for dependency in &scope.depends_on {
            ensure!(
                names.contains(dependency),
                "missing dependency {dependency} for {}",
                scope.task
            );
            ensure!(
                dependencies.insert(dependency),
                "duplicate dependency {dependency} for {}",
                scope.task
            );
        }
    }
    // Kahn's algorithm validates arbitrary depth without recursive stack growth.
    let mut remaining: BTreeMap<_, _> = document
        .dependencies
        .iter()
        .map(|s| {
            (
                s.task.as_str(),
                s.depends_on.iter().collect::<BTreeSet<_>>(),
            )
        })
        .collect();
    while !remaining.is_empty() {
        let ready: Vec<_> = remaining
            .iter()
            .filter(|(_, deps)| deps.is_empty())
            .map(|(name, _)| *name)
            .collect();
        ensure!(!ready.is_empty(), "cycle in plan dependencies");
        for name in ready {
            remaining.remove(name);
            for deps in remaining.values_mut() {
                deps.retain(|dep| dep.as_str() != name);
            }
        }
    }
    Ok(())
}
