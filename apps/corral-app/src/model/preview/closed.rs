//! Synthetic closed tasks, off the board as corrald keeps them: served a page at a time in the
//! daemon's order (most recently closed first) behind opaque anchors, each with its own task
//! detail so opening one works as it does live.

use super::{put, stamp};
use crate::model::daemon::Feed;
use corral_protocol::board::{RailGlyph, RailStep};
use corral_protocol::detail::*;

/// Shorter than the daemon's page, so the sample shows its pager.
const PAGE: usize = 4;

/// Id, title, status, hours since it closed, plan, goal; most recently closed first.
type Sample = (
    &'static str,
    &'static str,
    &'static str,
    u64,
    Option<&'static str>,
    Option<&'static str>,
);

const CLOSED: &[Sample] = &[
    (
        "T-866",
        "queue-ledger-retention",
        "done",
        2,
        Some("T-870"),
        Some("Bounded host queue ledger"),
    ),
    (
        "T-864",
        "pm-read-cache-warmup",
        "dropped",
        5,
        Some("T-893"),
        Some("PM reads stay bounded"),
    ),
    (
        "T-862",
        "Co-work C0: completion engine spike",
        "done",
        26,
        Some("T-851"),
        None,
    ),
    (
        "T-859",
        "annotation-service-prototype",
        "dropped",
        50,
        None,
        Some("Owner annotations on runs"),
    ),
    (
        "T-857",
        "api-connections-retry-budget",
        "done",
        76,
        Some("T-898"),
        None,
    ),
    ("T-848", "gate-runner-sandbox", "done", 170, None, None),
];

fn anchor(id: &str) -> String {
    format!("preview-closed:{id}")
}

fn task(feed: &Feed, &(id, title, status, hours, plan, goal): &Sample) -> ClosedTask {
    ClosedTask {
        id: id.into(),
        title: title.into(),
        status: status.into(),
        updated: stamp(feed, feed.now.saturating_sub(hours * 3600)),
        project: Some("corral".into()),
        plan: plan.map(str::to_owned),
        goal: goal.map(str::to_owned),
    }
}

/// A closed task's detail: a done task passed every stage and kept its run; a dropped one never
/// reached them.
fn detail(feed: &Feed, sample: &Sample) -> TaskDetail {
    let row = task(feed, sample);
    let done = row.status == "done";
    let glyph = if done {
        RailGlyph::Pass
    } else {
        RailGlyph::Unavailable
    };
    let ran = sample.3.saturating_add(3) * 3600;
    TaskDetail {
        id: row.id,
        owner_revision: "preview-closed-1".into(),
        title: row.title.clone(),
        status: row.status,
        body_excerpt: row
            .goal
            .unwrap_or_else(|| format!("Preview · {}", row.title)),
        stages: std::array::from_fn(|_| RailStep {
            glyph,
            sha7: None,
            at: None,
        }),
        dependencies: vec![],
        progress: None,
        owner_next: None,
        hold: None,
        park: None,
        park_revision: None,
        artifacts: vec![],
        run: done.then(|| TaskRun {
            id: "R1".into(),
            session: None,
            profile: Some("pi-sol xhigh".into()),
            model: Some("gpt-6.1-sol".into()),
            effort: Some("xhigh".into()),
            base: Some("0123456789abcdef".into()),
            head: Some("fedcba9876543210".into()),
            started_at: Some(stamp(feed, feed.now.saturating_sub(ran))),
            ended_at: Some(row.updated),
        }),
        // The sample plans' own shape (`content::board`'s fallback), so a closed task's plan
        // reads as an open one's; only a done task counts as landed.
        plan_context: sample.4.map(|plan| PlanPosition {
            plan: plan.into(),
            layer: 1,
            layers: 3,
            landed: u32::from(done),
            total: 3,
        }),
        log_tail: vec![],
    }
}

/// Every sample closed task's detail, and the newest page.
pub(super) fn populate(feed: &mut Feed) {
    for sample in CLOSED {
        let body = DetailBody::Task(detail(feed, sample));
        put(
            feed,
            DetailRef::Task {
                id: sample.0.into(),
            },
            body,
        );
    }
    let _ = page(feed, &DetailRef::Closed { before: None });
}

/// Serves a Closed page, rebuilt for whichever anchor asks; other targets are not ours.
pub(super) fn page(feed: &mut Feed, target: &DetailRef) -> Result<bool, String> {
    let DetailRef::Closed { before } = target else {
        return Ok(false);
    };
    let start = match before {
        None => 0,
        Some(before) => {
            CLOSED
                .iter()
                .position(|sample| anchor(sample.0) == *before)
                .ok_or_else(|| format!("Unknown preview closed anchor {before}"))?
                + 1
        }
    };
    let rows: Vec<ClosedTask> = CLOSED
        .iter()
        .skip(start)
        .take(PAGE)
        .map(|sample| task(feed, sample))
        .collect();
    let older = (start + PAGE < CLOSED.len())
        .then(|| rows.last().map(|row| anchor(&row.id)))
        .flatten();
    put(
        feed,
        target.clone(),
        DetailBody::Closed(ClosedPage {
            total: Some(CLOSED.len() as u64),
            rows,
            older,
        }),
    );
    Ok(true)
}
