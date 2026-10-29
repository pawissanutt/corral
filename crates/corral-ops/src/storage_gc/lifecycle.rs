//! The sync-time lifecycle pass over finished worktrees (plan T-253 section 10). Rows are keyed
//! `(task, generation, target)`: the generation is the run that owned the worktree, so a task
//! reopened and finished again gets a new row. Each pass is bounded by new work, never history:
//! tasks that left the open board since the pass's change-feed cursor, then at most one batch of
//! deferred rows due by now (`lifecycle_due`), then at most one batch of rows recorded under
//! another policy digest (`lifecycle_policy`). A row already recorded under the current digest
//! and not due costs no filesystem work.

use std::time::Duration;

use anyhow::Result;
use corral_core::backend::{ChangeSeq, LifecycleOutcome, LifecycleRow, LifecycleWrite};
use corral_core::records::RecordArea;
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

use super::worktrees::{self, Assessment, Finding, TARGET};
use crate::Ctx;

const LEASE: &str = "lifecycle-gc";
const BATCH: u32 = 16;
const FIRST_DEFER: Duration = Duration::from_secs(60);
const MAX_DEFER: Duration = Duration::from_secs(3600);

/// The change-feed position, zero-padded so its stored size never grows with history.
#[derive(Serialize, Deserialize)]
struct Cursor {
    seq: String,
}

impl Cursor {
    fn at(seq: u64) -> Cursor {
        Cursor {
            seq: format!("{seq:020}"),
        }
    }
}

const CURSOR: RecordArea<Cursor> = RecordArea::new("lifecycle-gc");

/// The digest of the cleanup policy every lifecycle decision depends on: the effective
/// `remove_worktree` of the workspace and of each configured project.
pub(crate) fn policy_digest(ctx: &Ctx) -> Result<String> {
    let config = &ctx.store.config;
    let projects: Vec<(&String, bool)> = config
        .projects
        .keys()
        .map(|name| (name, config.review_policy(Some(name), None).remove_worktree))
        .collect();
    let policy = serde_json::json!({
        "version": 1,
        "remove_worktree": config.review_policy(None, None).remove_worktree,
        "projects": projects,
    });
    Ok(crate::verification::digest(&serde_json::to_vec(&policy)?))
}

/// One bounded pass at `now`; another process's running pass makes it a no-op.
pub fn lifecycle_pass(ctx: &Ctx, now: Timestamp) -> Result<()> {
    let Some(_lease) = ctx.store.try_lock(LEASE)? else {
        return Ok(());
    };
    let digest = policy_digest(ctx)?;
    for task in newly_left(ctx)? {
        if let Err(error) = settle(ctx, &task, None, &digest, now) {
            report(&task, &error);
        }
    }
    for row in ctx.store.lifecycle_due(now, BATCH)? {
        if let Err(error) = settle(ctx, &row.task, Some(&row), &digest, now) {
            report(&row.task, &error);
        }
    }
    for row in ctx.store.lifecycle_policy_other_than(&digest, BATCH)? {
        if let Err(error) = settle(ctx, &row.task, Some(&row), &digest, now) {
            report(&row.task, &error);
        }
    }
    Ok(())
}

fn report(task: &str, error: &anyhow::Error) {
    crate::messages::report(
        Some(task),
        "lifecycle",
        format!("worktree lifecycle: {error:#}"),
    );
}

/// Tasks that left the open board since the saved cursor. The first pass, and a pass whose
/// cursor fell behind the change feed's retention, starts at the current position.
fn newly_left(ctx: &Ctx) -> Result<Vec<String>> {
    let saved = CURSOR.load(&ctx.store, "board")?;
    let start = |ctx: &Ctx| -> Result<Vec<String>> {
        let seq = ctx.store.view()?.seq().0;
        CURSOR.save(&ctx.store, "board", &Cursor::at(seq))?;
        Ok(Vec::new())
    };
    let Some(saved) = saved else {
        return start(ctx);
    };
    let board = match ctx.store.board(Some(ChangeSeq(saved.seq.parse()?))) {
        Ok(board) => board,
        Err(error) if is_gap(&error) => return start(ctx),
        Err(error) => return Err(error),
    };
    // Only task changes move the cursor; its own save is not one.
    if !board.left.is_empty() || !board.heads.is_empty() {
        CURSOR.save(&ctx.store, "board", &Cursor::at(board.seq.0))?;
    }
    Ok(board.left)
}

fn is_gap(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<corral_core::backend::FeedError>()
            .is_some_and(|e| matches!(e, corral_core::backend::FeedError::Gap { .. }))
    })
}

/// Brings `task`'s current worktree row up to date. `due` is the row a due or policy batch
/// selected; a row of an older generation or of a reopened task leaves the batches as retained.
fn settle(
    ctx: &Ctx,
    task: &str,
    due: Option<&LifecycleRow>,
    digest: &str,
    now: Timestamp,
) -> Result<()> {
    let head = ctx.store.head(task)?;
    let finished = matches!(
        head.status,
        corral_core::Status::Done | corral_core::Status::Dropped
    );
    let current = head.current_run.as_deref().map(|run| run.id.as_str());
    if let Some(row) = due
        && (!finished || current != Some(row.generation.as_str()))
    {
        // Only a deferred row is still owed work; any other keeps its outcome under the digest.
        let decision = match row.outcome {
            LifecycleOutcome::Deferred if finished => retain("superseded by a newer run"),
            LifecycleOutcome::Deferred => retain("task reopened"),
            outcome => Decision {
                outcome,
                reason: row.reason.clone(),
                bytes: row.bytes,
                next_check_at: None,
            },
        };
        return write(ctx, row, Some(row.outcome), decision, digest, now);
    }
    let (true, Some(generation)) = (finished, current) else {
        return Ok(());
    };
    let stored = match due {
        Some(row) => Some(row.clone()),
        None => ctx
            .store
            .lifecycle_rows(task)?
            .into_iter()
            .find(|row| row.generation == generation && row.target == TARGET),
    };
    if let Some(row) = &stored {
        let due_now = row.outcome == LifecycleOutcome::Deferred
            && row.next_check_at.is_none_or(|at| at <= now);
        if row.policy_digest == digest && !due_now {
            return Ok(());
        }
    }
    let Some(found) = worktrees::assess(ctx, &head)? else {
        return Ok(());
    };
    record(ctx, &found, stored.as_ref(), digest, now, "sync").map(drop)
}

fn retain(reason: &str) -> Decision {
    Decision {
        outcome: LifecycleOutcome::Retained,
        reason: Some(reason.into()),
        bytes: None,
        next_check_at: None,
    }
}

struct Decision {
    outcome: LifecycleOutcome,
    reason: Option<String>,
    bytes: Option<u64>,
    next_check_at: Option<Timestamp>,
}

fn write(
    ctx: &Ctx,
    key: &LifecycleRow,
    expected: Option<LifecycleOutcome>,
    decision: Decision,
    digest: &str,
    now: Timestamp,
) -> Result<()> {
    ctx.store.put_lifecycle(LifecycleWrite {
        task: key.task.clone(),
        generation: key.generation.clone(),
        target: key.target.clone(),
        expected,
        outcome: decision.outcome,
        policy_digest: digest.to_owned(),
        reason: decision.reason,
        bytes: decision.bytes,
        next_check_at: decision.next_check_at,
        at: now,
    })
}

/// Acts on an assessment (removing a reclaimable worktree), records the outcome and returns it
/// with its reason.
pub(crate) fn record(
    ctx: &Ctx,
    found: &Assessment,
    stored: Option<&LifecycleRow>,
    digest: &str,
    now: Timestamp,
    via: &str,
) -> Result<(LifecycleOutcome, Option<String>)> {
    let decision = match &found.finding {
        Finding::Reclaim { bytes, absent, .. } => match worktrees::reclaim(ctx, found, via) {
            Ok(()) => Decision {
                outcome: LifecycleOutcome::Reclaimed,
                reason: absent.then(|| "already absent".to_owned()),
                bytes: Some(match stored {
                    Some(row) if *absent && row.outcome == LifecycleOutcome::Reclaimed => {
                        row.bytes.unwrap_or(0)
                    }
                    _ => *bytes,
                }),
                next_check_at: None,
            },
            Err(error) => {
                ctx.store.log_event(
                    Some(&found.task),
                    "storage_reclaim_failed",
                    serde_json::json!({
                        "class": "worktree",
                        "run": found.generation,
                        "path": found.path,
                        "error": format!("{error:#}"),
                    }),
                )?;
                Decision {
                    outcome: LifecycleOutcome::Failed,
                    reason: Some(format!("{error:#}")),
                    bytes: None,
                    next_check_at: None,
                }
            }
        },
        Finding::Retain(reason) => retain(reason),
        Finding::Defer(reason) => Decision {
            outcome: LifecycleOutcome::Deferred,
            reason: Some(reason.clone()),
            bytes: None,
            next_check_at: Some(next_check(stored, now)?),
        },
    };
    let key = LifecycleRow {
        task: found.task.clone(),
        generation: found.generation.clone(),
        target: TARGET.into(),
        policy_digest: String::new(),
        outcome: decision.outcome,
        reason: None,
        bytes: None,
        next_check_at: None,
        at: now,
    };
    let result = (decision.outcome, decision.reason.clone());
    write(
        ctx,
        &key,
        stored.map(|row| row.outcome),
        decision,
        digest,
        now,
    )?;
    Ok(result)
}

/// The next check of a deferred row: one minute, doubling up to an hour.
fn next_check(stored: Option<&LifecycleRow>, now: Timestamp) -> Result<Timestamp> {
    let delay = match stored {
        Some(row) if row.outcome == LifecycleOutcome::Deferred => row
            .next_check_at
            .map_or(FIRST_DEFER, |at| {
                at.duration_since(row.at).unsigned_abs() * 2
            })
            .clamp(FIRST_DEFER, MAX_DEFER),
        _ => FIRST_DEFER,
    };
    Ok(now.checked_add(SignedDuration::try_from(delay)?)?)
}
