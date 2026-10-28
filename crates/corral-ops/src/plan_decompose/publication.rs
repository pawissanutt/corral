//! The multi-scope publication shared by the owner's decomposition (a full parent) and the
//! automatic one (the parent's head shell, plan T-253 section 4): staging, ID remapping, scope
//! bindings and the one-operation publish. Staged tasks never inherit the parent's history.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, ensure};
use corral_core::{
    PlanAcceptance, PlanDocument, Status, Store, Task, TaskRef as _, operating_contract_revision,
};

use super::{ChildScope, Journal, PUBLICATIONS, tracker_plan};

/// A staged child or tracker is a new task: the parent's scope-amendment history, aggregate
/// attempts, member gates and PM receipts stay with the parent.
pub(super) fn clear_inherited(task: &mut Task) {
    task.approved_scope_history.clear();
    task.approved_scope_revision = 0;
    task.aggregate_attempts.clear();
    task.member_gates.clear();
    task.agent_pm_receipts.clear();
}

/// The journal to publish: `previous` resumed, or children and tracker staged from `parent`;
/// collisions remapped, and every staged child bound to its scope.
pub(super) fn stage(
    store: &Store,
    parent: &Task,
    previous: Option<Journal>,
    document: PlanDocument,
    backlink: &str,
    artifact_sha256: &str,
    source_contract: String,
) -> Result<Journal> {
    let mut journal = if let Some(journal) = previous {
        journal
    } else {
        let mut children = children(store, parent, &document, backlink)?;
        let tracker =
            tracker_plan::stage(parent, &document, backlink, artifact_sha256, &mut children)?;
        Journal {
            backlink: backlink.to_owned(),
            source_contract,
            projected_contract: String::new(),
            artifact_sha256: artifact_sha256.to_owned(),
            scopes: BTreeMap::new(),
            children: children.iter().map(Store::render_task_markdown).collect(),
            tracker: Some(Store::render_task_markdown(&tracker)),
            previous_contracts: BTreeMap::new(),
            document,
        }
    };
    // An interruption can leave reserved-but-unwritten IDs available to the
    // normal allocator. Remap only those collisions before publishing again.
    tracker_plan::upgrade(&mut journal, parent)?;
    remap_collisions(store, &mut journal)?;
    // A collision changes both task IDs and dependency contracts. Bind only
    // the final rendered children, never the originally reserved numbers.
    let staged = journal
        .children
        .iter()
        .map(|text| Store::parse_task_markdown(text))
        .collect::<Result<Vec<_>>>()?;
    ensure!(
        staged.len() == journal.document.dependencies.len(),
        "decomposition journal scope count changed"
    );
    for (scope, child) in journal.document.dependencies.iter().zip(&staged) {
        ensure!(
            child.title == scope.task
                && child.write_scope == scope.write_scope
                && child.plan.as_deref() == Some(backlink),
            "decomposition journal scope identity changed"
        );
    }
    journal.scopes = journal
        .document
        .dependencies
        .iter()
        .zip(&staged)
        .map(|(scope, child)| {
            Ok((
                scope.task.clone(),
                ChildScope {
                    task: child.id.clone(),
                    scope_digest: crate::verification::digest(&serde_json::to_vec(scope)?),
                    contract_revision: operating_contract_revision(child)?,
                },
            ))
        })
        .collect::<Result<_>>()?;
    Ok(journal)
}

/// The staged children's IDs, in scope order.
pub(super) fn child_ids(journal: &Journal) -> Result<Vec<String>> {
    journal
        .children
        .iter()
        .map(|text| Ok(Store::parse_task_markdown(text)?.id))
        .collect()
}

/// Saves the journal's intent, then publishes the tracker and every child in one operation:
/// either all of them exist or none does, so normal allocation never steals a reserved ID.
pub(super) fn publish(store: &Store, parent: &str, journal: &Journal) -> Result<()> {
    PUBLICATIONS.save_intent(store, parent, journal)?;
    let mut staged = Vec::with_capacity(journal.children.len() + 1);
    if let Some(text) = &journal.tracker {
        staged.push(Store::parse_task_markdown(text)?);
    }
    for text in &journal.children {
        staged.push(Store::parse_task_markdown(text)?);
    }
    store.publish_staged(&staged)
}

fn reserved_numbers(store: &Store) -> Result<BTreeSet<u64>> {
    // list() skips malformed cards. Their filenames still reserve IDs, just as
    // they do for Store::create; never overwrite or alias an unreadable card.
    store.reserved_task_numbers()
}

fn next_number(store: &Store) -> Result<u64> {
    reserved_numbers(store)?
        .last()
        .copied()
        .unwrap_or(0)
        .checked_add(1)
        .context("task ID exhausted")
}

pub(super) fn children(
    store: &Store,
    parent: &Task,
    document: &PlanDocument,
    backlink: &str,
) -> Result<Vec<Task>> {
    let first = next_number(store)?;
    let ids: BTreeMap<_, _> = document
        .dependencies
        .iter()
        .enumerate()
        .map(|(index, scope)| {
            let number = first
                .checked_add(u64::try_from(index)?)
                .context("task ID exhausted")?;
            Ok((scope.task.as_str(), format!("T-{number:03}")))
        })
        .collect::<Result<_>>()?;
    document
        .dependencies
        .iter()
        .map(|scope| {
            let mut child = parent.clone();
            child.id = ids[scope.task.as_str()].clone();
            child.title = scope.task.clone();
            child.status = Status::Backlog;
            child.design = false;
            child.plan_required = Some(false);
            child.plan = Some(backlink.to_owned());
            child.tracker = false;
            child.approved_scope = None;
            child.source_digest = None;
            child.parent = None;
            child.children.clear();
            child.plans.clear();
            child.plan_runs.clear();
            child.runs.clear();
            child.artifacts.clear();
            child.comments.clear();
            child.completion = None;
            child.dashboard = None;
            child.rounds = 0;
            child.queued = false;
            child.dispatch_plan = None;
            child.launch = None;
            child.dispatch_hold = false;
            child.review_reservation = None;
            child.review_wait = None;
            clear_inherited(&mut child);
            child.created = corral_core::clock::now();
            child.updated = child.created;
            child.write_scope = scope.write_scope.clone();
            child.depends_on = parent.depends_on.clone();
            child.depends_on.push(parent.id.clone());
            for dependency in &scope.depends_on {
                child.depends_on.push(ids[dependency.as_str()].clone());
            }
            // Top-level acceptance has no task selector. With no scope-specific
            // scenarios, retaining all is conservative: never silently drop one.
            let acceptance = if scope.acceptance.is_empty() {
                &document.acceptance
            } else {
                &scope.acceptance
            };
            child.body = body(document, acceptance, backlink);
            if let Some(manifest) = &parent.completion {
                let mut manifest = manifest.clone();
                manifest.criteria = acceptance
                    .iter()
                    .filter_map(|a| {
                        let mut criterion = manifest
                            .criteria
                            .iter()
                            .find(|c| Some(c.id.as_str()) == a.criterion.as_deref())?
                            .clone();
                        criterion.scenario = a.scenario.clone();
                        criterion.expected = a.expected.clone();
                        Some(criterion)
                    })
                    .collect();
                ensure!(
                    manifest.criteria.len() == acceptance.len(),
                    "scope {} acceptance must reference registered criteria",
                    scope.task
                );
                manifest.spec_digest = crate::verification::spec_digest(&child)?;
                child.completion = Some(manifest);
            }
            Ok(child)
        })
        .collect()
}

fn body(document: &PlanDocument, acceptance: &[PlanAcceptance], backlink: &str) -> String {
    let mut body = format!(
        "Governed by approved plan {backlink}.\n\n{}\n",
        document.goal
    );
    for (heading, entries) in [
        ("Non-goals", &document.non_goals),
        ("Decisions", &document.decisions),
        ("Contracts", &document.contracts),
        ("Verification", &document.verification),
    ] {
        body.push_str(&format!("\n## {heading}\n"));
        for entry in entries {
            body.push_str(&format!("- {entry}\n"));
        }
    }
    body.push_str("\n## Acceptance\n");
    for a in acceptance {
        if let Some(id) = &a.criterion {
            body.push_str(&format!("\n### {id}\n"));
        }
        body.push_str(&format!(
            "\nScenario: {}\nExpected: {}\n",
            a.scenario, a.expected
        ));
    }
    body
}

pub(super) fn remap_collisions(store: &Store, journal: &mut Journal) -> Result<()> {
    let mut tasks = journal
        .children
        .iter()
        .map(|text| Store::parse_task_markdown(text))
        .collect::<Result<Vec<_>>>()?;
    if let Some(text) = &journal.tracker {
        tasks.push(Store::parse_task_markdown(text)?);
    }
    let mut next = next_number(store)?;
    for task in &tasks {
        let number = task.id.trim_start_matches("T-").parse::<u64>()?;
        next = next.max(number.checked_add(1).context("task ID exhausted")?);
    }
    let mut replacements = BTreeMap::new();
    let reserved = reserved_numbers(store)?;
    for task in &tasks {
        // Each staged ID is looked up by key: only the tasks it may collide with are read.
        if let Some(existing) = store.head_if_exists(&task.id)? {
            if (task.tracker && existing.tracker && existing.parent == task.parent)
                || existing.plan.as_ref() == Some(&journal.backlink)
            {
                let contract = existing.contract_revision()?;
                let staged_contract = operating_contract_revision(task)?;
                ensure!(
                    existing.status == Status::Backlog
                        && existing.runs_len()? == 0
                        && existing.launch.is_none()
                        && !existing.queued
                        && (contract == staged_contract
                            || journal
                                .previous_contracts
                                .get(&task.id)
                                .is_some_and(|contracts| contracts.contains(&contract))),
                    "staged child {} was modified; cannot resume decomposition",
                    task.id
                );
            } else {
                replacements.insert(task.id.clone(), format!("T-{next:03}"));
                next = next.checked_add(1).context("task ID exhausted")?;
            }
        } else if reserved.contains(&task.id.trim_start_matches("T-").parse::<u64>()?) {
            // An unreadable card is still somebody else's allocated ID.
            replacements.insert(task.id.clone(), format!("T-{next:03}"));
            next = next.checked_add(1).context("task ID exhausted")?;
        }
    }
    for task in &mut tasks {
        // The next receipt is saved before any rewritten task. If interrupted,
        // either its current or exact earlier staged contract may be on disk.
        journal
            .previous_contracts
            .entry(task.id.clone())
            .or_default()
            .insert(operating_contract_revision(task)?);
        if let Some(id) = replacements.get(&task.id) {
            task.id = id.clone();
            if let Some(manifest) = task.completion.as_mut() {
                manifest.spec_digest.clear();
            }
        }
        for dependency in &mut task.depends_on {
            if let Some(id) = replacements.get(dependency) {
                *dependency = id.clone();
            }
        }
        if let Some(id) = task.parent.as_ref().and_then(|id| replacements.get(id)) {
            task.parent = Some(id.clone());
        }
        for id in &mut task.children {
            if let Some(replacement) = replacements.get(id) {
                *id = replacement.clone();
            }
        }
        let digest = crate::verification::spec_digest(task)?;
        if let Some(manifest) = task.completion.as_mut() {
            manifest.spec_digest = digest;
        }
    }
    if journal.tracker.is_some() {
        journal.tracker = tasks.pop().as_ref().map(Store::render_task_markdown);
    }
    journal.children = tasks.iter().map(Store::render_task_markdown).collect();
    Ok(())
}
