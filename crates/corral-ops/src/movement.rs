//! Informational, coalesced notices. This queue never owns the worker's pending reply.
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
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
}

fn state_path(ctx: &Ctx, id: &str) -> PathBuf {
    ctx.store
        .state_dir()
        .join(format!("main-movement-{id}.json"))
}

fn save(path: &Path, state: &State) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(state)?)
        .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("saving {}", path.display()))
}

/// Run after landing advancement, even if herdr is down. The durable approval log is authority.
pub(crate) fn reconcile(ctx: &Ctx, now: Timestamp, available: bool) -> Result<()> {
    let tasks = ctx.store.list()?;
    let events = ctx.store.events_shared()?;
    for task in &tasks {
        if let Err(error) = reconcile_one(ctx, task, &events, &tasks, now, available) {
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
) -> Result<()> {
    let Some(_lock) = ctx.store.try_lock(&format!("main-movement-{}", task.id))? else {
        return Ok(());
    };
    let path = state_path(ctx, &task.id);
    let mut state: State = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).context("parsing main-movement state")?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => State::default(),
        Err(e) => return Err(e).context("reading main-movement state"),
    };
    // Re-read after obtaining the queue lock; snapshots from sync may already be obsolete.
    let task = ctx.store.get(&task.id)?;
    let active =
        task.status.holds_slot() && task.current_run().is_some_and(|r| r.outcome.is_none());
    for i in 0..state.notices.len() {
        if !state.notices[i].state.terminal() && !state.notices[i].target.matches(&task) {
            delivery::record(
                ctx,
                &path,
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
            save(&path, &state)?;
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
        save(&path, &state)?;
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
    save(&path, &state)?;
    if !state.pending.is_empty()
        && state
            .changed_at
            .is_some_and(|at| now.duration_since(at) >= SignedDuration::from_secs(QUIET_SECS))
    {
        if let Some(notice) = construct(ctx, &task, &state.pending)? {
            state.notices.push(notice);
        }
        state.pending.clear();
        state.changed_at = None;
        save(&path, &state)?;
    }
    for i in 0..state.notices.len() {
        delivery::deliver(ctx, &path, &mut state, i, available)?;
    }
    Ok(())
}

fn construct(ctx: &Ctx, task: &Task, landings: &[Landing]) -> Result<Option<Notice>> {
    let run = task.current_run().context("task has no run")?;
    let repo = &run.worktree;
    let head = corral_git::rev_sha(repo, "HEAD")?;
    let base = corral_git::rev_sha(repo, &run.base)?;
    let merge_base = corral_git::merge_base(repo, &head, &base)?;
    let observed_paths = corral_git::observed_paths(repo)?;
    let mut worker: BTreeSet<_> = corral_git::changed_paths_with_renames(repo, &merge_base, &head)?
        .into_iter()
        .collect();
    worker.extend(observed_paths.iter().cloned());
    let mut landed = BTreeSet::new();
    let mut relevant = Vec::new();
    for landing in landings {
        // Ignore already integrated or no longer reachable approvals (e.g. an owner reset).
        if corral_git::merge_base(repo, &landing.sha, &head)? == landing.sha
            || corral_git::merge_base(repo, &landing.sha, &base)? != landing.sha
        {
            continue;
        }
        let previous = match &landing.previous_sha {
            Some(sha) => sha.clone(),
            None => corral_git::merge_base(repo, &run.base_sha, &landing.sha)?,
        };
        landed.extend(corral_git::changed_paths_with_renames(
            repo,
            &previous,
            &landing.sha,
        )?);
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
        .state_dir()
        .join(format!("main-movement-{}.json", notice.id));
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
