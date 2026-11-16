//! Closed history seeks by the last emitted key, never by an offset into growing history.
use anyhow::{Context, Result, bail, ensure};
use corral_core::backend::{AggregateName, HeadQuery, TaskNum};
use corral_core::{Status, TaskHead};
use corral_protocol::detail::{CLOSED_ANCHOR_MAX, CLOSED_PAGE_MAX, ClosedPage, ClosedTask};
use serde::{Deserialize, Serialize};

use super::short;
use crate::Ctx;

#[derive(Serialize, Deserialize)]
struct Anchor(u8, jiff::Timestamp, TaskNum);

fn decode(before: Option<&str>) -> Result<Option<Anchor>> {
    let Some(raw) = before else {
        return Ok(None);
    };
    if raw.is_empty() || raw.len() > CLOSED_ANCHOR_MAX {
        bail!("invalid closed before anchor");
    }
    let anchor = serde_json::from_str::<Anchor>(raw)
        .ok()
        .filter(|anchor| anchor.0 == 1 && anchor.2.0 > 0 && anchor.2.0 <= i64::MAX as u64);
    anchor
        .map(Some)
        .ok_or_else(|| anyhow::anyhow!("invalid closed before anchor"))
}

pub(super) fn validate(before: Option<&str>) -> Result<()> {
    decode(before).map(|_| ())
}

pub(super) fn page(ctx: &Ctx, before: Option<&str>) -> Result<ClosedPage> {
    page_with_limit(ctx, before, CLOSED_PAGE_MAX as u32)
}

pub(super) fn page_with_limit(ctx: &Ctx, before: Option<&str>, limit: u32) -> Result<ClosedPage> {
    ensure!(
        limit > 0 && u64::from(limit) <= CLOSED_PAGE_MAX,
        "invalid closed page size"
    );
    let anchor = decode(before)?;
    let stream = |status: Status| -> Result<_> {
        let query = match &anchor {
            Some(Anchor(_, updated, num)) => HeadQuery::RecentByStatusBefore {
                status: status.as_str().into(),
                updated: *updated,
                num: *num,
            },
            None => HeadQuery::RecentByStatus {
                status: status.as_str().into(),
            },
        };
        Ok(ctx
            .store
            .heads_page(&query, limit + 1)?
            .into_iter()
            .peekable())
    };
    let mut done = stream(Status::Done)?;
    let mut dropped = stream(Status::Dropped)?;
    let mut heads = Vec::with_capacity(limit as usize + 1);
    while heads.len() <= limit as usize {
        let take_done = match (done.peek(), dropped.peek()) {
            (Some(a), Some(b)) => (a.updated, a.num) > (b.updated, b.num),
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => break,
        };
        if let Some(head) = if take_done {
            done.next()
        } else {
            dropped.next()
        } {
            heads.push(head);
        }
    }
    let more = heads.len() > limit as usize;
    if more {
        heads.pop();
    }
    let older = if more {
        heads
            .last()
            .map(|head| {
                let encoded = serde_json::to_string(&Anchor(1, head.updated, head.num))?;
                ensure!(
                    encoded.len() <= CLOSED_ANCHOR_MAX,
                    "closed anchor exceeds limit"
                );
                Ok(encoded)
            })
            .transpose()?
    } else {
        None
    };
    let counts = ctx
        .store
        .view()?
        .aggregates(&[AggregateName::StatusCounts])?;
    let total = counts
        .status_counts
        .into_iter()
        .filter(|count| matches!(count.status.as_str(), "done" | "dropped"))
        .try_fold(0u64, |sum, count| sum.checked_add(count.n))
        .context("closed task count overflow")?;
    let rows = heads
        .into_iter()
        .map(|head| row(ctx, head))
        .collect::<Result<_>>()?;
    Ok(ClosedPage {
        total: Some(total),
        rows,
        older,
    })
}

fn row(ctx: &Ctx, head: TaskHead) -> Result<ClosedTask> {
    let plan = head.parent.clone().or_else(|| {
        head.plan.as_deref().map(|reference| {
            reference
                .split_once('@')
                .map_or(reference, |(id, _)| id)
                .to_owned()
        })
    });
    let parent = if head.approved_scope.is_none() {
        plan.as_deref()
            .map(|id| ctx.store.head_if_exists(id))
            .transpose()?
            .flatten()
    } else {
        None
    };
    let goal = head
        .approved_scope
        .as_ref()
        .or_else(|| {
            parent
                .as_ref()
                .and_then(|parent| parent.approved_scope.as_ref())
        })
        .map(|scope| scope.goal.chars().take(80).collect());
    Ok(ClosedTask {
        id: head.id.clone(),
        title: short(&head.title, 160),
        status: head.status.as_str().into(),
        updated: head.updated.to_string(),
        project: head.project.clone(),
        plan,
        goal,
    })
}
