//! Trackers aggregate child completion; they never launch agent work.
use anyhow::{Context, Result, bail, ensure};
use corral_core::{ApprovedScope, PlanAcceptance, PlanDeliveryTarget, Status, Store, Task};
use serde::Serialize;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, OnceLock};

use corral_core::backend::{ChangeScope, ChangeSeq, FeedError, HeadQuery};

use crate::{Change, Ctx};

#[path = "tracker_target.rs"]
mod target;
pub use target::PlanPrerequisite;
pub(crate) use target::{check_dispatch_base, target_blocker};

/// Mutually exclusive work buckets; acceptance does not imply target inclusion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanWorkState {
    Accepted,
    Running,
    Review,
    Waiting,
    Missing,
}

/// Counts always retain every required member, including dropped work.
#[derive(Debug, Clone, Default, Serialize)]
pub struct PlanWorkCounts {
    pub accepted: usize,
    pub running: usize,
    pub review: usize,
    pub waiting: usize,
    pub missing: usize,
}

/// One normal task and its current readiness, not a separate scheduler entry.
#[derive(Debug, Clone, Serialize)]
pub struct PlanWork {
    pub task: String,
    pub title: String,
    pub status: Status,
    pub state: PlanWorkState,
    pub run: Option<String>,
    pub reason: Option<String>,
    pub prerequisites: Vec<PlanPrerequisite>,
}

/// Mapping is coverage only, never evidence that an aggregate scenario passed.
#[derive(Debug, Clone, Serialize)]
pub struct PlanCoverage {
    pub acceptance: PlanAcceptance,
    pub tasks: Vec<String>,
}

/// Shared, read-only tracker projection for CLI and board consumers.
#[derive(Debug, Serialize)]
pub struct PlanProgress {
    pub task: String,
    pub status: Status,
    pub approved_scope: Option<ApprovedScope>,
    pub scope_revision: u64,
    pub counts: PlanWorkCounts,
    pub work: Vec<PlanWork>,
    pub coverage: Vec<PlanCoverage>,
    pub unmapped_criteria: Vec<PlanAcceptance>,
    /// Present when acceptance criteria require aggregate acceptance beyond child completion.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aggregate: Option<crate::AggregateProgress>,
}

#[derive(Clone)]
struct Projection {
    tracker: Task,
    counts: PlanWorkCounts,
    work: Vec<PlanWork>,
    coverage: Vec<PlanCoverage>,
    unmapped_criteria: Vec<PlanAcceptance>,
    scopes: BTreeSet<String>,
    qualification: Vec<(usize, Task)>,
}

struct Derived {
    seq: ChangeSeq,
    projection: Projection,
}

/// Process-local values contain only the tracker and its reachable work, not board history.
/// Feed reads use the caller's pinned view, including when it predates the cached cursor.
fn projection(ctx: &Ctx, id: &str) -> Result<(Projection, bool)> {
    type Key = (std::path::PathBuf, String);
    static DERIVED: OnceLock<Mutex<BTreeMap<Key, Derived>>> = OnceLock::new();
    let mut cache = DERIVED
        .get_or_init(Mutex::default)
        .lock()
        .map_err(|error| anyhow::anyhow!("locking tracker progress: {error}"))?;
    let view = ctx.store.view()?;
    view.ensure_fresh()?;
    let seq = view.seq();
    let key = (ctx.store.cache_root()?, Store::normalize_id(id)?);
    if let Some(previous) = cache.get_mut(&key)
        && previous.seq <= seq
    {
        let mut cursor = previous.seq;
        let mut changed = false;
        while cursor < seq {
            let page = match view.changes(cursor, 256) {
                Ok(page) => page,
                Err(FeedError::Gap { .. }) => {
                    changed = true;
                    break;
                }
                Err(error) => return Err(error.into()),
            };
            let mut unknown = BTreeSet::new();
            for entry in &page.entries {
                for scope in &entry.scopes {
                    if let ChangeScope::Task { num, .. } = scope {
                        let id = format!("T-{:03}", num.0);
                        changed |= previous.projection.scopes.contains(&id);
                        unknown.insert(id);
                    }
                }
            }
            if !changed && !unknown.is_empty() {
                // Membership also depends on absent incoming edges: another owner listing
                // a member/Plan, or an unlisted child naming a known Plan as parent.
                // Read only tasks changed since this cursor, on the same pinned view.
                // Only successful projections are cached, so a removed unknown edge
                // cannot leave a cached refusal behind.
                changed = ctx
                    .store
                    .heads(&HeadQuery::Ids(unknown.into_iter().collect()))?
                    .iter()
                    .any(|task| {
                        task.parent
                            .iter()
                            .chain(&task.children)
                            .any(|id| previous.projection.scopes.contains(id))
                    });
            }
            if changed || !page.more {
                break;
            }
            cursor = page
                .entries
                .last()
                .context("empty tracker change page")?
                .seq;
        }
        if !changed {
            previous.seq = seq;
            return Ok((previous.projection.clone(), true));
        }
    }
    // A missing cache, feed gap or older snapshot recovers from the complete head computation.
    let projection = compute(ctx, id)?;
    cache.insert(
        key,
        Derived {
            seq,
            projection: projection.clone(),
        },
    );
    Ok((projection, false))
}

/// Read one coherent board snapshot without reconciling or accepting the Plan.
///
/// # Errors
/// Rejects non-trackers, missing members or dependencies, cycles, and invalid membership.
pub fn plan_progress(ctx: &Ctx, id: &str) -> Result<PlanProgress> {
    let _board = ctx
        .store
        .try_lock("dispatch")?
        .context("board is being updated; retry Plan progress")?;
    let store = if ctx.store.is_snapshot() {
        ctx.store.clone()
    } else {
        // Live callers historically refresh config as well as task data.
        Store::open_unobserved(ctx.store.root())?.read_only_snapshot()?
    };
    let fresh = Ctx {
        store,
        ..ctx.clone()
    };
    let (mut projection, cached) = projection(&fresh, id)?;
    if cached {
        // Git ancestry, target journals and resource admission are live inputs, not task
        // scopes. Refresh only qualification, retaining the task/coverage derivation.
        for (index, child) in &projection.qualification {
            let prerequisites = target::prerequisites(&fresh.store, &projection.tracker, child)?;
            let (state, reason) = work_state(&fresh, child, &prerequisites)?;
            let work = &mut projection.work[*index];
            *work_count(&mut projection.counts, work.state) -= 1;
            *work_count(&mut projection.counts, state) += 1;
            work.prerequisites = prerequisites;
            work.state = state;
            work.reason = reason;
        }
    }
    let Projection {
        tracker,
        counts,
        work,
        coverage,
        unmapped_criteria,
        ..
    } = projection;
    // Readiness reads maintained artifact facts, Git refs and checkpoint records, independent
    // of task scopes. Do not memoize acceptance readiness on the task-only feed cursor.
    let aggregate = crate::plan_acceptance::progress(&fresh, &tracker);
    Ok(PlanProgress {
        task: tracker.id,
        status: tracker.status,
        approved_scope: tracker.approved_scope,
        scope_revision: tracker.approved_scope_revision,
        counts,
        work,
        coverage,
        unmapped_criteria,
        aggregate,
    })
}

fn compute(fresh: &Ctx, id: &str) -> Result<Projection> {
    let store = &fresh.store;
    let tracker = store.head(id)?.to_shell_task()?;
    ensure!(tracker.tracker, "{} is not a tracker Plan", tracker.id);
    let mut scopes = BTreeSet::from([tracker.id.clone()]);
    scopes.extend(tracker.parent.iter().cloned());
    if let Some((design, _)) = tracker
        .approved_scope
        .as_ref()
        .and_then(|scope| scope.approved_design.as_deref())
        .and_then(|link| link.split_once('@'))
    {
        scopes.insert(design.into());
    }
    let mut work = Vec::new();
    let mut qualification = Vec::new();
    let mut counts = PlanWorkCounts::default();
    let mut coverage: Vec<_> = tracker
        .approved_scope
        .iter()
        .flat_map(|scope| &scope.acceptance)
        .map(|acceptance| PlanCoverage {
            acceptance: acceptance.clone(),
            tasks: Vec::new(),
        })
        .collect();
    let mut seen = std::collections::BTreeSet::new();
    for id in &tracker.children {
        ensure!(seen.insert(id), "duplicate Plan member {id}");
        let child_head = store
            .head(id)
            .with_context(|| format!("{}: missing Plan member {id}", tracker.id))?;
        let child = child_head.to_shell_task()?;
        if tracker.approved_scope.is_some() {
            ensure!(
                store
                    .execution_plan_head(&child_head)?
                    .as_ref()
                    .map(|plan| &plan.id)
                    == Some(&tracker.id),
                "{id}: execution Plan membership changed"
            );
        }
        // Validate the complete reachable graph even for accepted or dropped work.
        crate::queue::dependency_check(store, &child)?;
        let prerequisites = target::prerequisites(store, &tracker, &child)?;
        let (state, reason) = work_state(fresh, &child, &prerequisites)?;
        *work_count(&mut counts, state) += 1;
        for mapping in &mut coverage {
            if maps_acceptance(store, &tracker, &child, &mapping.acceptance)? {
                mapping.tasks.push(child.id.clone());
            }
        }
        if !child.depends_on.is_empty()
            || matches!(child.status, Status::Idea | Status::Backlog | Status::Ready)
        {
            qualification.push((work.len(), child.clone()));
        }
        let run = child.current_run().map(|run| run.id.clone());
        work.push(PlanWork {
            task: child.id,
            title: child.title,
            status: child.status,
            state,
            run,
            reason,
            prerequisites,
        });
    }
    let unmapped_criteria = coverage
        .iter()
        .filter(|mapping| mapping.tasks.is_empty())
        .map(|mapping| mapping.acceptance.clone())
        .collect();
    let mut pending = tracker.children.clone();
    let mut visited = BTreeSet::new();
    while let Some(id) = pending.pop() {
        if visited.insert(id.clone()) {
            let head = store.head(&id)?;
            scopes.insert(id);
            scopes.extend(head.parent.iter().cloned());
            pending.extend(head.depends_on.iter().cloned());
        }
    }
    Ok(Projection {
        tracker,
        counts,
        work,
        coverage,
        unmapped_criteria,
        scopes,
        qualification,
    })
}

fn work_count(counts: &mut PlanWorkCounts, state: PlanWorkState) -> &mut usize {
    match state {
        PlanWorkState::Accepted => &mut counts.accepted,
        PlanWorkState::Running => &mut counts.running,
        PlanWorkState::Review => &mut counts.review,
        PlanWorkState::Waiting => &mut counts.waiting,
        PlanWorkState::Missing => &mut counts.missing,
    }
}

fn work_state(
    ctx: &Ctx,
    child: &Task,
    prerequisites: &[PlanPrerequisite],
) -> Result<(PlanWorkState, Option<String>)> {
    Ok(match child.status {
        Status::Done => (PlanWorkState::Accepted, None),
        Status::Running => (PlanWorkState::Running, None),
        Status::Review => (PlanWorkState::Review, None),
        Status::Dropped => (
            PlanWorkState::Missing,
            Some("required work was dropped; approved scope is unchanged".into()),
        ),
        Status::NeedsMe => (PlanWorkState::Waiting, Some("owner input required".into())),
        Status::Idea | Status::Backlog | Status::Ready => {
            match crate::operating::queued_origin(&ctx.store, child) {
                Err(error) => (PlanWorkState::Missing, Some(format!("{error:#}"))),
                Ok(_) => {
                    let reason = prerequisites
                        .iter()
                        .find(|dependency| !dependency.satisfied)
                        .and_then(|dependency| dependency.reason.clone())
                        .or(crate::queue::blocked_reason(ctx, &ctx.store, child, false)?);
                    (
                        PlanWorkState::Waiting,
                        Some(reason.unwrap_or_else(|| "ready for authorized dispatch".into())),
                    )
                }
            }
        }
    })
}

fn maps_acceptance(
    store: &Store,
    tracker: &Task,
    child: &Task,
    acceptance: &PlanAcceptance,
) -> Result<bool> {
    if child.completion.as_ref().is_some_and(|manifest| {
        manifest.criteria.iter().any(|criterion| {
            acceptance.criterion.as_ref() == Some(&criterion.id)
                && acceptance.scenario == criterion.scenario
                && acceptance.expected == criterion.expected
        })
    }) {
        return Ok(true);
    }
    let Some(link) = tracker
        .approved_scope
        .as_ref()
        .and_then(|scope| scope.approved_design.as_deref())
        .filter(|link| child.plan.as_deref() == Some(*link))
    else {
        return Ok(false);
    };
    let (id, revision) = link
        .split_once('@')
        .context("invalid approved design link")?;
    let design = store.head(id)?;
    let plan = design
        .plan_by_id(revision)?
        .context("approved design revision is missing")?;
    let Some(scope) = plan
        .document
        .dependencies
        .iter()
        .find(|scope| scope.task == child.title)
    else {
        return Ok(false);
    };
    let scenarios = if scope.acceptance.is_empty() {
        &plan.document.acceptance
    } else {
        &scope.acceptance
    };
    Ok(scenarios.contains(acceptance))
}

pub(crate) fn validate_target(target: &PlanDeliveryTarget) -> Result<()> {
    match target {
        PlanDeliveryTarget::Main => Ok(()),
        PlanDeliveryTarget::Integration { branch } => {
            ensure!(branch != "main", "integration target must differ from main");
            corral_git::validate_branch_name(branch).context("invalid Plan integration branch")?;
            Ok(())
        }
    }
}

pub(crate) fn validate_scope(
    store: &Store,
    scope: &ApprovedScope,
    project: Option<&str>,
) -> Result<()> {
    scope.validate(store, project)?;
    validate_target(&scope.delivery_target)?;
    Ok(())
}

pub(crate) fn check_action(task: &Task, action: &str) -> Result<()> {
    if task.tracker {
        bail!(
            "{} is a tracker; cannot {action}; dispatch its child tasks instead",
            task.id
        );
    }
    Ok(())
}

/// Read both the tracker and its children under the board writer lock. Returning
/// false from try_update preserves timestamps and concurrent owner edits.
pub(crate) fn reconcile_one(ctx: &Ctx, id: &str) -> Result<Option<Change>> {
    let mut from = None;
    let saved = ctx.store.try_update(id, |task| {
        // Acceptance criteria make all-children-Done ready for aggregate acceptance, not Done.
        if !task.tracker
            || task.plan_acceptance_required()
            || matches!(task.status, Status::Done | Status::Dropped)
            || task.children.is_empty()
        {
            return Ok(false);
        }
        let children = ctx.store.list()?;
        if !task.children.iter().all(|id| {
            children
                .iter()
                .any(|child| child.id == *id && child.status == Status::Done)
        }) {
            return Ok(false);
        }
        from = Some(task.status);
        task.status = Status::Done;
        task.queued = false;
        task.dispatch_plan = None;
        ctx.store.log_event(
            Some(&task.id),
            "tracker_completed",
            json!({"from": from, "to": "done", "children": task.children}),
        )?;
        Ok(true)
    })?;
    let Some(task) = saved else {
        return Ok(None);
    };
    Ok(from.map(|from| Change {
        task: task.id,
        from,
        to: Status::Done,
        note: "all tracker children are Done".into(),
        stage: None,
    }))
}

pub(crate) fn reconcile(ctx: &Ctx) -> Result<Vec<Change>> {
    let mut changes = Vec::new();
    for task in ctx.store.list()? {
        if task.tracker
            && let Some(change) = reconcile_one(ctx, &task.id)?
        {
            changes.push(change);
        }
    }
    Ok(changes)
}

#[cfg(test)]
#[path = "tracker_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "tracker_progress_tests.rs"]
mod progress_tests;

#[cfg(test)]
#[path = "tracker_membership_tests.rs"]
mod membership_tests;
