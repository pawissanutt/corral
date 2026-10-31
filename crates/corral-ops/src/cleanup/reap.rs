//! Finished-reviewer shutdown: actual live ownership and durable verdict evidence precede signals.
use crate::Ctx;
use anyhow::{Context, Result};
use corral_core::TaskRef;
use jiff::Timestamp;
use std::collections::BTreeSet;

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
        if page
            .iter()
            .any(|event| event.detail["reviewer"].as_str() == Some(reviewer) && event.ts >= at)
        {
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
    let result: Result<()> = (|| {
        // One view also makes absent marker lookups independent of unrelated live processes:
        // no repeated view opens and no decoded historical container inventory.
        let owner_ctx = ctx;
        let ctx = Ctx {
            store: ctx.store.read_only_snapshot()?,
            ..ctx.clone()
        };
        let prefix = format!("{}-", source.id());
        let mut candidates = BTreeSet::new();
        for marker in process_markers()? {
            let Some(receipt) = crate::launch_exit::process_receipt(&ctx, &marker)? else {
                continue;
            };
            let Some(attempt) = receipt
                .file_stem()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_prefix(&prefix))
            else {
                continue;
            };
            let Some((run, child)) = attempt.split_once('-') else {
                continue;
            };
            candidates.insert((run.to_owned(), child.to_owned()));
        }
        if candidates.is_empty() {
            return Ok(());
        }
        let head = ctx.store.head(source.id())?;
        if !crate::session_scope::owns_head(&ctx, &head)? {
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
                && !durable_verdict(&ctx, source.id(), &run, &review.reviewer, review.at)?
            {
                continue;
            }
            if let Err(error) =
                crate::cleanup_worktree::reap_review(&ctx, source.id(), &run, &child)
            {
                super::failed(
                    owner_ctx,
                    source.id(),
                    &format!("review {run}-{child}"),
                    &error,
                );
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        super::failed(ctx, source.id(), "review process ownership", &error);
    }
}

#[cfg(test)]
#[path = "reap_tests.rs"]
mod tests;
