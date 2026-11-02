//! Finished-reviewer shutdown: actual live ownership and durable verdict evidence precede signals.
use crate::Ctx;
use anyhow::{Context, Result};
use corral_core::TaskRef;
use jiff::Timestamp;
use std::collections::{BTreeMap, BTreeSet};

/// Current runtime input, never an inventory of retained launch records or task history.
#[cfg(target_os = "linux")]
fn process_markers() -> Result<BTreeSet<Vec<u8>>> {
    let mut markers = BTreeSet::new();
    for entry in std::fs::read_dir("/proc").context("listing reviewer process ownership")? {
        let entry = entry?;
        if entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
            .is_none()
        {
            continue;
        }
        let Ok(env) = std::fs::read(entry.path().join("environ")) else {
            continue;
        };
        for value in env.split(|byte| *byte == 0) {
            if let Some(marker) = value.strip_prefix(b"CORRAL_PROCESS_OWNER=")
                && marker.len() == 64
                && marker.iter().all(u8::is_ascii_hexdigit)
            {
                markers.insert(marker.to_vec());
            }
        }
    }
    Ok(markers)
}
#[cfg(not(target_os = "linux"))]
fn process_markers() -> Result<BTreeSet<Vec<u8>>> {
    Ok(BTreeSet::new())
}

/// T-661 is ONLY this unindexed event-derived gate. Match any timestamp, not the newest
/// sequence: wall time can move backward. Existing task/kind/run filters narrow every page.
fn durable_verdict(
    ctx: &Ctx,
    task: &str,
    run: &str,
    reviewer: &str,
    at: Timestamp,
) -> Result<bool> {
    let _cost = corral_core::work::followup("T-661", "cleanup.verdict_event");
    let mut after = 0;
    loop {
        let page = ctx
            .store
            .events_page(Some(task), &["verdict"], Some(run), after, 256)?;
        if page.iter().any(|event| {
            event.detail["reviewer"].as_str() == Some(reviewer)
                && match event.detail["review_at"].as_str() {
                    Some(stamp) => stamp == at.to_string(),
                    None => event.ts >= at,
                }
        }) {
            return Ok(true);
        }
        let Some(last) = page.last() else {
            return Ok(false);
        };
        after = last.seq;
        if page.len() < 256 {
            return Ok(false);
        }
    }
}

pub(crate) fn reap_closed_reviews<T: TaskRef + ?Sized>(ctx: &Ctx, source: &T) {
    reap(ctx, Some(source.id()));
}

/// Owning-sync retry net. Discover actual marked residents once, not all retained tasks.
pub(crate) fn reap_current_reviews(ctx: &Ctx) {
    reap(ctx, None);
}

fn reap(owner_ctx: &Ctx, selected: Option<&str>) {
    let result: Result<()> = (|| {
        // One view also makes absent marker lookups independent of unrelated live processes.
        let ctx = Ctx {
            store: owner_ctx.store.read_only_snapshot()?,
            ..owner_ctx.clone()
        };
        let mut tasks: BTreeMap<String, BTreeSet<(String, String)>> = BTreeMap::new();
        for marker in process_markers()? {
            let Some(receipt) = crate::launch_exit::process_receipt(&ctx, &marker)? else {
                continue;
            };
            let Some((number, attempt)) = receipt
                .file_stem()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_prefix("T-"))
                .and_then(|name| name.split_once('-'))
            else {
                continue;
            };
            if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            let task = format!("T-{number}");
            if selected.is_some_and(|id| task != id) {
                continue;
            }
            let Some((run, child)) = attempt.split_once('-') else {
                continue;
            };
            tasks
                .entry(task)
                .or_default()
                .insert((run.to_owned(), child.to_owned()));
        }
        for (task, candidates) in tasks {
            if let Err(error) = reap_candidates(owner_ctx, &ctx, &task, candidates) {
                super::failed(owner_ctx, &task, "review process ownership", &error);
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        match selected {
            Some(task) => super::failed(owner_ctx, task, "review process ownership", &error),
            None => crate::messages::report(
                None,
                "cleanup",
                format!("review process ownership: {error:#}"),
            ),
        }
    }
}

fn reap_candidates(
    owner_ctx: &Ctx,
    ctx: &Ctx,
    task: &str,
    candidates: BTreeSet<(String, String)>,
) -> Result<()> {
    let head = ctx.store.head(task)?;
    if !crate::session_scope::owns_head(ctx, &head)? {
        return Ok(());
    }
    for (run, child) in candidates {
        let Some(review_child) = head
            .review_child(&run, &child)?
            .filter(|child| child.ended.is_some())
        else {
            continue;
        };
        if let Some(review) = review_child
            .review
            .as_ref()
            .filter(|review| review.verdict.is_some())
            && !durable_verdict(ctx, task, &run, &review.reviewer, review.at)?
        {
            continue;
        }
        if let Err(error) = crate::cleanup_worktree::reap_review(ctx, task, &run, &child) {
            super::failed(owner_ctx, task, &format!("review {run}-{child}"), &error);
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "reap_tests.rs"]
mod tests;
