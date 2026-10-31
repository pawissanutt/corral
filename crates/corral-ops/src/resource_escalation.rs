//! One owner alert per memory/disk pressure episode; running processes are never paused.
use std::path::Path;

use anyhow::{Context, Result};
use corral_core::records::RecordArea;
use corral_core::{Status, Store, Task};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{Change, Ctx, resources::ResourceState};

#[derive(Debug, Serialize, Deserialize)]
struct Attention {
    task: String,
    run: Option<String>,
    from: Status,
    previous_excerpt: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Alert {
    attention: Option<Attention>,
    message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    notice: Option<corral_core::NoticeClaim>,
}

/// The core publication's fresh notice capability is consumed only by the owning-sync phase.
/// Reopened old alerts can publish retained facts, never authorize another physical invocation.
#[derive(Default)]
pub(crate) struct Reconciliation {
    pub changes: Vec<Change>,
    fresh: Option<Alert>,
}

const ALERTS: RecordArea<Alert> = RecordArea::new("resource-alert");

#[derive(Debug, Serialize)]
struct TaskSize {
    task: String,
    worktree: String,
    worktree_bytes: Option<u64>,
    target_bytes: Option<u64>,
    error: Option<String>,
}

fn read(ctx: &Ctx) -> Result<Option<Alert>> {
    ALERTS.load(&ctx.store, "state")
}

fn clear_alert(store: &Store, now: Timestamp) -> Result<()> {
    store.records_transaction(|| {
        ALERTS.delete(store, "state")?;
        store.log_event(None, "resources_cleared", json!({"at": now}))
    })
}

fn publish_alert(
    store: &Store,
    alert: &Alert,
    state: &ResourceState,
    sizes: &[TaskSize],
    now: Timestamp,
) -> Result<()> {
    store.records_transaction(|| {
        ALERTS.save(store, "state", alert)?;
        store.log_event(
            alert.attention.as_ref().map(|a| a.task.as_str()),
            "escalated",
            json!({"source": "resources", "reason": alert.message, "snapshot": state.snapshot, "largest_worktrees": sizes, "at": now}),
        )
    })
}

fn save(ctx: &Ctx, alert: &Alert) -> Result<()> {
    ALERTS.save(&ctx.store, "state", alert)
}

pub(crate) fn notice(ctx: &Ctx) -> Result<Option<corral_core::NoticeClaim>> {
    Ok(read(ctx)?.and_then(|alert| alert.notice))
}

pub(crate) fn update_notice(
    ctx: &Ctx,
    update: impl FnOnce(&mut corral_core::NoticeClaim) -> Result<()>,
) -> Result<()> {
    let mut alert = read(ctx)?.context("resource alert source missing")?;
    update(
        alert
            .notice
            .as_mut()
            .context("resource alert claim missing")?,
    )?;
    save(ctx, &alert)
}

fn drain(ctx: &Ctx, alert: &Alert) -> Result<()> {
    if let Some(claim) = &alert.notice {
        let published = crate::owner_notice::drain_source(
            ctx,
            alert.attention.as_ref().map(|a| a.task.as_str()),
            &corral_core::attention::SourceNoticeRef {
                area: "resources".into(),
                key: "resource-alert".into(),
                claim_id: claim.batch_id.clone(),
            },
        )?;
        anyhow::ensure!(published, "resource notice publication is in flight");
    }
    Ok(())
}

/// Recover already-durable notice facts before another core transition can clear the source.
pub(crate) fn drain_notices(ctx: &Ctx) -> Result<()> {
    if let Some(alert) = read(ctx)? {
        drain(ctx, &alert)?;
    }
    Ok(())
}

/// Separately invoked after the core commit. Only this return's fresh source transition may
/// invoke transport; restart drains existing claims instead of resending unknown outcomes.
pub(crate) fn deliver(ctx: &Ctx, update: Reconciliation) -> Result<Vec<Change>> {
    if let Some(alert) = update.fresh {
        let subject = alert
            .attention
            .as_ref()
            .map(|attention| attention.task.as_str());
        let claim = alert.notice.context("committed resource claim missing")?;
        let reference = crate::owner_notice::register_source(
            ctx,
            subject,
            "resources",
            "resource-alert",
            &claim,
        )?;
        crate::owner_notice::notify(
            ctx,
            crate::owner_notice::source_committed(subject, reference, claim)?,
            "Machine resources need you",
            &alert.message,
            true,
        )?;
    }
    Ok(update.changes)
}

/// Keep the attention card visible even if herdr still reports its agent as working.
pub(crate) fn holds_attention(ctx: &Ctx, task: &Task) -> Result<bool> {
    Ok(read(ctx)?.is_some_and(|alert| {
        alert.attention.is_some_and(|attention| {
            attention.task == task.id
                && attention.run.as_deref() == task.current_run().map(|r| r.id.as_str())
        })
    }))
}

pub(crate) fn reconcile(
    ctx: &Ctx,
    state: &ResourceState,
    now: Timestamp,
) -> Result<Reconciliation> {
    corral_core::work::record(corral_core::work::Work::ResourcePass, 1);
    let Some(_lock) = ctx.store.try_lock("resource-alert")? else {
        return Ok(Reconciliation::default());
    };
    let previous = read(ctx)?;
    if !state.severe {
        let Some(previous) = previous else {
            return Ok(Reconciliation::default());
        };
        let mut changes = Vec::new();
        if let Some(attention) = previous.attention {
            let mut restored = false;
            ctx.store.try_update_head(&attention.task, |task| {
                if let Some(claim) = &previous.notice {
                    task.attention.source_notices.retain(|source| {
                        source.area != "resources" || source.claim_id != claim.batch_id
                    });
                }
                if task.status == Status::NeedsMe
                    && task.head().current_run.as_ref().map(|r| &r.id) == attention.run.as_ref()
                    && task.head().current_run.as_ref().is_none_or(|r| {
                        r.blocked_excerpt.as_deref() == Some(previous.message.as_str())
                    })
                {
                    task.status = attention.from;
                    if let Some(run) = task.current_run_mut() {
                        run.blocked_excerpt = attention.previous_excerpt.clone();
                    }
                    restored = true;
                }
                clear_alert(&ctx.store, now)?;
                Ok(true)
            })?;
            if restored {
                changes.push(Change {
                    task: attention.task,
                    from: Status::NeedsMe,
                    to: attention.from,
                    note: "machine memory/disk pressure cleared".into(),
                    stage: None,
                });
            }
        } else {
            clear_alert(&ctx.store, now)?;
        }
        return Ok(Reconciliation {
            changes,
            fresh: None,
        });
    }
    if previous.is_some() {
        return Ok(Reconciliation::default());
    }

    let mut tasks = ctx
        .store
        .heads(&corral_core::backend::HeadQuery::Open { project: None })?;
    let mut sizes: Vec<_> = tasks
        .iter()
        .flat_map(|task| {
            task.current_run
                .as_deref()
                .into_iter()
                .map(move |run| (task, run))
        })
        // Finished runs are cleanup candidates, not the live consumers an alert is about.
        .filter(|(_, run)| run.ended.is_none() && run.outcome.is_none() && run.worktree.is_dir())
        .map(|(task, run)| {
            corral_core::work::record(corral_core::work::Work::WorktreeWalk, 1);
            let total = directory_bytes(&run.worktree);
            let target = directory_bytes(&run.worktree.join("target"));
            TaskSize {
                task: task.id.clone(),
                worktree: run.worktree.display().to_string(),
                worktree_bytes: total.as_ref().ok().copied(),
                target_bytes: target.as_ref().ok().copied(),
                error: total
                    .err()
                    .or_else(|| target.err())
                    .map(|e| format!("{e:#}")),
            }
        })
        .collect();
    sizes.sort_by_key(|size| std::cmp::Reverse(size.worktree_bytes.unwrap_or(0)));
    sizes.truncate(5);
    let consumers = sizes
        .iter()
        .map(|size| {
            format!(
                "{}: worktree {} bytes, target {} bytes{}",
                size.task,
                size.worktree_bytes
                    .map_or_else(|| "?".into(), |n| n.to_string()),
                size.target_bytes
                    .map_or_else(|| "?".into(), |n| n.to_string()),
                size.error
                    .as_ref()
                    .map_or(String::new(), |e| format!(" ({e})"))
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    let message = format!(
        "{}\nLargest task worktrees: {}.\nInspect finished/dropped targets and leftover worktrees for cleanup.",
        state
            .reason
            .as_deref()
            .unwrap_or("machine memory/disk pressure"),
        if consumers.is_empty() {
            "none"
        } else {
            &consumers
        },
    );
    tasks.sort_by_key(|task| std::cmp::Reverse(task.updated));
    let selected = tasks
        .iter()
        // Waiting work keeps its admission identity; existing owner questions stay untouched.
        .find(|task| task.status == Status::Running);
    let attention = selected.map(|task| Attention {
        task: task.id.clone(),
        run: task.current_run.as_deref().map(|run| run.id.clone()),
        from: task.status,
        previous_excerpt: task
            .current_run
            .as_deref()
            .and_then(|run| run.blocked_excerpt.clone()),
    });
    // The existing singleton first retains the selected restore state. A stopped creator
    // never causes a subsequent pressure pass to create a second notification episode.
    let mut alert = Alert {
        attention,
        message,
        notice: None,
    };
    let mut batch = None;
    if let Some(task) = selected {
        let expected_run = alert
            .attention
            .as_ref()
            .context("selected attention missing")?
            .run
            .clone();
        ctx.store.update_head(&task.id, |current| {
            if current.status != task.status
                || current.head().current_run.as_ref().map(|r| &r.id) != expected_run.as_ref()
            {
                anyhow::bail!("task changed while raising resource alert; retry next sync");
            }
            anyhow::ensure!(current.attention.notice_outbox.len() + current.attention.source_notices.len() <= 30,
                "notification_accounting_backpressure: resource entry needs a claim and source reference");
            current.status = Status::NeedsMe;
            if let Some(run) = current.current_run_mut() { run.blocked_excerpt = Some(alert.message.clone()); }
            let pending = crate::owner_notice::record_entry_head(ctx, current, "resource_escalation", now, Vec::new())?;
            alert.notice = Some(current.attention.notice_outbox.iter()
                .find(|claim| claim.batch_id == pending.batch_id)
                .context("resource entry claim missing")?.clone());
            batch = Some(pending);
            publish_alert(&ctx.store, &alert, state, &sizes, now)?;
            Ok(())
        })?;
    } else {
        // Claim construction is pure; the singleton, claim and event publish together.
        alert.notice = Some(crate::owner_notice::prepare_subject(
            ctx,
            None,
            "resource_escalation",
            &format!("resources/{now}"),
            now,
            Vec::new(),
        )?);
        publish_alert(&ctx.store, &alert, state, &sizes, now)?;
    }
    drop(batch); // Retain reservation through the actual core commit, never transport.
    let changes = alert
        .attention
        .as_ref()
        .map(|attention| Change {
            task: attention.task.clone(),
            from: attention.from,
            to: Status::NeedsMe,
            note: alert.message.clone(),
            stage: None,
        })
        .into_iter()
        .collect();

    Ok(Reconciliation {
        changes,
        fresh: Some(alert),
    })
}

fn directory_bytes(path: &Path) -> Result<u64> {
    if !path.exists() {
        return Ok(0);
    }
    let mut pending = vec![path.to_path_buf()];
    let mut total = 0_u64;
    while let Some(path) = pending.pop() {
        let metadata = std::fs::symlink_metadata(&path)
            .with_context(|| format!("reading metadata for {}", path.display()))?;
        if metadata.is_dir() {
            for entry in
                std::fs::read_dir(&path).with_context(|| format!("listing {}", path.display()))?
            {
                pending.push(
                    entry
                        .with_context(|| format!("listing {}", path.display()))?
                        .path(),
                );
            }
        } else if metadata.is_file() {
            total = total.saturating_add(metadata.len());
        }
    }
    Ok(total)
}
