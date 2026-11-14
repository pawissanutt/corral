//! Derived append-only references for global chat filing reads. Canonical filings remain
//! authoritative; only bootstrap enumerates them. A published head means every earlier slot
//! exists. Backfill batches publish the head last, so an interrupted build safely repeats.
use super::{FILINGS, Filing, filing, guard_key};
use crate::Ctx;
use anyhow::{Context, Result, ensure};
use corral_core::records::RecordArea;
use corral_protocol::detail::FILINGS_PAGE_MAX;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

const LOG: RecordArea<FilingLogEntry> = RecordArea::new("app_idea_filing_log");
const HEAD: RecordArea<FilingLogHead> = RecordArea::new("app_idea_filing_log_head");
const SHARED: &str = "shared";
const BACKFILL_BATCH: usize = 256;

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct FilingLogEntry {
    session: String,
    input: String,
}

#[derive(Serialize, Deserialize)]
struct FilingLogHead {
    next: u64,
}

#[derive(Debug)]
pub struct FilingLogPage {
    pub total: u64,
    pub entries: Vec<(u64, Filing)>,
    pub older: Option<u64>,
    pub newer: Option<u64>,
}

fn head(ctx: &Ctx) -> Result<Option<FilingLogHead>> {
    let head = HEAD.load(&ctx.store, SHARED)?;
    ensure!(
        head.as_ref().is_none_or(|head| head.next > 0),
        "corrupt chat filing log head"
    );
    Ok(head)
}

pub(super) fn ensure_filing_log(ctx: &Ctx) -> Result<()> {
    if head(ctx)?.is_some() {
        return Ok(());
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let _claim = loop {
        if let Some(claim) = ctx.store.try_lock("app-idea-filing")? {
            break claim;
        }
        ensure!(
            Instant::now() < deadline,
            "busy: idea filing is in progress"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    ensure_locked(ctx)
}

/// The caller already holds app-idea-filing. Never acquire that non-reentrant claim here.
pub(super) fn ensure_locked(ctx: &Ctx) -> Result<()> {
    if head(ctx)?.is_some() {
        return Ok(());
    }
    let mut filings = FILINGS
        .list(&ctx.store)?
        .into_iter()
        .map(|(key, filing)| {
            ensure!(
                key == guard_key(&filing.session, &filing.input),
                "corrupt canonical chat filing key"
            );
            let number = filing
                .task
                .strip_prefix("T-")
                .and_then(|n| n.parse::<u64>().ok())
                .filter(|n| *n > 0)
                .context("invalid task in canonical chat filing")?;
            corral_core::assistant::validate_id(&filing.session)?;
            Ok((number, filing))
        })
        .collect::<Result<Vec<_>>>()?;
    filings.sort_by(|(a, left), (b, right)| {
        a.cmp(b)
            .then_with(|| left.input.cmp(&right.input))
            .then_with(|| left.session.cmp(&right.session))
    });
    let next = u64::try_from(filings.len())?
        .checked_add(1)
        .context("chat filing log exhausted")?;
    for (batch_number, batch) in filings.chunks(BACKFILL_BATCH).enumerate() {
        ctx.store.records_transaction(|| {
            for (offset, (_, filing)) in batch.iter().enumerate() {
                let seq = u64::try_from(batch_number * BACKFILL_BATCH + offset)? + 1;
                let entry = FilingLogEntry {
                    session: filing.session.clone(),
                    input: filing.input.clone(),
                };
                let key = seq.to_string();
                if let Some(previous) = LOG.load(&ctx.store, &key)? {
                    ensure!(
                        previous == entry,
                        "corrupt partial chat filing log at {seq}"
                    );
                }
                LOG.save(&ctx.store, &key, &entry)?;
            }
            Ok(())
        })?;
    }
    // No canonical filing may interleave while this claim is held; only a complete prefix
    // becomes readable. Failure before this write leaves bootstrap safely retryable.
    HEAD.save(&ctx.store, SHARED, &FilingLogHead { next })
}

/// Joins the same task writer unit as FILINGS.save. The filing claim serializes appenders.
pub(super) fn append(ctx: &Ctx, session: &str, input: &str) -> Result<()> {
    let head = head(ctx)?.context("unavailable: chat filing log is not built")?;
    let next = head
        .next
        .checked_add(1)
        .context("chat filing log exhausted")?;
    let key = head.next.to_string();
    ensure!(
        LOG.load(&ctx.store, &key)?.is_none(),
        "corrupt chat filing log append position"
    );
    LOG.save(
        &ctx.store,
        &key,
        &FilingLogEntry {
            session: session.into(),
            input: input.into(),
        },
    )?;
    HEAD.save(&ctx.store, SHARED, &FilingLogHead { next })
}

pub(super) fn filing_log(ctx: &Ctx, before: Option<u64>) -> Result<FilingLogPage> {
    read_page(ctx, before, FILINGS_PAGE_MAX)
}

fn read_page(ctx: &Ctx, before: Option<u64>, page_size: u64) -> Result<FilingLogPage> {
    ensure!(before != Some(0), "invalid_params: before must be positive");
    ensure!(
        page_size > 0 && page_size <= FILINGS_PAGE_MAX,
        "invalid filing page size"
    );
    let head = head(ctx)?.context("unavailable: chat filing log is not built")?;
    let end = before.unwrap_or(head.next).min(head.next);
    let start = end.saturating_sub(page_size).max(1);
    let mut entries = Vec::with_capacity((end - start) as usize);
    for seq in (start..end).rev() {
        let entry = LOG
            .load(&ctx.store, &seq.to_string())?
            .with_context(|| format!("dangling chat filing log slot {seq}"))?;
        let filing = filing(ctx, &entry.session, &entry.input)?
            .with_context(|| format!("dangling canonical chat filing at slot {seq}"))?;
        ensure!(
            filing.session == entry.session && filing.input == entry.input,
            "corrupt canonical chat filing at slot {seq}"
        );
        entries.push((seq, filing));
    }
    Ok(FilingLogPage {
        total: head.next - 1,
        entries,
        older: (start > 1).then_some(start),
        newer: end.checked_add(page_size).filter(|next| *next < head.next),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed(ctx: &Ctx, seq: u64) {
        let input = format!("A-1/g0/i{seq}");
        FILINGS
            .save(
                &ctx.store,
                &guard_key("A-1", &input),
                &Filing {
                    task: format!("T-{seq:03}"),
                    session: "A-1".into(),
                    input: input.clone(),
                    device: None,
                    name: None,
                    words_sha256: "fixture".into(),
                },
            )
            .unwrap();
        append(ctx, "A-1", &input).unwrap();
    }

    #[test]
    fn dense_filing_pages_keep_older_anchors_stable_without_history_scan() {
        let fx = crate::agent::test_support::fx();
        ensure_filing_log(&fx.ctx).unwrap();
        for seq in 1..=5 {
            seed(&fx.ctx, seq);
        }
        let latest = read_page(&fx.ctx, None, 2).unwrap();
        assert_eq!(latest.total, 5);
        assert_eq!(
            latest
                .entries
                .iter()
                .map(|(seq, _)| *seq)
                .collect::<Vec<_>>(),
            vec![5, 4]
        );
        assert_eq!((latest.older, latest.newer), (Some(4), None));
        let middle = read_page(&fx.ctx, latest.older, 2).unwrap();
        assert_eq!((middle.older, middle.newer), (Some(2), None));
        let oldest = read_page(&fx.ctx, middle.older, 2).unwrap();
        assert_eq!(oldest.entries[0].0, 1);
        assert_eq!((oldest.older, oldest.newer), (None, Some(4)));
        seed(&fx.ctx, 6);
        let anchored = read_page(&fx.ctx, middle.older, 2).unwrap();
        assert_eq!(anchored.total, 6);
        assert_eq!(anchored.entries[0].0, 1);
        assert_eq!(anchored.newer, Some(4));
        assert!(filing_log(&fx.ctx, Some(0)).is_err());
        LOG.delete(&fx.ctx.store, "1").unwrap();
        assert!(
            read_page(&fx.ctx, Some(2), 2)
                .unwrap_err()
                .to_string()
                .contains("dangling")
        );
    }
}
