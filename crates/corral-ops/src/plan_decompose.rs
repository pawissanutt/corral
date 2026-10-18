//! Recoverable approval-to-task projection. The parent is the dispatch barrier.
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use corral_core::{
    OperatingActionKind, PlanAcceptance, PlanDocument, ScopedTask, Status, Store, Task,
    operating_contract_revision, parse_task, render_task,
};
use serde::{Deserialize, Serialize};

use crate::Ctx;

#[cfg(test)]
#[path = "plan_decompose_tests.rs"]
mod tests;

#[derive(Serialize, Deserialize)]
struct Journal {
    backlink: String,
    document: PlanDocument,
    /// Rendered task files retain bodies, unlike Task's serde representation.
    children: Vec<String>,
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

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChildScope {
    task: String,
    scope_digest: String,
    contract_revision: String,
}

/// Automatic acceptance cannot promise implementation scopes the owner never included.
pub(crate) fn check_scopes(store: &Store, task: &Task, plan_id: &str) -> Result<()> {
    let plan = task
        .plans
        .last()
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
            grant.task == task.id
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
pub(crate) fn scoped_child(store: &Store, task: &Task) -> Result<Option<ScopedTask>> {
    let Some((parent, _)) = task.plan.as_deref().and_then(|link| link.split_once('@')) else {
        return Ok(None);
    };
    if parent == task.id {
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
pub(crate) fn check_attachment(store: &Store, task: &Task, plan_id: &str) -> Result<()> {
    let backlink = format!("{}@{plan_id}", task.id);
    let Some(previous) = task.plan.as_ref().filter(|link| *link != &backlink) else {
        return Ok(());
    };
    ensure!(
        task.plans
            .iter()
            .any(|plan| previous == &format!("{}@{}", task.id, plan.id)),
        "task already belongs to another approved plan"
    );
    ensure!(
        task.runs.is_empty() && task.launch.is_none(),
        "cannot replace a plan after implementation admission"
    );
    ensure!(
        task.children.is_empty(),
        "cannot replace a plan after child publication has started"
    );
    let journal = store
        .state_dir()
        .join(format!("plan-decompose-{}.json", task.id));
    if journal.exists() {
        let publication: Journal = serde_json::from_slice(
            &std::fs::read(&journal).context("reading previous plan publication")?,
        )
        .context("parsing previous plan publication")?;
        if publication.backlink == backlink {
            // Publication writes the replacement journal before its children,
            // but keeps the original parent attachment until all writes finish.
            // The source contract includes that exact attachment.
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
        !matches!(task.status, Status::Done | Status::Dropped),
        "cannot replace a terminal task's plan"
    );
    Ok(())
}

fn check_publication_identity(journal: &Journal, parent: &Task, plan_id: &str) -> Result<()> {
    let plan = parent
        .plans
        .last()
        .filter(|plan| plan.id == plan_id && plan.approval.is_some())
        .context("publication requires the exact accepted latest plan")?;
    ensure!(
        journal.backlink == format!("{}@{plan_id}", parent.id)
            && journal.document == plan.document
            && journal.artifact_sha256 == plan.artifact.sha256
            && journal.source_contract == operating_contract_revision(parent)?,
        "unfinished decomposition publication identity changed"
    );
    if plan.document.dependencies.len() <= 1 {
        // A self-projection has no rendered children: its scope binds the
        // projected parent, while the source retains the previous attachment.
        ensure!(
            journal.children.is_empty() && journal.scopes.len() == plan.document.dependencies.len(),
            "self-projection journal scope count changed"
        );
        let mut projected = parent.clone();
        projected.plan = Some(journal.backlink.clone());
        if let Some(scope) = plan.document.dependencies.first()
            && !scope.write_scope.is_empty()
        {
            projected.write_scope = scope.write_scope.clone();
        }
        ensure!(
            journal.projected_contract == operating_contract_revision(&projected)?,
            "self-projection journal parent identity changed"
        );
        for scope in &plan.document.dependencies {
            let binding = journal
                .scopes
                .get(&scope.task)
                .context("self-projection journal scope binding missing")?;
            ensure!(
                binding.task == parent.id
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
    let mut ids = BTreeSet::new();
    for (scope, text) in plan.document.dependencies.iter().zip(&journal.children) {
        let child = parse_task(text, Path::new(""))?;
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
    apply_with_origin(ctx, id, plan_id, crate::operating::Origin::Owner)
}

pub(crate) fn apply_auto(ctx: &Ctx, id: &str, plan_id: &str) -> Result<()> {
    apply_with_origin(ctx, id, plan_id, crate::operating::Origin::Automatic)
}

fn apply_with_origin(
    ctx: &Ctx,
    id: &str,
    plan_id: &str,
    origin: crate::operating::Origin,
) -> Result<()> {
    ctx.store.dispatch_update(id, |store, parent| {
        crate::operating::require_origin(store, parent, OperatingActionKind::AcceptPlan, origin)?;
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
            let journal_path = store
                .state_dir()
                .join(format!("plan-decompose-{}.json", parent.id));
            let current_contract = operating_contract_revision(parent)?;
            let source_contract = if journal_path.exists() {
                let previous: Journal = serde_json::from_slice(
                    &std::fs::read(&journal_path).context("reading self-projection journal")?,
                )
                .context("parsing self-projection journal")?;
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
                source_contract,
                projected_contract,
                artifact_sha256: plan.artifact.sha256.clone(),
                scopes,
            };
            write_atomic(&journal_path, &serde_json::to_vec(&journal)?)?;
            *parent = projected;
            return Ok(());
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
            parent.runs.is_empty() && parent.launch.is_none(),
            "cannot decompose a task with implementation runs or a launch"
        );
        let board = store.list()?;
        // Existing prerequisites are inherited by every child. Validate their
        // entire reachable graph, not just names directly listed by the parent.
        let _waiting = crate::queue::dependency_check(store, parent)?;
        let journal_path = store
            .state_dir()
            .join(format!("plan-decompose-{}.json", parent.id));
        let previous = if journal_path.exists() {
            let bytes = std::fs::read(&journal_path)
                .with_context(|| format!("reading {}", journal_path.display()))?;
            let journal: Journal =
                serde_json::from_slice(&bytes).context("reading decomposition journal")?;
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
        let mut journal = if let Some(journal) = previous {
            journal
        } else {
            Journal {
                backlink: backlink.clone(),
                source_contract: operating_contract_revision(parent)?,
                projected_contract: String::new(),
                artifact_sha256: plan.artifact.sha256.clone(),
                scopes: BTreeMap::new(),
                children: children(store, parent, &document, &backlink)?
                    .iter()
                    .map(render_task)
                    .collect(),
                document,
            }
        };
        // An interruption can leave reserved-but-unwritten IDs available to the
        // normal allocator. Remap only those collisions before publishing again.
        remap_collisions(store, &board, &mut journal)?;
        // A collision changes both task IDs and dependency contracts. Bind only
        // the final rendered children, never the originally reserved numbers.
        let staged = journal
            .children
            .iter()
            .map(|text| parse_task(text, Path::new("")))
            .collect::<Result<Vec<_>>>()?;
        ensure!(
            staged.len() == journal.document.dependencies.len(),
            "decomposition journal scope count changed"
        );
        for (scope, child) in journal.document.dependencies.iter().zip(&staged) {
            ensure!(
                child.title == scope.task
                    && child.write_scope == scope.write_scope
                    && child.plan.as_ref() == Some(&backlink),
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
        let mut projected = parent.clone();
        projected.plan = Some(backlink.clone());
        projected.children = staged.iter().map(|task| task.id.clone()).collect();
        projected.status = Status::Done;
        projected.queued = false;
        projected.dispatch_plan = None;
        projected.dispatch_hold = false;
        journal.projected_contract = operating_contract_revision(&projected)?;
        write_atomic(&journal_path, &serde_json::to_vec(&journal)?)?;
        let mut ids = Vec::with_capacity(journal.children.len());
        for text in &journal.children {
            let task = parse_task(text, Path::new(""))?;
            let path = child_path(store, &task.id);
            write_atomic(&path, text.as_bytes())?;
            ids.push(task.id);
        }
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
        Ok(())
    })?;
    Ok(())
}

pub(crate) fn validate(document: &PlanDocument) -> Result<()> {
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

fn reserved_numbers(store: &Store) -> Result<BTreeSet<u64>> {
    // list() skips malformed cards. Their filenames still reserve IDs, just as
    // they do for Store::create; never overwrite or alias an unreadable card.
    let mut numbers = BTreeSet::new();
    let directory = store.dir().join("tasks");
    for entry in
        std::fs::read_dir(&directory).with_context(|| format!("reading {}", directory.display()))?
    {
        let entry = entry.context("reading task directory entry")?;
        if entry
            .file_type()
            .context("reading task file type")?
            .is_dir()
        {
            continue;
        }
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "md") {
            continue;
        }
        let Some(rest) = path
            .file_stem()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix("T-"))
        else {
            continue;
        };
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty()
            && (rest.len() == digits.len() || rest[digits.len()..].starts_with('-'))
        {
            numbers.insert(digits.parse::<u64>().context("invalid task filename ID")?);
        }
    }
    Ok(numbers)
}

fn next_number(store: &Store) -> Result<u64> {
    reserved_numbers(store)?
        .last()
        .copied()
        .unwrap_or(0)
        .checked_add(1)
        .context("task ID exhausted")
}

fn children(
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
            child.path = child_path(store, &child.id);
            child.status = Status::Backlog;
            child.design = false;
            child.plan_required = Some(false);
            child.plan = Some(backlink.to_owned());
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
            child.created = jiff::Timestamp::now();
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

fn child_path(store: &Store, id: &str) -> PathBuf {
    store.dir().join("tasks").join(format!("{id}-plan.md"))
}

fn remap_collisions(store: &Store, board: &[Task], journal: &mut Journal) -> Result<()> {
    let mut tasks = journal
        .children
        .iter()
        .map(|text| parse_task(text, Path::new("")))
        .collect::<Result<Vec<_>>>()?;
    let mut next = next_number(store)?;
    for task in &tasks {
        let number = task.id.trim_start_matches("T-").parse::<u64>()?;
        next = next.max(number.checked_add(1).context("task ID exhausted")?);
    }
    let mut replacements = BTreeMap::new();
    let reserved = reserved_numbers(store)?;
    for task in &tasks {
        if let Some(existing) = board.iter().find(|t| t.id == task.id) {
            if existing.path == child_path(store, &task.id)
                && existing.plan.as_ref() == Some(&journal.backlink)
            {
                ensure!(
                    existing.status == Status::Backlog
                        && existing.runs.is_empty()
                        && existing.launch.is_none()
                        && !existing.queued
                        && operating_contract_revision(existing)?
                            == operating_contract_revision(task)?,
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
        let digest = crate::verification::spec_digest(task)?;
        if let Some(manifest) = task.completion.as_mut() {
            manifest.spec_digest = digest;
        }
    }
    journal.children = tasks.iter().map(render_task).collect();
    Ok(())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let directory = path.parent().context("decomposition path has no parent")?;
    let mut file = tempfile::NamedTempFile::new_in(directory)
        .with_context(|| format!("staging {}", path.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("writing {}", path.display()))?;
    file.persist(path)
        .with_context(|| format!("publishing {}", path.display()))?;
    Ok(())
}
