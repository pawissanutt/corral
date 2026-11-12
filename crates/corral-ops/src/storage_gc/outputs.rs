//! One output-only reclamation service. Opt-in declarations, registered empty private roots,
//! existing task/launch exclusion, no-follow effects and the shared lifecycle retry index.
use std::path::PathBuf;

use anyhow::{Context, Result, ensure};
use corral_core::backend::{LifecycleOutcome, LifecycleRow, LifecycleWrite};
use corral_core::fs_guard::{DirHandle, Removal};
use corral_core::output::{OutputCfg, OutputKind};
use corral_core::records::RecordArea;
use corral_core::{Status, Store, TaskHead, TaskRef};
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

use super::Disposition;
use crate::Ctx;

mod files;
mod lifecycle;
mod processes;
mod protection;
pub(crate) use protection::protect_artifact_source;
#[cfg(all(test, unix))]
mod tests;
pub use lifecycle::output_pass;
pub(crate) use lifecycle::schedule;

const OWNERS: RecordArea<Owner> = RecordArea::new("output-owners");
const REGISTRATION_NOTES: RecordArea<RegistrationNote> = RecordArea::new("output-registration");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct RegistrationNote {
    run: String,
    refusal: Option<String>,
}
const PREFIX: &str = "output:";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Owner {
    version: u32,
    project: String,
    task: String,
    run: String,
    epoch: String,
    config: OutputCfg,
    worktree: PathBuf,
    path: PathBuf,
    root_dev: u64,
    root_ino: u64,
    dev: u64,
    ino: u64,
    created: Timestamp,
}

impl Owner {
    fn generation(&self) -> String {
        format!("{}:{}", self.run, self.epoch)
    }
}

fn key(task: &str, class: &str) -> String {
    format!("{task}-{class}")
}
fn target(class: &str) -> String {
    format!("{PREFIX}{class}")
}

/// Called inside the existing submission task lock, after prepare and before any consumer
/// can launch. Only empty directories or previously registered identities are accepted.
/// The caller retains the returned leases through its metadata commit and launch.
pub(crate) fn prepare(
    store: &Store,
    task: &(impl TaskRef + ?Sized),
) -> Result<Vec<corral_core::StoreLock>> {
    let mut held = Vec::new();
    let Some(project) = task.project().as_deref() else {
        return Ok(held);
    };
    let Some(classes) = store.config.storage.outputs.get(project) else {
        return Ok(held);
    };
    let Some(run) = task.current_run_head()? else {
        return Ok(held);
    };
    for (class, cfg) in classes {
        let registered = (|| -> Result<(corral_core::StoreLock, Option<Owner>)> {
            let path = files::location(store, task.id(), project, &run.worktree, cfg)?;
            let output_lease = store
                .try_lock(&protection::lease_name(&path))?
                .context("output consumer/cleanup lease is held; registration deferred")?;
            let previous = OWNERS.load(store, &key(task.id(), class))?;
            let root = DirHandle::open(&run.worktree)?;
            let exists = std::fs::symlink_metadata(&path);
            let same_directory = exists.is_ok();
            let mut nonce = [0u8; 16];
            getrandom::fill(&mut nonce).map_err(|e| anyhow::anyhow!("output generation: {e}"))?;
            let epoch: String = nonce.iter().map(|b| format!("{b:02x}")).collect();
            if previous.is_none() || !same_directory {
                protection::reserve(store, task.id(), class, &path, &epoch)?;
            }
            let output = match exists {
                Ok(_) => {
                    let output = DirHandle::open(&path)?;
                    if let Some(owner) = &previous {
                        ensure!(
                            owner.version == 1
                                && owner.task == *task.id()
                                && owner.project == project
                                && owner.config.ownership_eq(cfg)
                                && owner.worktree == run.worktree
                                && owner.path == path
                                && owner.root_dev == root.identity().dev
                                && owner.root_ino == root.identity().ino,
                            "output ownership changed; do not adopt ambiguous contents"
                        );
                        protection::validate(store, owner, class)?;
                        ensure!(
                            owner.dev == output.identity().dev
                                && owner.ino == output.identity().ino,
                            "unregistered replacement output"
                        );
                        files::private_root(&path)?;
                        if owner.run == run.id {
                            return Ok((output_lease, None));
                        }
                    } else {
                        ensure!(
                            std::fs::read_dir(&path)?.next().is_none(),
                            "unregistered nonempty output; existing contents are never adopted"
                        );
                    }
                    output
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    root.create_entry(std::path::Path::new(&cfg.path))?;
                    DirHandle::open(&path)?
                }
                Err(error) => return Err(error).context("inspect output before launch"),
            };
            files::private_root(&path)?;
            let owner = Owner {
                version: 1,
                project: project.into(),
                task: task.id().into(),
                run: run.id.clone(),
                epoch,
                config: cfg.clone(),
                worktree: run.worktree.clone(),
                path,
                root_dev: root.identity().dev,
                root_ino: root.identity().ino,
                dev: output.identity().dev,
                ino: output.identity().ino,
                created: store.now(),
            };
            Ok((output_lease, Some(owner)))
        })();
        let refusal = match registered {
            Ok((output_lease, candidate)) => {
                if let Some(owner) = candidate {
                    // Publication failures abort the caller's unit; do not commit partial provenance.
                    protection::publish(store, class, &owner)?;
                }
                held.push(output_lease);
                None
            }
            Err(error) => Some(format!("{error:#}")),
        };
        // Unsupported/shared/existing output is a reclamation refusal, not a new
        // worker/gate/review refusal. Never disturb the unfinished workflow for GC.
        let note = RegistrationNote {
            run: run.id.clone(),
            refusal,
        };
        let previous = REGISTRATION_NOTES.load(store, &key(task.id(), class))?;
        if (note.refusal.is_some() || previous.is_some()) && previous.as_ref() != Some(&note) {
            REGISTRATION_NOTES.save(store, &key(task.id(), class), &note)?;
        }
    }
    Ok(held)
}

#[derive(Debug, Clone, Serialize)]
pub struct OutputEntry {
    pub task: String,
    pub run: Option<String>,
    pub generation: Option<String>,
    pub project: String,
    pub output_class: String,
    pub kind: OutputKind,
    pub path: PathBuf,
    pub path_validated: bool,
    pub rebuildable: bool,
    pub grace_hours: u32,
    /// None means not measured, never an invented zero-byte estimate.
    pub measured_bytes: Option<u64>,
    pub reclaimed_bytes: u64,
    pub last_reclaimed_bytes: Option<u64>,
    pub disposition: Disposition,
    pub reason: String,
    pub consumer_state: String,
    pub next_check_at: Option<Timestamp>,
}

fn entry(head: &TaskHead, class: &str, cfg: &OutputCfg, owner: Option<&Owner>) -> OutputEntry {
    OutputEntry {
        task: head.id.clone(),
        run: head.current_run.as_deref().map(|r| r.id.clone()),
        generation: owner.map(Owner::generation),
        project: head.project.clone().unwrap_or_default(),
        output_class: class.into(),
        kind: cfg.kind,
        path: head
            .current_run
            .as_deref()
            .map(|r| r.worktree.join(&cfg.path))
            .unwrap_or_default(),
        path_validated: false,
        rebuildable: cfg.rebuildable,
        grace_hours: cfg.grace_hours,
        measured_bytes: None,
        reclaimed_bytes: 0,
        last_reclaimed_bytes: None,
        disposition: Disposition::Deferred,
        reason: "not assessed".into(),
        consumer_state: "unknown".into(),
        next_check_at: None,
    }
}

fn inactive_since(head: &TaskHead) -> Option<Timestamp> {
    if matches!(head.status, Status::Done | Status::Dropped) {
        return None;
    }
    head.hold
        .as_ref()
        .map(|h| h.at)
        .or_else(|| head.park.as_ref().map(|p| p.at))
        .or_else(|| {
            (head.status == Status::NeedsMe)
                .then_some(head.status_since)
                .flatten()
        })
}

fn pending(head: &TaskHead) -> Result<()> {
    ensure!(
        head.launch.is_none() && !head.queued && head.dispatch_plan.is_none(),
        "pending worker launch/build"
    );
    ensure!(
        head.review_reservation.is_none() && head.review_wait.is_none(),
        "pending gate/reviewer"
    );
    ensure!(
        head.queued_agent_gates()?.is_empty(),
        "pending native gate request"
    );
    let run = head
        .current_run
        .as_deref()
        .context("unknown run ownership")?;
    ensure!(
        run.pending_reply.is_none() && run.stuck.is_none(),
        "unknown worker delivery/process ownership"
    );
    ensure!(
        !run.gate.as_ref().is_some_and(|g| g.holds_slot()),
        "active gate"
    );
    ensure!(
        !run.review.as_ref().is_some_and(|r| r.holds_slot()) && run.facts.pending_children == 0,
        "active reviewer"
    );
    ensure!(
        head.summary.pending_plan_reviews == 0
            && head
                .last_plan
                .as_deref()
                .is_none_or(|p| p.facts.pending_children == 0),
        "pending plan reviewer"
    );
    ensure!(
        head.last_aggregate_attempt
            .as_deref()
            .is_none_or(|a| a.facts.pending_children == 0
                && !a.gate.as_ref().is_some_and(|g| g.holds_slot())),
        "active aggregate gate/reviewer"
    );
    for host in [
        run.agent_host.as_deref(),
        run.current_child
            .as_deref()
            .and_then(|r| r.agent_host.as_deref()),
    ]
    .into_iter()
    .flatten()
    {
        ensure!(
            matches!(
                host.binding.health,
                corral_core::agent::HostHealth::Lost | corral_core::agent::HostHealth::Fenced
            ),
            "worker/reviewer host can still launch build/LSP consumers"
        );
    }
    Ok(())
}

fn consumers(ctx: &Ctx, head: &TaskHead, owner: &Owner) -> Result<()> {
    if owner.config.kind == OutputKind::CargoTarget {
        files::cargo_routing(&owner.worktree)?;
    }
    ensure!(
        crate::session_scope::owns_head(ctx, head)?,
        "unknown/foreign engine ownership"
    );
    let run = head
        .current_run
        .as_deref()
        .context("unknown producing run")?;
    let pane = run
        .herdr
        .as_ref()
        .context("unknown worker pane ownership")?;
    ensure!(
        ctx.herdr.pane_get(&pane.pane_id)?.is_none(),
        "worker pane can still launch build/LSP consumers"
    );
    if let Some(pane) = run.current_child.as_deref().and_then(|r| r.herdr.as_ref()) {
        ensure!(
            ctx.herdr.pane_get(&pane.pane_id)?.is_none(),
            "reviewer pane can still consume output"
        );
    }
    use std::io::Read;
    let receipt = crate::launch_exit::receipt(ctx, &head.id, &run.id)?;
    let mut marker = Vec::new();
    corral_core::fs_guard::open_regular_nofollow(&receipt.with_extension("owner"))?
        .take(65)
        .read_to_end(&mut marker)?;
    ensure!(
        marker.len() == 64 && marker.iter().all(u8::is_ascii_hexdigit),
        "unknown producing process ownership"
    );
    processes::unused(&head.id, &owner.worktree, &|token| {
        if token == marker {
            return Ok(true);
        }
        let receipt = crate::launch_exit::process_receipt(ctx, token)?
            .context("unknown process ownership token")?;
        Ok(receipt
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with(&format!("{}-", head.id))))
    })
}

type Probe<'a> = dyn Fn(&Ctx, &TaskHead, &Owner) -> Result<()> + 'a;

fn assess(
    ctx: &Ctx,
    head: &TaskHead,
    class: &str,
    cfg: &OutputCfg,
    owner: Option<&Owner>,
    now: Timestamp,
    probe: &Probe<'_>,
) -> OutputEntry {
    let mut line = entry(head, class, cfg, owner);
    let result = (|| -> Result<()> {
        let owner = match owner {
            Some(owner) => owner,
            None => {
                let note = REGISTRATION_NOTES.load(&ctx.store, &key(&head.id, class))?;
                let refusal = note
                    .filter(|n| Some(n.run.as_str()) == line.run.as_deref())
                    .and_then(|n| n.refusal);
                anyhow::bail!(
                    "unregistered output (contents never adopted){}",
                    refusal
                        .map(|r| format!("; last registration: {r}"))
                        .unwrap_or_default()
                );
            }
        };
        files::identity(ctx, head, cfg, owner)?;
        line.path_validated = true;
        if protection::protected(&ctx.store, owner, class)? {
            line.disposition = Disposition::Retained;
            line.consumer_state =
                "registered evidence source retains this output incarnation".into();
            anyhow::bail!("output holds a registered evidence source");
        }
        let since = inactive_since(head)
            .context("not inactive, or inactivity timestamp unknown")?
            .max(owner.created);
        let due = since.checked_add(SignedDuration::from_hours(i64::from(cfg.grace_hours)))?;
        if now < due {
            line.next_check_at = Some(due);
            anyhow::bail!("inactive output retention/grace has not elapsed");
        }
        pending(head)?;
        probe(ctx, head, owner)?;
        line.consumer_state = "no active or pending consumers; launch excluded".into();
        line.measured_bytes = Some(files::contents(ctx, cfg, &owner.worktree, &owner.path)?);
        line.disposition = Disposition::Reclaimable;
        line.reason = "registered, rebuildable, private and unused after grace".into();
        Ok(())
    })();
    if let Err(error) = result {
        line.reason = format!("{error:#}");
    }
    line
}

fn save(
    ctx: &Ctx,
    owner: &Owner,
    class: &str,
    previous: Option<&LifecycleRow>,
    line: &OutputEntry,
    digest: &str,
    now: Timestamp,
) -> Result<()> {
    ctx.store.put_lifecycle(LifecycleWrite {
        task: owner.task.clone(),
        generation: owner.generation(),
        target: target(class),
        expected: previous.map(|r| r.outcome),
        policy_digest: digest.into(),
        outcome: if line.disposition == Disposition::Reclaimed {
            LifecycleOutcome::Reclaimed
        } else if line.disposition == Disposition::Retained {
            LifecycleOutcome::Retained
        } else {
            LifecycleOutcome::Deferred
        },
        reason: Some(line.reason.clone()),
        bytes: if line.disposition == Disposition::Reclaimed {
            Some(line.reclaimed_bytes)
        } else {
            None
        },
        next_check_at: if matches!(
            line.disposition,
            Disposition::Reclaimed | Disposition::Retained
        ) {
            None
        } else {
            line.next_check_at
                .or(Some(super::lifecycle::next_check(previous, now)?))
        },
        at: now,
    })
}

struct Registered<'a> {
    class: &'a str,
    cfg: &'a OutputCfg,
    owner: &'a Owner,
}

/// Caller holds this task's lock through fresh state reads, consumer observation, intent,
/// removal and settlement. All state writers and authorized launch callbacks share it.
fn reclaim(
    ctx: &Ctx,
    head: &TaskHead,
    output: Registered<'_>,
    previous: Option<&LifecycleRow>,
    now: Timestamp,
    digest: &str,
    probe: &Probe<'_>,
) -> Result<OutputEntry> {
    let Registered { class, cfg, owner } = output;
    let Some(_output) = ctx.store.try_lock(&protection::lease_name(&owner.path))? else {
        let mut line = entry(head, class, cfg, Some(owner));
        line.reason = "output consumer/registration lease is held".into();
        save(ctx, owner, class, previous, &line, digest, now)?;
        return Ok(line);
    };
    let Some(_policy) = ctx.store.try_lock("config")? else {
        let mut line = entry(head, class, cfg, Some(owner));
        line.reason = "output policy is being changed".into();
        save(ctx, owner, class, previous, &line, digest, now)?;
        return Ok(line);
    };
    let mut line = assess(ctx, head, class, cfg, Some(owner), now, probe);
    if line.disposition != Disposition::Reclaimable {
        save(ctx, owner, class, previous, &line, digest, now)?;
        return Ok(line);
    }
    line.disposition = Disposition::Deferred;
    line.reason = "reclamation intent; interrupted bytes are unconfirmed until settlement".into();
    save(ctx, owner, class, previous, &line, digest, now)?;
    let intent = ctx
        .store
        .lifecycle_row(&owner.task, &owner.generation(), &target(class))?;
    let result =
        DirHandle::open(&owner.worktree)?.remove_entry(std::path::Path::new(&cfg.path), &|| {
            // Strict current config and head, not the asynchronous pass's observations.
            let store = ctx.store.reopen_unobserved()?;
            ensure!(
                store
                    .config
                    .storage
                    .outputs
                    .get(&owner.project)
                    .and_then(|c| c.get(class))
                    == Some(cfg),
                "output policy changed before deletion"
            );
            ensure!(
                OWNERS.load(&store, &key(&head.id, class))?.as_ref() == Some(owner),
                "registered output epoch changed before deletion"
            );
            let current = store.head(&head.id)?;
            let fresh_ctx = Ctx {
                store,
                ..ctx.clone()
            };
            let fresh = assess(&fresh_ctx, &current, class, cfg, Some(owner), now, probe);
            ensure!(
                fresh.disposition == Disposition::Reclaimable,
                "output changed before deletion: {}",
                fresh.reason
            );
            Ok(())
        });
    match result {
        Ok(removal) => {
            line.reclaimed_bytes = match removal {
                Removal::Removed { bytes } => bytes,
                Removal::Absent => 0,
            };
            line.disposition = Disposition::Reclaimed;
            line.reason = if matches!(removal, Removal::Absent) {
                "already absent; interrupted deletion bytes unconfirmed"
            } else {
                "private rebuildable output reclaimed; worktree and run unchanged"
            }
            .into();
            line.next_check_at = None;
            if let Removal::Removed { bytes } = removal {
                ctx.store.log_event(Some(&head.id), "storage_reclaimed", serde_json::json!({"class":"output", "output_class":class, "run":owner.run, "generation":owner.generation(), "path":owner.path, "bytes":bytes}))?;
            }
        }
        Err(error) => {
            line.disposition = Disposition::Failed;
            line.reason = format!("{error:#}; any partial deletion bytes unconfirmed");
        }
    }
    save(ctx, owner, class, intent.as_ref(), &line, digest, now)?;
    Ok(line)
}

/// Retire disabled output policy rows without filesystem work, so disabling the feature
/// cannot pin the shared due/policy indexes ahead of finished-worktree decisions.
pub(super) fn retire_disabled(
    ctx: &Ctx,
    row: &LifecycleRow,
    digest: &str,
    now: Timestamp,
) -> Result<()> {
    if row.target.starts_with(PREFIX) && ctx.store.config.storage.outputs.is_empty() {
        lifecycle::retire(ctx, row, digest, now)?;
    }
    Ok(())
}

/// Explicit readback/apply of declared outputs, outside board/scheduling paths. Unregistered
/// and ineligible outputs are reported, never silently adopted or counted as zero bytes.
pub fn output_gc(ctx: &Ctx, apply: bool) -> Result<Vec<OutputEntry>> {
    let fresh = Ctx {
        store: ctx.store.reopen_unobserved()?,
        ..ctx.clone()
    };
    let ctx = &fresh;
    let mut lines = Vec::new();
    let now = ctx.store.now();
    let digest = super::lifecycle::policy_digest(ctx)?;
    for head in ctx
        .store
        .heads(&corral_core::backend::HeadQuery::Open { project: None })?
    {
        let Some(classes) = head
            .project
            .as_ref()
            .and_then(|p| ctx.store.config.storage.outputs.get(p))
        else {
            continue;
        };
        for (class, cfg) in classes {
            let Some(_task) = ctx.store.try_lock(&head.id)? else {
                let mut line = entry(&head, class, cfg, None);
                line.reason = "task launch/state transition holds exclusion".into();
                lines.push(line);
                continue;
            };
            let fresh = ctx.store.head(&head.id)?;
            let owner = OWNERS.load(&ctx.store, &key(&head.id, class))?;
            let row = owner
                .as_ref()
                .map(|owner| {
                    ctx.store
                        .lifecycle_row(&head.id, &owner.generation(), &target(class))
                })
                .transpose()?
                .flatten();
            let line = if let Some(row) = row
                .as_ref()
                .filter(|r| r.outcome == LifecycleOutcome::Reclaimed)
            {
                let mut line = entry(&fresh, class, cfg, owner.as_ref());
                line.disposition = Disposition::Reported;
                line.last_reclaimed_bytes = row.bytes;
                line.reason = "generation already reclaimed (cached settlement); a new launch registers a fresh epoch".into();
                line
            } else if apply && let Some(owner) = owner.as_ref() {
                reclaim(
                    ctx,
                    &fresh,
                    Registered { class, cfg, owner },
                    row.as_ref(),
                    now,
                    &digest,
                    &consumers,
                )?
            } else if owner.is_none() {
                assess(ctx, &fresh, class, cfg, None, now, &consumers)
            } else if let Some(_read_lease) = ctx.store.try_lock(&protection::lease_name(
                &owner.as_ref().context("output owner disappeared")?.path,
            ))? {
                assess(ctx, &fresh, class, cfg, owner.as_ref(), now, &consumers)
            } else {
                let mut line = entry(&fresh, class, cfg, owner.as_ref());
                line.reason = "output consumer/registration lease is held".into();
                line
            };
            lines.push(line);
        }
    }
    Ok(lines)
}
