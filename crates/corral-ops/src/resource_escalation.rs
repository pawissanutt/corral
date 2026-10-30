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

/// Keep the attention card visible even if herdr still reports its agent as working.
pub(crate) fn holds_attention(ctx: &Ctx, task: &Task) -> Result<bool> {
    Ok(read(ctx)?.is_some_and(|alert| {
        alert.attention.is_some_and(|attention| {
            attention.task == task.id
                && attention.run.as_deref() == task.current_run().map(|r| r.id.as_str())
        })
    }))
}

pub(crate) fn reconcile(ctx: &Ctx, state: &ResourceState, now: Timestamp) -> Result<Vec<Change>> {
    corral_core::work::record(corral_core::work::Work::ResourcePass, 1);
    let Some(_lock) = ctx.store.try_lock("resource-alert")? else {
        return Ok(Vec::new());
    };
    let previous = read(ctx)?;
    if let Some(previous) = &previous {
        drain(ctx, previous)?;
    }
    if !state.severe {
        let Some(previous) = previous else {
            return Ok(Vec::new());
        };
        if let (Some(attention), Some(claim)) = (&previous.attention, &previous.notice) {
            ctx.store.update_attention_head(&attention.task, |ledger| {
                ledger.source_notices.retain(|source| {
                    source.area != "resources" || source.claim_id != claim.batch_id
                });
                Ok(())
            })?;
        }
        let mut changes = Vec::new();
        if let Some(attention) = previous.attention {
            let written = ctx.store.try_update_head(&attention.task, |task| {
                if task.status != Status::NeedsMe
                    || task.head().current_run.as_ref().map(|r| &r.id) != attention.run.as_ref()
                    || task.head().current_run.as_ref().is_some_and(|r| {
                        r.blocked_excerpt.as_deref() != Some(previous.message.as_str())
                    })
                {
                    return Ok(false);
                }
                task.status = attention.from;
                if let Some(run) = task.current_run_mut() {
                    run.blocked_excerpt = attention.previous_excerpt.clone();
                }
                clear_alert(&ctx.store, now)?;
                Ok(true)
            })?;
            if written.is_some() {
                changes.push(Change {
                    task: attention.task,
                    from: Status::NeedsMe,
                    to: attention.from,
                    note: "machine memory/disk pressure cleared".into(),
                    stage: None,
                });
                return Ok(changes);
            }
        }
        clear_alert(&ctx.store, now)?;
        return Ok(changes);
    }
    if previous.is_some() {
        return Ok(Vec::new());
    }

    let mut tasks = ctx
        .store
        .heads(&corral_core::backend::HeadQuery::Open { project: None })?
        .iter()
        .map(|head| head.to_shell_task())
        .collect::<Result<Vec<_>>>()?;
    let mut sizes: Vec<_> = tasks
        .iter()
        .flat_map(|task| task.runs.iter().map(move |run| (task, run)))
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
        run: task.current_run().map(|run| run.id.clone()),
        from: task.status,
        previous_excerpt: task
            .current_run()
            .and_then(|run| run.blocked_excerpt.clone()),
    });
    // The existing singleton first retains the selected restore state. A stopped creator
    // never causes a subsequent pressure pass to create a second notification episode.
    let mut alert = Alert {
        attention,
        message,
        notice: None,
    };
    let subject = selected.map(|task| task.id.as_str());
    let mut batch = None;
    let claim = if let Some(task) = selected {
        let attention = alert
            .attention
            .as_ref()
            .context("selected attention missing")?;
        let saved = ctx.store.update_head(&task.id, |current| {
            if current.status != task.status
                || current.head().current_run.as_ref().map(|r| &r.id) != attention.run.as_ref()
            {
                anyhow::bail!("task changed while raising resource alert; retry next sync");
            }
            anyhow::ensure!(current.attention.notice_outbox.len() + current.attention.source_notices.len() <= 30,
                "notification_accounting_backpressure: resource entry needs a claim and source reference");
            current.status = Status::NeedsMe;
            if let Some(run) = current.current_run_mut() {
                run.blocked_excerpt = Some(alert.message.clone());
            }
            batch = Some(crate::owner_notice::record_entry_head(
                ctx,
                current,
                "resource_escalation",
                now,
                Vec::new(),
            )?);
            publish_alert(&ctx.store, &alert, state, &sizes, now)?;
            Ok(())
        })?;
        saved
            .attention
            .notice_outbox
            .iter()
            .find(|claim| Some(&claim.batch_id) == batch.as_ref().map(|pending| &pending.batch_id))
            .context("saved resource entry claim missing")?
            .clone()
    } else {
        publish_alert(&ctx.store, &alert, state, &sizes, now)?;
        crate::owner_notice::prepare_subject(
            ctx,
            None,
            "resource_escalation",
            &format!("resources/{now}"),
            now,
            Vec::new(),
        )?
    };
    let reference =
        crate::owner_notice::register_source(ctx, subject, "resources", "resource-alert", &claim)?;
    alert.notice = Some(claim.clone());
    save(ctx, &alert)?;
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

    crate::owner_notice::notify(
        ctx,
        crate::owner_notice::source_committed(subject, reference, claim)?,
        "Machine resources need you",
        &alert.message,
        true,
    )?;
    Ok(changes)
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
