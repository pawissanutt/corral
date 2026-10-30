//! `corral agent rewind` (T-575): an owner rewinds a worker's isolated worktree and Pi session
//! cursor to the start of an earlier turn. The worker host's `checkpoints` capability leaves a
//! private per-worktree ref and one `corral_checkpoint` session entry for each turn's start and
//! end. A rewind is recorded before anything changes and commits each stage before the next one
//! starts: `prepared`, then (inside the resume path) the fence and the reaping of the old
//! generation's jobs (`fenced`), a saved snapshot of the worktree (`saved`), the restore
//! (`restoring`, `restored`), and the next generation opened at the checkpoint's session entry
//! (`launched`). Running it again continues an interrupted rewind; `--abort` puts restored files
//! back from the saved snapshot. Only paths the agent changed during its turns since the
//! checkpoint are restored; a path that also changed outside them refuses the rewind, and every
//! other change stays. Receipts, turns, usage and approvals are never rewound.
//! Owner: T-460 child checkpoints-rewind.
use std::collections::{BTreeSet, HashMap};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use corral_core::ArtifactKind;
use corral_core::agent::{
    self, AgentRunState, CHECKPOINT_ENTRY, CheckpointEntry, CheckpointPhase, ContainerRef,
    REWIND_PATHS_MAX, RewindRecord, RewindStage,
};
use jiff::Timestamp;
use serde_json::{Value, json};

use super::cli::AgentTarget;
use crate::Ctx;
use git::{Restored, changed, ensure_isolated, git, restore, snapshot, status_text};

#[path = "rewind_git.rs"]
mod git;

pub const OWNER: &str = "checkpoints-rewind";
const SESSION_MAX_BYTES: u64 = 256 * 1024 * 1024;
const LINE_MAX_BYTES: u64 = 64 * 1024;
const CHECKPOINTS_MAX: usize = 4096;
const CONFLICTS_SHOWN: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RewindArgs {
    pub target: AgentTarget,
    /// The checkpoint (`g<generation>-t<turn>`) to restore; absent continues a pending rewind.
    pub to: Option<String>,
    pub list: bool,
    pub abort: bool,
    pub replace: bool,
}

pub fn rewind(ctx: &Ctx, args: &RewindArgs) -> Result<()> {
    let container = super::recovery::resolve(ctx, &args.target)?;
    let outcome = match (args.list, args.abort) {
        (true, _) => list(ctx, &container)?,
        (false, true) => abort(ctx, &container)?,
        (false, false) => rewind_container(ctx, &container, args.to.as_deref(), args.replace)?,
    };
    println!("{}", serde_json::to_string_pretty(&outcome)?);
    Ok(())
}

/// A worker run's binding and its worktree.
struct Worker {
    task: String,
    run: String,
    worktree: PathBuf,
    state: AgentRunState,
}

fn worker(ctx: &Ctx, container: &ContainerRef) -> Result<Worker> {
    let ContainerRef::Worker { task, run } = container else {
        bail!(
            "not_worker: rewind applies to a worker run's isolated worktree; {} is not one",
            container.key()
        );
    };
    let record = ctx.store.get(task)?;
    let found = agent::container_run(&record, container)
        .with_context(|| format!("{} does not exist", container.key()))?;
    let state = found
        .agent_host
        .as_deref()
        .cloned()
        .with_context(|| format!("{} has no Pi-native binding", container.key()))?;
    Ok(Worker {
        task: task.clone(),
        run: run.clone(),
        worktree: found.worktree.clone(),
        state,
    })
}

/// One checkpoint snapshot as the session recorded it.
#[derive(Debug, Clone)]
struct Checkpoint {
    entry: CheckpointEntry,
    commit: String,
    head: String,
    at: Option<String>,
}

/// The session file the next generation resumes: the latest one a generation bound.
fn session_file(state: &AgentRunState) -> Option<PathBuf> {
    let bound = state
        .generations
        .iter()
        .filter_map(|g| g.bound.as_ref().map(|b| (g.n, b)));
    bound
        .max_by_key(|(n, _)| *n)
        .map(|(_, b)| b.session_file.clone())
}

/// Calls `f` with each JSON entry of the session, in order, within byte bounds; read, never
/// written. Lines over `LINE_MAX_BYTES` are messages, never checkpoint or bind entries.
fn entries(path: &Path, mut f: impl FnMut(Value) -> Result<()>) -> Result<()> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).with_context(|| format!("opening {}", path.display())),
    };
    ensure!(
        file.metadata()?.is_file(),
        "{} is not a regular file",
        path.display()
    );
    let mut reader = BufReader::new(file.take(SESSION_MAX_BYTES));
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = (&mut reader)
            .take(LINE_MAX_BYTES)
            .read_until(b'\n', &mut line)
            .context("reading the Pi session")?;
        if read == 0 {
            return Ok(());
        }
        if read as u64 == LINE_MAX_BYTES && line.last() != Some(&b'\n') {
            let mut rest = Vec::new();
            reader
                .read_until(b'\n', &mut rest)
                .context("reading the Pi session")?;
            continue;
        }
        if let Ok(value) = serde_json::from_slice::<Value>(&line) {
            f(value)?;
        }
    }
}

/// Every complete checkpoint entry of the session, in session order.
fn scan(path: &Path) -> Result<Vec<Checkpoint>> {
    let mut found = Vec::new();
    entries(path, |value| {
        if value["type"] != "custom" || value["customType"] != CHECKPOINT_ENTRY {
            return Ok(());
        }
        let Ok(entry) = serde_json::from_value::<CheckpointEntry>(value["data"].clone()) else {
            return Ok(());
        };
        let (Some(commit), Some(head)) = (entry.commit.clone(), entry.head.clone()) else {
            return Ok(());
        };
        if entry.phase == CheckpointPhase::Start && entry.leaf.is_none() {
            return Ok(());
        }
        ensure!(
            found.len() < CHECKPOINTS_MAX,
            "the session holds more than {CHECKPOINTS_MAX} checkpoints"
        );
        let at = value["timestamp"].as_str().map(str::to_owned);
        found.push(Checkpoint {
            entry,
            commit,
            head,
            at,
        });
        Ok(())
    })?;
    Ok(found)
}

/// Whether a host of generation `from` or later bound the session on a branch from `leaf`: its
/// `corral_host_bound` entry descends from that entry.
fn bound_from(path: &Path, leaf: &str, from: u32) -> Result<bool> {
    let (mut parents, mut markers) = (HashMap::new(), Vec::new());
    entries(path, |value| {
        let Some(id) = value["id"].as_str() else {
            return Ok(());
        };
        let generation = value["data"]["generation"].as_u64();
        if value["type"] == "custom"
            && value["customType"] == "corral_host_bound"
            && generation.is_some_and(|g| g >= u64::from(from))
        {
            markers.push(id.to_owned());
        }
        if let Some(parent) = value["parentId"].as_str() {
            parents.insert(id.to_owned(), parent.to_owned());
        }
        Ok(())
    })?;
    Ok(markers.iter().any(|marker| {
        let mut at = marker.as_str();
        for _ in 0..=parents.len() {
            match parents.get(at) {
                Some(parent) if parent == leaf => return true,
                Some(parent) => at = parent,
                None => return false,
            }
        }
        false
    }))
}

/// The session entry a relaunch of the worker must open at: a restored rewind's, then the last
/// launched one's until a host is seen bound on a branch from it, which settles it (`applied`).
/// A relaunch whose host died before opening the session is therefore given it again.
pub(crate) fn owed_cursor(
    ctx: &Ctx,
    container: &ContainerRef,
    state: &AgentRunState,
) -> Result<Option<String>> {
    if let Some(pending) = state.pending_rewind() {
        if pending.stage != RewindStage::Restored || pending.applied.is_some() {
            return Ok(None);
        }
        // A launch can reach its host before `Launched` is recorded; the first relaunch records
        // the generation it opens before that host can run, and later ones check its evidence.
        let Some(from) = pending.generation else {
            let (id, next) = (pending.id.clone(), state.binding.generation + 1);
            update(ctx, container, |state| {
                if let Some(record) = state.rewind_mut(&id) {
                    record.generation.get_or_insert(next);
                }
                Ok(())
            })?;
            return Ok(Some(pending.leaf.clone()));
        };
        return settle_cursor(ctx, container, state, pending, from);
    }
    let launched = state.rewinds.iter().rev();
    let Some(last) = launched
        .into_iter()
        .find(|r| r.stage == RewindStage::Launched)
    else {
        return Ok(None);
    };
    if last.applied.is_some() {
        return Ok(None);
    }
    let from = last
        .generation
        .context("a launched rewind names its generation")?;
    settle_cursor(ctx, container, state, last, from)
}

/// The rewind's cursor until a host of generation `from` or later is seen bound in the session on
/// a branch from it; that evidence records it `applied` and nothing is owed any more.
fn settle_cursor(
    ctx: &Ctx,
    container: &ContainerRef,
    state: &AgentRunState,
    record: &RewindRecord,
    from: u32,
) -> Result<Option<String>> {
    let bound = match session_file(state) {
        Some(path) => bound_from(&path, &record.leaf, from)?,
        None => false,
    };
    if !bound {
        return Ok(Some(record.leaf.clone()));
    }
    let id = record.id.clone();
    update(ctx, container, |state| {
        if let Some(record) = state.rewind_mut(&id) {
            record.applied.get_or_insert_with(Timestamp::now);
        }
        Ok(())
    })?;
    Ok(None)
}

/// The checkpoints on the current timeline: a launched rewind abandons every checkpoint after
/// the start it restored, and the generations it opened continue from there.
fn timeline(all: Vec<Checkpoint>, rewinds: &[RewindRecord]) -> Vec<Checkpoint> {
    let mut launched: Vec<(u32, &str)> = rewinds
        .iter()
        .filter(|r| r.stage == RewindStage::Launched)
        .filter_map(|r| r.generation.map(|g| (g, r.checkpoint.as_str())))
        .collect();
    launched.sort();
    let mut pending = launched.into_iter().peekable();
    let mut line: Vec<Checkpoint> = Vec::new();
    let cut = |line: &mut Vec<Checkpoint>, id: &str| {
        let start = line
            .iter()
            .position(|c| c.entry.id == id && c.entry.phase == CheckpointPhase::Start);
        if let Some(at) = start {
            line.truncate(at + 1);
        }
    };
    for checkpoint in all {
        while let Some((_, id)) = pending.next_if(|(g, _)| checkpoint.entry.generation >= *g) {
            cut(&mut line, id);
        }
        line.push(checkpoint);
    }
    for (_, id) in pending {
        cut(&mut line, id);
    }
    line
}

fn checkpoints(state: &AgentRunState) -> Result<Vec<Checkpoint>> {
    let all = match session_file(state) {
        Some(path) => scan(&path)?,
        None => Vec::new(),
    };
    Ok(timeline(all, &state.rewinds))
}

/// What a rewind restores: every path the agent changed in its turns since the checkpoint.
/// Changes between turns, or after the last one ended, were not the agent's; a path changed
/// both ways is a conflict, and so is a HEAD that moved outside the agent's turns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Plan {
    agent: BTreeSet<String>,
    conflicts: BTreeSet<String>,
}

fn plan(dir: &Path, span: &[Checkpoint], current: (&str, &str)) -> Result<Plan> {
    let mut points: Vec<(Option<CheckpointPhase>, &str, &str)> = span
        .iter()
        .map(|c| (Some(c.entry.phase), c.commit.as_str(), c.head.as_str()))
        .collect();
    points.push((None, current.0, current.1));
    let (mut agent, mut foreign, mut head_foreign) = (BTreeSet::new(), BTreeSet::new(), false);
    for pair in points.windows(2) {
        let [(from_phase, from, from_head), (to_phase, to, to_head)] = pair else {
            continue;
        };
        let names = changed(dir, from, to)?;
        // An interval after an end checkpoint and before the next start is between turns.
        if *from_phase == Some(CheckpointPhase::End) && *to_phase != Some(CheckpointPhase::End) {
            head_foreign |= from_head != to_head;
            foreign.extend(names);
        } else {
            agent.extend(names);
        }
    }
    let mut conflicts: BTreeSet<String> = agent.intersection(&foreign).cloned().collect();
    if head_foreign {
        conflicts.insert("HEAD".into());
    }
    Ok(Plan { agent, conflicts })
}

fn refuse_conflicts(plan: &Plan) -> Result<()> {
    if plan.conflicts.is_empty() {
        return Ok(());
    }
    let shown: Vec<&str> = plan
        .conflicts
        .iter()
        .take(CONFLICTS_SHOWN)
        .map(String::as_str)
        .collect();
    bail!(
        "rewind_conflict: {} path(s) changed both in the agent's turns and outside them, so \
         restoring would overwrite changes that are not the agent's: {}{}",
        plan.conflicts.len(),
        shown.join(", "),
        if plan.conflicts.len() > shown.len() {
            ", ..."
        } else {
            ""
        }
    )
}

fn update<R>(
    ctx: &Ctx,
    container: &ContainerRef,
    f: impl FnOnce(&mut AgentRunState) -> Result<R>,
) -> Result<R> {
    let task = container.task().context("a rewind names a task")?;
    let mut out = None;
    ctx.store.try_update(task, |t| {
        let state =
            agent::container_state_mut(t, container).context("the agent binding disappeared")?;
        out = Some(f(state)?);
        Ok(true)
    })?;
    out.context("the rewind record was not updated")
}

fn set_stage(
    ctx: &Ctx,
    container: &ContainerRef,
    id: &str,
    stage: RewindStage,
    apply: impl FnOnce(&mut RewindRecord),
) -> Result<RewindRecord> {
    let record = update(ctx, container, |state| {
        let record = state
            .rewind_mut(id)
            .context("the rewind record disappeared")?;
        record.stage = stage;
        record.updated = Timestamp::now();
        apply(record);
        Ok(record.clone())
    })?;
    super::failpoint(&format!("rewind_{}", stage_name(stage)));
    Ok(record)
}

fn stage_name(stage: RewindStage) -> &'static str {
    match stage {
        RewindStage::Prepared => "prepared",
        RewindStage::Fenced => "fenced",
        RewindStage::Saved => "saved",
        RewindStage::Restoring => "restoring",
        RewindStage::Restored => "restored",
        RewindStage::Launched => "launched",
        RewindStage::Aborted => "aborted",
    }
}

fn record(ctx: &Ctx, container: &ContainerRef, id: &str) -> Result<RewindRecord> {
    let state = worker(ctx, container)?.state;
    state
        .rewinds
        .into_iter()
        .find(|r| r.id == id)
        .context("the rewind record disappeared")
}

/// The restore plan against the saved snapshot, recomputed the same way on every continuation.
fn saved_plan(w: &Worker, record: &RewindRecord) -> Result<Plan> {
    let before = record
        .before
        .as_deref()
        .context("the rewind has no saved snapshot")?;
    let line = checkpoints(&w.state)?;
    let start = position(&line, &record.checkpoint)?;
    let before_head = git(&w.worktree, &["rev-parse", &format!("{before}^")])?;
    plan(&w.worktree, &line[start..], (before, &before_head))
}

fn position(line: &[Checkpoint], id: &str) -> Result<usize> {
    line.iter()
        .position(|c| c.entry.id == id && c.entry.phase == CheckpointPhase::Start)
        .with_context(|| {
            format!(
                "checkpoint_unknown: no start checkpoint {id} on this run's timeline (see --list)"
            )
        })
}

/// The stages between the fence and the launch, each continued from where it stopped.
fn stages(ctx: &Ctx, container: &ContainerRef, id: &str) -> Result<()> {
    loop {
        let current = record(ctx, container, id)?;
        let w = worker(ctx, container)?;
        match current.stage {
            RewindStage::Prepared => {
                super::jobs::reap(ctx, &w.task, Timestamp::now())?;
                let live = w
                    .state
                    .jobs
                    .iter()
                    .filter(|j| j.state.live() && j.generation <= current.from_generation);
                let live: Vec<&str> = live.map(|j| j.id.as_str()).collect();
                let unreaped = worker(ctx, container)?
                    .state
                    .jobs
                    .iter()
                    .any(|j| live.contains(&j.id.as_str()) && j.state.live());
                ensure!(
                    !unreaped,
                    "jobs_alive: the fenced generation's jobs could not be reaped: {live:?}"
                );
                set_stage(ctx, container, id, RewindStage::Fenced, |_| {})?;
            }
            RewindStage::Fenced => {
                let (before, _) = snapshot(
                    &w.worktree,
                    &format!("corral rewind {id}: worktree before restore"),
                )?;
                let name = format!("refs/worktree/corral/rewind/{}/{id}", w.run);
                git(&w.worktree, &["update-ref", &name, &before])?;
                set_stage(ctx, container, id, RewindStage::Saved, |r| {
                    r.before = Some(before);
                    r.before_ref = Some(name);
                })?;
            }
            RewindStage::Saved => {
                refuse_conflicts(&saved_plan(&w, &current)?)?;
                set_stage(ctx, container, id, RewindStage::Restoring, |_| {})?;
            }
            RewindStage::Restoring => {
                let plan = saved_plan(&w, &current)?;
                let before = current
                    .before
                    .as_deref()
                    .context("the rewind has no saved snapshot")?;
                let done = restore(&w.worktree, &plan.agent, &current.commit, before)?;
                set_stage(ctx, container, id, RewindStage::Restored, |r| {
                    r.restored_count = done.restored.len();
                    r.restored = done.restored.into_iter().take(REWIND_PATHS_MAX).collect();
                    r.kept = done.kept;
                })?;
            }
            RewindStage::Restored | RewindStage::Launched | RewindStage::Aborted => return Ok(()),
        }
    }
}

/// Rewinds to `to`, or continues the pending rewind. Nothing is fenced when it is refused.
pub fn rewind_container(
    ctx: &Ctx,
    container: &ContainerRef,
    to: Option<&str>,
    replace: bool,
) -> Result<Value> {
    let w = worker(ctx, container)?;
    let id = match w.state.pending_rewind() {
        Some(pending) => {
            ensure!(
                to.is_none_or(|to| to == pending.checkpoint),
                "rewind_pending: {} is still rewinding to {} ({}); run `corral agent rewind {}` \
                 to continue it or `--abort` to abandon it",
                container.key(),
                pending.checkpoint,
                stage_name(pending.stage),
                w.task
            );
            pending.id.clone()
        }
        None => prepare(ctx, container, &w, to)?,
    };
    let mut restore = || stages(ctx, container, &id);
    let resumed = super::recovery::recover_with(ctx, container, replace, None, Some(&mut restore));
    let (outcome, _) = match resumed {
        Ok(resumed) => resumed,
        Err(error) => return Err(release_unfenced(ctx, container, &id, error)),
    };
    let generation = outcome["generation"]
        .as_u64()
        .and_then(|g| u32::try_from(g).ok());
    super::failpoint("rewind_launch_submitted");
    let done = set_stage(ctx, container, &id, RewindStage::Launched, |r| {
        r.generation = r.generation.or(generation)
    })?;
    ctx.store.log_event(
        Some(&w.task),
        "agent_rewound",
        json!({"container": container.key(), "rewind": done.id, "checkpoint": done.checkpoint,
            "fenced": outcome["fenced"], "generation": generation, "restored": done.restored_count,
            "kept": done.kept}),
    )?;
    Ok(json!({"container": container.key(), "rewind": done, "resume": outcome}))
}

/// A rewind the resume path refused before its fence (a live host without `--replace`, a gone
/// pane) changed nothing: it is closed `aborted`, so it never holds up `corral agent resume`.
fn release_unfenced(
    ctx: &Ctx,
    container: &ContainerRef,
    id: &str,
    error: anyhow::Error,
) -> anyhow::Error {
    let released = update(ctx, container, |state| {
        let n = state.binding.generation;
        let fenced = state.generation(n).is_some_and(|g| g.fenced.is_some());
        let Some(record) = state.rewind_mut(id) else {
            return Ok(false);
        };
        if record.stage != RewindStage::Prepared || fenced || n != record.from_generation {
            return Ok(false);
        }
        record.stage = RewindStage::Aborted;
        record.updated = Timestamp::now();
        record.detail = Some(format!("refused before the fence: {error:#}"));
        Ok(true)
    });
    match released {
        Ok(_) => error,
        Err(cleanup) => error.context(format!("closing the refused rewind failed: {cleanup:#}")),
    }
}

/// Checks the target and records the rewind `prepared`; refusals change nothing.
fn prepare(ctx: &Ctx, container: &ContainerRef, w: &Worker, to: Option<&str>) -> Result<String> {
    let to = to.context("rewind_target_missing: name a checkpoint with --to (see `--list`)")?;
    ensure_isolated(&w.worktree)?;
    ensure!(
        w.state.binding.generation > 0,
        "{} has no generation to rewind",
        container.key()
    );
    let line = checkpoints(&w.state)?;
    let start = position(&line, to)?;
    let target = &line[start];
    let reference = target
        .entry
        .git_ref
        .as_deref()
        .context("checkpoint_missing: the checkpoint names no ref")?;
    let resolved =
        git(&w.worktree, &["rev-parse", "--verify", "-q", reference]).unwrap_or_default();
    ensure!(
        resolved == target.commit,
        "checkpoint_missing: {reference} no longer points at checkpoint {to}'s commit {}",
        target.commit
    );
    let (now, head) = snapshot(&w.worktree, "corral rewind: worktree check")?;
    refuse_conflicts(&plan(&w.worktree, &line[start..], (&now, &head))?)?;
    let leaf = target
        .entry
        .leaf
        .clone()
        .context("the start checkpoint has no session entry")?;
    let at = Timestamp::now();
    let id = update(ctx, container, |state| {
        ensure!(
            state.pending_rewind().is_none(),
            "rewind_pending: another rewind was recorded first"
        );
        let n = state
            .rewinds
            .iter()
            .filter_map(|r| r.id.strip_prefix("rw-")?.parse::<u32>().ok())
            .max();
        let id = format!("rw-{}", n.unwrap_or(0) + 1);
        state.push_rewind(RewindRecord {
            id: id.clone(),
            checkpoint: to.into(),
            commit: target.commit.clone(),
            head: target.head.clone(),
            leaf,
            from_generation: state.binding.generation,
            stage: RewindStage::Prepared,
            before: None,
            before_ref: None,
            generation: None,
            applied: None,
            restored: Vec::new(),
            restored_count: 0,
            kept: Vec::new(),
            detail: None,
            at,
            updated: at,
        });
        Ok(id)
    })?;
    ctx.store.log_event(
        Some(&w.task),
        "agent_rewind_prepared",
        json!({"container": container.key(), "rewind": id, "checkpoint": to}),
    )?;
    super::failpoint("rewind_prepared");
    Ok(id)
}

/// Abandons the pending rewind; files it restored go back to the saved snapshot first.
fn abort(ctx: &Ctx, container: &ContainerRef) -> Result<Value> {
    let w = worker(ctx, container)?;
    let pending = w
        .state
        .pending_rewind()
        .cloned()
        .context("no_rewind_pending: nothing to abort")?;
    let mut undone = Restored::default();
    if let Some(before) = pending.before.as_deref() {
        let plan = saved_plan(&w, &pending)?;
        undone = restore(&w.worktree, &plan.agent, before, &pending.commit)?;
    }
    let restored = undone.restored.len();
    let done = set_stage(ctx, container, &pending.id, RewindStage::Aborted, |r| {
        r.detail = Some(format!(
            "aborted at {}; {restored} path(s) put back",
            stage_name(pending.stage)
        ));
        r.kept = undone.kept;
    })?;
    ctx.store.log_event(
        Some(&w.task),
        "agent_rewind_aborted",
        json!({"container": container.key(), "rewind": done.id, "from": stage_name(pending.stage),
            "put_back": restored}),
    )?;
    Ok(json!({"container": container.key(), "rewind": done, "put_back": restored}))
}

fn list(ctx: &Ctx, container: &ContainerRef) -> Result<Value> {
    let w = worker(ctx, container)?;
    let line = checkpoints(&w.state)?;
    let ended: BTreeSet<&str> = line
        .iter()
        .filter(|c| c.entry.phase == CheckpointPhase::End)
        .map(|c| c.entry.id.as_str())
        .collect();
    let starts = line.iter().filter(|c| c.entry.phase == CheckpointPhase::Start).map(|c| {
        json!({"id": c.entry.id, "generation": c.entry.generation, "turn": c.entry.turn,
            "commit": c.commit, "head": c.head, "at": c.at, "ended": ended.contains(c.entry.id.as_str())})
    });
    Ok(json!({
        "container": container.key(), "checkpoints": starts.collect::<Vec<_>>(),
        "pending": w.state.pending_rewind(), "rewinds": w.state.rewinds,
    }))
}

/// `git status --porcelain=v2` and `git diff --stat` of a worktree, kept as a run artifact (the
/// PM's as a file): the evidence for every upstream call recovery reports unknown.
pub(super) fn worktree_evidence(
    ctx: &Ctx,
    container: &ContainerRef,
    n: u32,
    worktree: &Path,
    artifact_run: Option<&str>,
) -> Result<String> {
    let dir = ctx.store.state_dir().join("agent-recovery");
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let name = container.key().replace('/', "-");
    let path = dir.join(format!("{name}-g{n}-worktree.txt"));
    let text = format!(
        "worktree {}\n\n$ git status --porcelain=v2\n{}\n$ git diff --stat\n{}",
        worktree.display(),
        status_text(worktree, &["status", "--porcelain=v2"]),
        status_text(worktree, &["diff", "--stat"])
    );
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    let (Some(task), Some(run)) = (container.task(), artifact_run) else {
        return Ok(path.display().to_string());
    };
    let title = format!(
        "Worktree after host loss ({} generation {n})",
        container.key()
    );
    let artifact = crate::artifacts::add_artifact_for_run(
        ctx,
        task,
        run,
        &path,
        Some(ArtifactKind::Evidence),
        Some(&title),
    )?;
    Ok(format!("artifact {}", artifact.path))
}

#[cfg(all(test, target_os = "linux"))]
#[path = "rewind_tests.rs"]
mod tests;
