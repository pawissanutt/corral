//! Bounded, replayable output decisions on the existing change feed and lifecycle due index.
//! Sync only schedules this worker; byte/process walks never run in its hot scope.
use std::collections::BTreeSet;
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use corral_core::backend::{ChangeSeq, LifecycleOutcome, LifecycleRow, LifecycleWrite};
use corral_core::records::RecordArea;
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

use super::{OWNERS, PREFIX, consumers, inactive_since, key, reclaim, target};
use crate::Ctx;

const BATCH: u32 = 16;
const CURSOR: RecordArea<Cursor> = RecordArea::new("output-lifecycle");

#[derive(Default, Serialize, Deserialize)]
struct Cursor {
    seq: u64,
    policy: String,
    pending: BTreeSet<(String, String)>,
}

/// Cheap async hook. A one-minute throttle coalesces short questions and ticks; it is kept
/// beside the schedule lock, not in the store, so an idle schedule appends nothing to the
/// change feed. Crash/restart merely postpones the next bounded pass. The child's own lease
/// excludes overlap.
pub(crate) fn schedule(ctx: &Ctx, now: Timestamp) -> Result<()> {
    let _span = corral_core::timing::span("sync.outputs_schedule");
    if ctx.store.config.storage.outputs.is_empty() {
        return Ok(());
    }
    let Some(_schedule) = ctx.store.try_lock("output-lifecycle-schedule")? else {
        return Ok(());
    };
    let path = ctx.store.dir().join("lock/output-lifecycle-schedule.next");
    let next = match std::fs::read_to_string(&path) {
        Ok(text) => text.trim().parse::<Timestamp>().ok(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    if next.is_some_and(|next| next > now) {
        return Ok(());
    }
    let next = now.checked_add(SignedDuration::from_secs(60))?;
    std::fs::write(&path, next.to_string())
        .with_context(|| format!("writing {}", path.display()))?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("--root")
        .arg(ctx.store.root())
        .args(["storage", "outputs", "--due"])
        .current_dir(ctx.store.root())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = command.spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

fn discover(ctx: &Ctx, digest: &str) -> Result<Cursor> {
    let mut cursor = CURSOR.load(&ctx.store, "board")?.unwrap_or_default();
    let since =
        (!cursor.policy.is_empty() && cursor.policy == digest).then_some(ChangeSeq(cursor.seq));
    // Only task changes, a new policy or an unfollowable feed move the cursor; its own saves
    // are not task changes.
    let (board, moved) = match ctx.store.board(since) {
        Ok(board) => {
            let moved = since.is_none() || !board.heads.is_empty() || !board.left.is_empty();
            (board, moved)
        }
        Err(error)
            if error.chain().any(|e| {
                matches!(
                    e.downcast_ref::<corral_core::backend::FeedError>(),
                    Some(corral_core::backend::FeedError::Gap { .. })
                )
            }) =>
        {
            (ctx.store.board(None)?, true)
        }
        Err(error) => return Err(error),
    };
    for head in board.heads {
        if head.current_run.is_none() {
            continue;
        }
        if let Some(classes) = head
            .project
            .as_ref()
            .and_then(|p| ctx.store.config.storage.outputs.get(p))
        {
            cursor
                .pending
                .extend(classes.keys().map(|class| (head.id.clone(), class.clone())));
        }
    }
    cursor.seq = board.seq.0;
    cursor.policy = digest.into();
    // Discover before acting: an interrupted pass owes exactly the same pending decisions.
    if moved {
        CURSOR.save(&ctx.store, "board", &cursor)?;
    }
    Ok(cursor)
}

pub(super) fn retire(ctx: &Ctx, row: &LifecycleRow, digest: &str, now: Timestamp) -> Result<()> {
    ctx.store.put_lifecycle(LifecycleWrite {
        task: row.task.clone(),
        generation: row.generation.clone(),
        target: row.target.clone(),
        expected: Some(row.outcome),
        outcome: if row.outcome == LifecycleOutcome::Deferred {
            LifecycleOutcome::Retained
        } else {
            row.outcome
        },
        policy_digest: digest.into(),
        reason: if row.outcome == LifecycleOutcome::Deferred {
            Some("output generation superseded, undeclared, or task no longer inactive".into())
        } else {
            row.reason.clone()
        },
        bytes: row.bytes,
        next_check_at: None,
        at: now,
    })
}

pub(super) fn settle(
    ctx: &Ctx,
    task: &str,
    class: &str,
    due: Option<&LifecycleRow>,
    digest: &str,
    now: Timestamp,
) -> Result<bool> {
    let Some(_task) = ctx.store.try_lock(task)? else {
        // A deferral needs no deletion authority. Move busy work into the existing due
        // index so it cannot pin the pending queue or starve other storage classes.
        if let Some(owner) = OWNERS.load(&ctx.store, &key(task, class))? {
            if let Some(due) = due
                && due.generation != owner.generation()
            {
                retire(ctx, due, digest, now)?;
                return Ok(true);
            }
            let row = ctx
                .store
                .lifecycle_row(task, &owner.generation(), &target(class))?;
            if row.as_ref().is_some_and(|r| {
                r.policy_digest == digest && r.next_check_at.is_some_and(|at| at > now)
            }) {
                return Ok(true);
            }
            if let Some(row) = row
                .as_ref()
                .filter(|r| r.outcome == LifecycleOutcome::Reclaimed)
            {
                if row.policy_digest != digest {
                    retire(ctx, row, digest, now)?;
                }
            } else {
                let next = row
                    .as_ref()
                    .and_then(|r| r.next_check_at)
                    .filter(|at| *at > now)
                    .unwrap_or(super::super::lifecycle::next_check(row.as_ref(), now)?);
                ctx.store.put_lifecycle(LifecycleWrite {
                    task: task.into(),
                    generation: owner.generation(),
                    target: target(class),
                    expected: row.as_ref().map(|r| r.outcome),
                    outcome: LifecycleOutcome::Deferred,
                    policy_digest: digest.into(),
                    reason: Some("task launch/state transition holds exclusion".into()),
                    bytes: None,
                    next_check_at: Some(next),
                    at: now,
                })?;
            }
        } else if let Some(row) = due {
            retire(ctx, row, digest, now)?;
        }
        return Ok(true);
    };
    let head = ctx.store.head(task)?;
    let owner = OWNERS.load(&ctx.store, &key(task, class))?;
    let cfg = head
        .project
        .as_ref()
        .and_then(|p| ctx.store.config.storage.outputs.get(p))
        .and_then(|c| c.get(class));
    let Some((owner, cfg)) = owner.as_ref().zip(cfg) else {
        if let Some(row) = due {
            retire(ctx, row, digest, now)?;
        }
        return Ok(true);
    };
    if let Some(row) = due
        && row.generation != owner.generation()
    {
        retire(ctx, row, digest, now)?;
        return Ok(true);
    }
    let row = ctx
        .store
        .lifecycle_row(task, &owner.generation(), &target(class))?;
    if inactive_since(&head).is_none() {
        if let Some(row) = row
            .as_ref()
            .filter(|r| r.outcome == LifecycleOutcome::Deferred)
        {
            retire(ctx, row, digest, now)?;
        }
        return Ok(true);
    }
    if let Some(row) = &row
        && row.policy_digest == digest
        && (row.outcome == LifecycleOutcome::Reclaimed
            || (row.outcome == LifecycleOutcome::Deferred
                && row.next_check_at.is_some_and(|at| at > now)))
    {
        return Ok(true);
    }
    if row
        .as_ref()
        .is_some_and(|r| r.policy_digest == digest && r.outcome == LifecycleOutcome::Retained)
        && super::protection::protected(&ctx.store, owner, class)?
    {
        return Ok(true);
    }
    reclaim(
        ctx,
        &head,
        super::Registered { class, cfg, owner },
        row.as_ref(),
        now,
        digest,
        &consumers,
    )?;
    Ok(true)
}

/// One bounded asynchronous lifecycle pass. The same service and policy backs explicit GC.
pub fn output_pass(ctx: &Ctx, now: Timestamp) -> Result<()> {
    let fresh = Ctx {
        store: ctx.store.reopen_unobserved()?,
        ..ctx.clone()
    };
    let ctx = &fresh;
    let Some(_pass) = ctx.store.try_lock("output-lifecycle")? else {
        return Ok(());
    };
    let digest = super::super::lifecycle::policy_digest(ctx)?;
    let mut cursor = discover(ctx, &digest)?;
    let before = cursor.pending.len();
    let pending: Vec<_> = cursor
        .pending
        .iter()
        .take(BATCH as usize)
        .cloned()
        .collect();
    for (task, class) in pending {
        match settle(ctx, &task, &class, None, &digest, now) {
            Ok(true) => {
                cursor.pending.remove(&(task, class));
            }
            Ok(false) => {}
            Err(error) => {
                crate::messages::report(Some(&task), "output lifecycle", format!("{error:#}"))
            }
        }
    }
    if cursor.pending.len() != before {
        CURSOR.save(&ctx.store, "board", &cursor)?;
    }
    for row in ctx
        .store
        .lifecycle_due(now, BATCH)?
        .into_iter()
        .chain(ctx.store.lifecycle_policy_other_than(&digest, BATCH)?)
    {
        if let Some(class) = row.target.strip_prefix(PREFIX)
            && let Err(error) = settle(ctx, &row.task, class, Some(&row), &digest, now)
        {
            crate::messages::report(Some(&row.task), "output lifecycle", format!("{error:#}"));
        }
    }
    Ok(())
}
