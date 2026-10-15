//! One owner alert per memory/disk pressure episode; running processes are never paused.
use std::path::Path;

use anyhow::{Context, Result};
use corral_core::{Status, Task};
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
}

#[derive(Debug, Serialize)]
struct TaskSize {
    task: String,
    worktree: String,
    worktree_bytes: Option<u64>,
    target_bytes: Option<u64>,
    error: Option<String>,
}

fn read(ctx: &Ctx) -> Result<Option<Alert>> {
    let path = ctx.store.state_dir().join("resource-alert.json");
    match std::fs::read(&path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
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
    let Some(_lock) = ctx.store.try_lock("resource-alert")? else {
        return Ok(Vec::new());
    };
    let path = ctx.store.state_dir().join("resource-alert.json");
    let previous = read(ctx)?;
    if !state.severe {
        let Some(previous) = previous else {
            return Ok(Vec::new());
        };
        let mut changes = Vec::new();
        if let Some(attention) = previous.attention {
            let written = ctx.store.try_update(&attention.task, |task| {
                if task.status != Status::NeedsMe
                    || task.current_run().map(|r| &r.id) != attention.run.as_ref()
                    || task.current_run().is_some_and(|r| {
                        r.blocked_excerpt.as_deref() != Some(previous.message.as_str())
                    })
                {
                    return Ok(false);
                }
                task.status = attention.from;
                if let Some(run) = task.current_run_mut() {
                    run.blocked_excerpt = attention.previous_excerpt.clone();
                }
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
            }
        }
        std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
        ctx.store
            .log_event(None, "resources_cleared", json!({"at": now}))?;
        return Ok(changes);
    }
    if previous.is_some() {
        return Ok(Vec::new());
    }

    let mut tasks = ctx.store.list()?;
    let mut sizes: Vec<_> = tasks
        .iter()
        .flat_map(|task| task.runs.iter().map(move |run| (task, run)))
        .filter(|(_, run)| run.worktree.is_dir())
        .map(|(task, run)| {
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
    let attention = tasks
        .into_iter()
        // Waiting work keeps its admission identity; existing owner questions stay untouched.
        .find(|task| task.status == Status::Running)
        .map(|task| {
            let attention = Attention {
                task: task.id.clone(),
                run: task.current_run().map(|run| run.id.clone()),
                from: task.status,
                previous_excerpt: task
                    .current_run()
                    .and_then(|run| run.blocked_excerpt.clone()),
            };
            ctx.store.update(&task.id, |current| {
                if current.status != task.status
                    || current.current_run().map(|r| &r.id) != attention.run.as_ref()
                {
                    anyhow::bail!("task changed while raising resource alert; retry next sync");
                }
                current.status = Status::NeedsMe;
                if let Some(run) = current.current_run_mut() {
                    run.blocked_excerpt = Some(message.clone());
                }
                Ok(())
            })?;
            Ok::<_, anyhow::Error>(attention)
        })
        .transpose()?;
    let changes = attention
        .as_ref()
        .map(|attention| Change {
            task: attention.task.clone(),
            from: attention.from,
            to: Status::NeedsMe,
            note: message.clone(),
            stage: None,
        })
        .into_iter()
        .collect();
    let alert = Alert { attention, message };
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(&alert)?)
        .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("saving {}", path.display()))?;
    ctx.store.log_event(
        alert.attention.as_ref().map(|a| a.task.as_str()),
        "escalated",
        json!({"source": "resources", "reason": alert.message, "snapshot": state.snapshot, "largest_worktrees": sizes, "at": now}),
    )?;
    if let Err(error) = ctx
        .herdr
        .notify("Machine resources need you", &alert.message, true)
    {
        crate::messages::report(
            None,
            "resources",
            format!("resource notification failed: {error:#}"),
        );
    }
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
