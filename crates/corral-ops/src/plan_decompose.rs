//! Recoverable approval-to-task projection. The parent is the dispatch barrier.
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use corral_core::{PlanAcceptance, PlanDocument, Status, Store, Task, parse_task, render_task};
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
}

/// Projects a final owner/auto acceptance, never an independent review verdict.
///
/// Must be called outside any store write callback: dispatch_update owns the
/// board lock. Child writes use rendered files directly under that same lock,
/// not create/save/update (which would acquire it again).
pub(crate) fn apply(ctx: &Ctx, id: &str, plan_id: &str) -> Result<()> {
    ctx.store.dispatch_update(id, |store, parent| {
        let plan = parent
            .plans
            .last()
            .filter(|plan| plan.id == plan_id)
            .context("decomposition requires the latest plan")?;
        ensure!(plan.approval.is_some(), "plan has no final acceptance");
        let document = plan.document.clone();
        validate(&document)?;
        let backlink = format!("{}@{}", parent.id, plan.id);
        ensure!(
            parent.plan.as_ref().is_none_or(|p| p == &backlink),
            "task already belongs to another approved plan"
        );
        if document.dependencies.len() <= 1 {
            ensure!(parent.children.is_empty(), "task already has children");
            parent.plan = Some(backlink);
            parent.queued = false;
            parent.dispatch_plan = None;
            if let Some(scope) = document.dependencies.first()
                && !scope.write_scope.is_empty()
            {
                parent.write_scope = scope.write_scope.clone();
            }
            return Ok(());
        }
        if parent.plan.as_ref() == Some(&backlink)
            && parent.status == Status::Done
            && !parent.children.is_empty()
        {
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
        let mut journal = if journal_path.exists() {
            let bytes = std::fs::read(&journal_path)
                .with_context(|| format!("reading {}", journal_path.display()))?;
            let journal: Journal =
                serde_json::from_slice(&bytes).context("reading decomposition journal")?;
            ensure!(
                journal.backlink == backlink && journal.document == document,
                "unfinished decomposition belongs to a different plan"
            );
            journal
        } else {
            Journal {
                backlink: backlink.clone(),
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
                        && !existing.queued,
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
