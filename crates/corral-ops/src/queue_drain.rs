//! The Ready-queue drain, which attempts each queued task at most once per meaningful state
//! change (T-505).
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Error, Result};
use corral_core::{Status, Task};
use jiff::Timestamp;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::dispatch::Admission;
use crate::{Change, Ctx, DispatchOpts, DispatchOutcome};

const REFUSALS: &str = "queue-refusals.json";

pub(crate) fn start_queued(ctx: &Ctx, now: Timestamp) -> Result<Vec<Change>> {
    let mut fresh = ctx.clone();
    fresh.store.reload_config_if_changed()?;
    let ctx = &fresh;
    // One process drains at a time and the others skip this pass instead of waiting, so a
    // refusal is checked, attempted and recorded once across every syncing process. The board
    // and refusal states are read only after the claim.
    let Some(_drain) = ctx.store.try_lock("queue-drain")? else {
        return Ok(Vec::new());
    };
    // One read of the board selects the queue and answers every skip check until this drain
    // attempts a dispatch, so the operating policy is resolved once per drain, not per check
    // (T-547). An attempt may change the board, so the next check reads it again.
    let board = ctx.store.read_only_snapshot()?;
    let mut queued: Vec<Task> = board
        .list()?
        .into_iter()
        .filter(|t| !t.tracker && (t.launch.is_some() || crate::queue::authorized_ready(&board, t)))
        .collect();
    crate::queue::sort_candidates(&mut queued);
    let mut refusals = Refusals::load(ctx)?;
    refusals.retain(&queued);
    let mut changes = Vec::new();
    let mut view = Some(board);
    for (index, task) in queued.into_iter().enumerate() {
        let store = match view.take() {
            Some(store) => store,
            None => ctx.store.read_only_snapshot()?,
        };
        let blocked = still_blocked(ctx, &store, &task, index + 1, now);
        view = Some(store);
        if blocked {
            continue;
        }
        if task.launch.is_none()
            && refusals.has(&task.id)
            && refusals.unchanged(&task.id, &refusal_state(ctx, &task)?)
        {
            continue;
        }
        view = None;
        let result = (|| {
            if !task.queued && task.launch.is_none() {
                crate::queue::enqueue_ready(ctx, &task.id, &DispatchOpts::default())?;
            }
            crate::dispatch::dispatch_queued(ctx, &task.id, index + 1, now)
        })();
        match result {
            Ok(outcome) => {
                refusals.clear(&task.id);
                if let DispatchOutcome::Started { run_id, .. } = outcome {
                    changes.push(Change {
                        task: task.id.clone(),
                        from: task.status,
                        to: Status::Running,
                        note: format!("dispatched queued task as {run_id}"),
                        stage: None,
                    });
                }
            }
            Err(err) => {
                crate::messages::report(
                    Some(&task.id),
                    "dispatch",
                    format!("dispatching queued {} failed: {err:#}", task.id),
                );
                if contended(&err) {
                    continue;
                }
                ctx.store.log_event(
                    Some(&task.id),
                    "dispatch_failed",
                    json!({ "error": format!("{err:#}") }),
                )?;
                // A reserved launch's failure depends on the terminal engine, which no state
                // here describes, so its recovery keeps retrying every pass.
                match ctx.store.get(&task.id) {
                    Ok(current) if current.launch.is_none() => {
                        refusals.record(&task.id, refusal_state(ctx, &current)?);
                    }
                    _ => refusals.clear(&task.id),
                }
            }
        }
    }
    refusals.save(ctx)?;
    Ok(changes)
}

/// A queued task whose admission check still names a blocker needs no dispatch attempt this pass.
/// The attempt would only take its launch and session-ownership locks, re-resolve its route and log
/// the same `queued` reason again. This is the claim's own check, read without locks; the claim
/// repeats it, so a stale read can only delay a launch by one pass, never admit one.
/// `store` is the drain's read-only snapshot of the board.
pub(crate) fn still_blocked(
    ctx: &Ctx,
    store: &corral_core::Store,
    task: &Task,
    rank: usize,
    at: Timestamp,
) -> bool {
    if !task.queued || task.launch.is_some() || task.dispatch_plan.is_none() {
        return false;
    }
    let admission = Admission::Queued { rank, at };
    let opts = DispatchOpts {
        queue_if_blocked: true,
        ..DispatchOpts::default()
    };
    let opts = DispatchOpts {
        plan: admission.plan_requested(task, &opts),
        ..opts
    };
    matches!(admission.reason(ctx, store, task, &opts), Ok(Some(_)))
}

/// Lock contention with another sync pass or launch clears by itself, and the next pass retries,
/// so it is not a dispatch failure worth an event.
pub(crate) fn contended(error: &Error) -> bool {
    const MARKERS: [&str; 2] = [
        "pane inventory is being reconciled",
        "dispatch is already in progress",
    ];
    error
        .chain()
        .any(|cause| MARKERS.iter().any(|m| cause.to_string().contains(m)))
}

/// Everything a persistent enqueue or dispatch refusal can depend on: the task itself (without
/// its derived attention bookkeeping), its transitive dependency graph, the configuration, the
/// discovered harness catalog that routes resolve against, and the project repository's refs,
/// HEAD and registered worktrees that bases, branches and worktrees resolve against.
fn refusal_state(ctx: &Ctx, task: &Task) -> Result<String> {
    let mut hash = Sha256::new();
    let mut record = serde_json::to_value(task).context("encoding task for refusal state")?;
    if let Some(fields) = record.as_object_mut() {
        fields.remove("attention");
        fields.remove("updated");
    }
    hash.update(serde_json::to_vec(&record)?);
    let mut pending = task.depends_on.clone();
    let mut seen = BTreeSet::new();
    while let Some(id) = pending.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        match ctx.store.get(&id) {
            Ok(dep) => {
                hash.update(format!(
                    "{id}:{}:{:?}\n",
                    dep.status.as_str(),
                    dep.depends_on
                ));
                pending.extend(dep.depends_on);
            }
            Err(_) => hash.update(format!("{id}:missing\n")),
        }
    }
    hash.update(serde_json::to_vec(&ctx.store.config).context("encoding configuration")?);
    let catalog = ctx.store.state_dir().join(crate::HARNESS_CACHE_FILE);
    match std::fs::read(&catalog) {
        Ok(bytes) => hash.update(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).with_context(|| format!("reading {}", catalog.display())),
    }
    if let Some(project) = task.project.as_deref()
        && let Ok(repo) = ctx.store.project_path(project)
    {
        for args in [
            &["for-each-ref", "--format=%(refname) %(objectname)"][..],
            &["symbolic-ref", "-q", "HEAD"],
            &["worktree", "list", "--porcelain"],
        ] {
            match std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .stdin(std::process::Stdio::null())
                .output()
            {
                Ok(out) => {
                    hash.update(out.status.code().unwrap_or(-1).to_le_bytes());
                    hash.update(&out.stdout);
                }
                Err(error) => hash.update(error.to_string()),
            }
        }
    }
    Ok(format!("{:x}", hash.finalize()))
}

/// The state each persistent refusal was logged under, shared by every syncing process.
struct Refusals {
    states: BTreeMap<String, String>,
    changed: bool,
}

impl Refusals {
    fn path(ctx: &Ctx) -> std::path::PathBuf {
        ctx.store.state_dir().join(REFUSALS)
    }

    fn load(ctx: &Ctx) -> Result<Self> {
        let path = Self::path(ctx);
        let states = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .with_context(|| format!("parsing {}", path.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
        };
        Ok(Self {
            states,
            changed: false,
        })
    }

    fn retain(&mut self, candidates: &[Task]) {
        let before = self.states.len();
        self.states
            .retain(|id, _| candidates.iter().any(|task| &task.id == id));
        self.changed |= self.states.len() != before;
    }

    fn has(&self, id: &str) -> bool {
        self.states.contains_key(id)
    }

    fn unchanged(&self, id: &str, state: &str) -> bool {
        self.states.get(id).is_some_and(|logged| logged == state)
    }

    fn record(&mut self, id: &str, state: String) {
        self.changed |= self.states.insert(id.to_owned(), state.clone()) != Some(state);
    }

    fn clear(&mut self, id: &str) {
        self.changed |= self.states.remove(id).is_some();
    }

    fn save(&self, ctx: &Ctx) -> Result<()> {
        if !self.changed {
            return Ok(());
        }
        std::fs::create_dir_all(ctx.store.state_dir()).context("creating state directory")?;
        crate::artifact_storage::replace(&Self::path(ctx), &serde_json::to_vec(&self.states)?)
    }
}
