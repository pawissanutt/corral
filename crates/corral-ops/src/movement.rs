//! Informational, coalesced notices. This queue never owns the worker's pending reply.
use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::{Context, Result};
use corral_core::records::RecordArea;
use corral_core::{ArtifactKind, Event, HerdrRef, Run, Task};
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

use crate::Ctx;
use crate::integration::{IntegrationPacket, RepairKey};

mod delivery;
#[cfg(test)]
mod tests;

const QUIET_SECS: i64 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Target {
    task: String,
    run: String,
    started: Timestamp,
    worktree: PathBuf,
    branch: String,
    base: String,
    herdr: Option<HerdrRef>,
}

impl Target {
    fn new(task: &Task, run: &Run) -> Self {
        Self {
            task: task.id.clone(),
            run: run.id.clone(),
            started: run.started,
            worktree: run.worktree.clone(),
            branch: run.branch.clone(),
            base: run.base.clone(),
            herdr: run.herdr.clone(),
        }
    }

    fn matches(&self, task: &Task) -> bool {
        task.status.holds_slot()
            && task
                .current_run()
                .is_some_and(|r| r.outcome.is_none() && Self::new(task, r) == *self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Landing {
    task: String,
    run: Option<String>,
    sha: String,
    previous_sha: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DeliveryState {
    Pending,
    Deferred,
    Sending,
    Delivered,
    Failed,
    Uncertain,
    Cancelled,
}

impl DeliveryState {
    fn terminal(&self) -> bool {
        match self {
            Self::Delivered | Self::Uncertain | Self::Cancelled => true,
            Self::Pending | Self::Deferred | Self::Sending | Self::Failed => false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Notice {
    id: String,
    target: Target,
    landings: Vec<Landing>,
    overlapping_paths: Vec<String>,
    observed_paths: Vec<String>,
    declared_scope: Vec<String>,
    /// The full B context packet, not just a truncated message.
    context: IntegrationPacket,
    rust_headroom: Vec<Headroom>,
    state: DeliveryState,
    detail: Option<String>,
    delivery: Option<corral_herdr::Delivery>,
    /// Pins the pane's shell before the first attempt; a replacement is never a retry target.
    shell_pid: Option<u32>,
    artifact: Option<String>,
    message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Headroom {
    path: String,
    candidate: Option<i64>,
    main: Option<i64>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    target: Option<Target>,
    seen: BTreeSet<String>,
    pending: Vec<Landing>,
    changed_at: Option<Timestamp>,
    notices: Vec<Notice>,
    /// The event sequence through which `target` has seen approvals.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::settling::padded"
    )]
    after: Option<u64>,
}

const RECORDS: RecordArea<State> = RecordArea::new("main-movement");

struct StateRecord<'a> {
    ctx: &'a Ctx,
    key: &'a str,
}

fn state_record<'a>(ctx: &'a Ctx, id: &'a str) -> StateRecord<'a> {
    StateRecord { ctx, key: id }
}

fn save(record: &StateRecord<'_>, state: &State) -> Result<()> {
    RECORDS.save(&record.ctx.store, record.key, state)
}

/// Run after landing advancement, even if herdr is down. The durable approval log is authority.
pub(crate) fn reconcile(ctx: &Ctx, now: Timestamp, available: bool) -> Result<()> {
    let open: Vec<Task> = ctx
        .store
        .heads(&corral_core::backend::HeadQuery::Open { project: None })?
        .iter()
        .map(corral_core::TaskHead::to_shell_task)
        .collect::<Result<_>>()?;
    // Only a landing after an active run started concerns it. A run that already observed
    // approvals continues after its cursor, so a tick reads only approvals committed since the
    // last one; a run new to its record reads back to its start once.
    let tail = ctx.store.event_tail()?;
    let mut cursors = std::collections::BTreeMap::new();
    let mut replay_since: Option<Timestamp> = None;
    for task in open.iter().filter(|task| task.status.holds_slot()) {
        let Some(run) = task.current_run().filter(|r| r.outcome.is_none()) else {
            continue;
        };
        let state = RECORDS.load(&ctx.store, &task.id)?.unwrap_or_default();
        match state
            .after
            .filter(|_| state.target.as_ref() == Some(&Target::new(task, run)))
        {
            Some(after) => {
                cursors.insert(task.id.clone(), after);
            }
            None => replay_since = Some(replay_since.map_or(run.started, |at| at.min(run.started))),
        }
    }
    let replayed: Vec<Event> = match replay_since {
        Some(since) => ctx
            .store
            .events_newest(None, &["approved"], None, |event| event.ts >= since)?
            .into_iter()
            .rev()
            .collect(),
        None => Vec::new(),
    };
    let mut followed = Vec::new();
    if let Some(mut after) = cursors.values().min().copied() {
        while after < tail {
            let page = ctx
                .store
                .events_of_run(None, &["approved"], None, after, 64)?;
            let Some(last) = page.last() else {
                break;
            };
            after = last.seq;
            followed.extend(page.into_iter().filter(|event| event.seq <= tail));
        }
    }
    let events_for = |task: &str| -> Vec<Event> {
        match cursors.get(task) {
            Some(after) => followed
                .iter()
                .filter(|event| event.seq > *after)
                .cloned()
                .collect(),
            None => replayed.clone(),
        }
    };
    let landed: Vec<String> = replayed
        .iter()
        .chain(&followed)
        .filter_map(|e| e.task.clone())
        .collect();
    let mut tasks = open.clone();
    for head in ctx
        .store
        .heads(&corral_core::backend::HeadQuery::Ids(landed))?
    {
        if !tasks.iter().any(|t| t.id == head.id) {
            tasks.push(head.to_shell_task()?);
        }
    }
    for task in &open {
        let events = events_for(&task.id);
        if let Err(error) = reconcile_one(ctx, task, &events, &tasks, now, available, tail) {
            crate::messages::report(
                Some(&task.id),
                "main movement",
                format!("{} main-movement notice deferred: {error:#}", task.id),
            );
        }
    }
    Ok(())
}

fn reconcile_one(
    ctx: &Ctx,
    task: &Task,
    events: &[Event],
    tasks: &[Task],
    now: Timestamp,
    available: bool,
    tail: u64,
) -> Result<()> {
    let Some(_lock) = ctx.store.try_lock(&format!("main-movement-{}", task.id))? else {
        return Ok(());
    };
    let record = state_record(ctx, &task.id);
    let mut state = RECORDS.load(&ctx.store, &task.id)?.unwrap_or_default();
    // Re-read after obtaining the queue lock; snapshots from sync may already be obsolete.
    let task = ctx.store.head(&task.id)?.to_shell_task()?;
    let active =
        task.status.holds_slot() && task.current_run().is_some_and(|r| r.outcome.is_none());
    for i in 0..state.notices.len() {
        if !state.notices[i].state.terminal() && !state.notices[i].target.matches(&task) {
            delivery::record(
                ctx,
                &record,
                &mut state,
                i,
                DeliveryState::Cancelled,
                "run finished or identity changed",
            )?;
        }
    }
    if !active {
        if state.target.is_some() {
            state.target = None;
            state.pending.clear();
            save(&record, &state)?;
        }
        return Ok(());
    }
    let run = task.current_run().context("active run disappeared")?;
    let target = Target::new(&task, run);
    if state
        .target
        .as_ref()
        .is_some_and(|old| old.run == target.run && old.started == target.started && old != &target)
    {
        // A different pane/worktree on the same run is not permission to replay its backlog.
        // A real retry has a new run identity and starts its own observation window.
        state.pending.clear();
        save(&record, &state)?;
        return Ok(());
    }
    if state.target.as_ref() != Some(&target) {
        state.target = Some(target.clone());
        state.seen.clear();
        state.pending.clear();
        state.changed_at = None;
    }
    for event in events {
        if event.kind != "approved"
            || event.ts < run.started
            || event.task.as_deref() == Some(&task.id)
            || !matches!(
                event.detail["strategy"].as_str(),
                Some("fast_forward" | "rebase")
            )
        {
            continue;
        }
        let landed_task = event
            .task
            .as_ref()
            .and_then(|id| tasks.iter().find(|t| &t.id == id));
        let project = event.detail["project"]
            .as_str()
            .or_else(|| landed_task.and_then(|t| t.project.as_deref()));
        let base = event.detail["base"]
            .as_str()
            .or_else(|| landed_task.and_then(|t| t.current_run().map(|r| r.base.as_str())));
        let (Some(id), Some(sha)) = (&event.task, event.detail["sha"].as_str()) else {
            continue;
        };
        if project != task.project.as_deref() || base != Some(&run.base) {
            continue;
        }
        if state.seen.insert(sha.to_string()) {
            state.pending.push(Landing {
                task: id.clone(),
                run: event.detail["run"].as_str().map(str::to_owned),
                sha: sha.to_string(),
                previous_sha: event.detail["previous_sha"].as_str().map(str::to_owned),
            });
            state.changed_at = Some(now);
        }
    }
    state.after = Some(tail);
    save(&record, &state)?;
    if !state.pending.is_empty()
        && state
            .changed_at
            .is_some_and(|at| now.duration_since(at) >= SignedDuration::from_secs(QUIET_SECS))
    {
        if let Some(notice) = construct(ctx, &ctx.store.get(&task.id)?, &state.pending)? {
            state.notices.push(notice);
        }
        state.pending.clear();
        state.changed_at = None;
        save(&record, &state)?;
    }
    for i in 0..state.notices.len() {
        delivery::deliver(ctx, &record, &mut state, i, available)?;
    }
    Ok(())
}

fn construct(ctx: &Ctx, task: &Task, landings: &[Landing]) -> Result<Option<Notice>> {
    let run = task.current_run().context("task has no run")?;
    let repo = &run.worktree;
    let head = corral_git::rev_sha(repo, "HEAD")?;
    let base = corral_git::rev_sha(repo, &run.base)?;
    let memo = crate::integration::git_memo(ctx);
    let merge_base = memo.merge_base(repo, &head, &base)?;
    let observed_paths = corral_git::observed_paths(repo)?;
    let mut worker: BTreeSet<_> = memo
        .changed_paths_with_renames(repo, &merge_base, &head)?
        .into_iter()
        .collect();
    worker.extend(observed_paths.iter().cloned());
    let mut landed = BTreeSet::new();
    let mut relevant = Vec::new();
    for landing in landings {
        // Ignore already integrated or no longer reachable approvals (e.g. an owner reset).
        if memo.merge_base(repo, &landing.sha, &head)? == landing.sha
            || memo.merge_base(repo, &landing.sha, &base)? != landing.sha
        {
            continue;
        }
        let previous = match &landing.previous_sha {
            Some(sha) => sha.clone(),
            None => memo.merge_base(repo, &run.base_sha, &landing.sha)?,
        };
        landed.extend(memo.changed_paths_with_renames(repo, &previous, &landing.sha)?);
        relevant.push(landing.clone());
    }
    let overlapping_paths: Vec<_> = landed
        .into_iter()
        .filter(|p| {
            worker.contains(p)
                || task
                    .write_scope
                    .iter()
                    .any(|scope| crate::artifacts::glob_match(scope, p))
        })
        .collect();
    if overlapping_paths.is_empty() {
        return Ok(None);
    }
    let target = Target::new(task, run);
    let id = crate::verification::digest(&serde_json::to_vec(&(&target, &relevant))?);
    let key = RepairKey {
        task: task.id.clone(),
        run: run.id.clone(),
        head,
        base,
        reason: format!("main_movement:{id}"),
    };
    let policy = ctx
        .store
        .config
        .review_policy(task.project.as_deref(), task.review);
    let mut context = IntegrationPacket::build(ctx, task, run, &policy, key, vec![], None)?;
    let size_limits = crate::integration::effective_size_limits(ctx, task, repo, &policy)?;
    // A burst can modify then restore a path, leaving no net change in B's tree diff.
    // Retain its current size hint even when it is absent from that net path list.
    for path in &overlapping_paths {
        if !context.file_sizes.iter().any(|s| &s.path == path) {
            context.file_sizes.push(crate::integration::FileSize::at(
                repo,
                &context.key,
                path.clone(),
                &size_limits,
            )?);
        }
    }
    context.file_sizes.sort_by(|a, b| a.path.cmp(&b.path));
    let rust_headroom = context
        .file_sizes
        .iter()
        .filter(|s| overlapping_paths.contains(&s.path))
        .filter_map(|s| s.ceiling.map(|ceiling| (s, ceiling)))
        .map(|(s, ceiling)| -> Result<_> {
            let ceiling = i64::try_from(ceiling)?;
            let remaining = |n: Option<usize>| -> Result<Option<i64>> {
                n.map(|n| Ok(ceiling.saturating_sub(i64::try_from(n)?)))
                    .transpose()
            };
            Ok(Headroom {
                path: s.path.clone(),
                candidate: remaining(s.candidate_sloc)?,
                main: remaining(s.main_sloc)?,
            })
        })
        .collect::<Result<_>>()?;
    Ok(Some(Notice {
        id,
        target,
        landings: relevant,
        overlapping_paths,
        observed_paths,
        declared_scope: task.write_scope.clone(),
        context,
        rust_headroom,
        state: DeliveryState::Pending,
        detail: None,
        delivery: None,
        shell_pid: None,
        artifact: None,
        message: None,
    }))
}

fn attach(ctx: &Ctx, notice: &Notice) -> Result<String> {
    let path = ctx
        .store
        .scratch_tmp()?
        .join(format!("main-movement-{}.json", notice.id));
    std::fs::create_dir_all(
        path.parent()
            .context("movement materialization has no parent")?,
    )
    .context("creating movement materialization directory")?;
    std::fs::write(&path, serde_json::to_vec_pretty(notice)?)
        .with_context(|| format!("writing {}", path.display()))?;
    let artifact = crate::artifacts::add_artifact_for_run(
        ctx,
        &notice.target.task,
        &notice.target.run,
        &path,
        Some(ArtifactKind::Report),
        Some("Main movement context"),
    )?;
    Ok(ctx.store.dir().join(artifact.path).display().to_string())
}
