//! What the clock moves on the Agents tab, kept apart from what its controls are built from: each
//! agent's running time, where each timeline bar starts and ends on the axis and how long the
//! axis is, and each job's elapsed time. Everything else on the tab is equal across a tick, so
//! its rows and buttons stay mounted while these words and bars update in place.

use std::collections::BTreeMap;

use super::super::Cx;
use super::super::activity::duration;
use super::{AgentsTab, fleet, jobs, time};

/// What the clock moves.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AgentsClock {
    /// Each agent's running time (`1m 02s`, `waiting`, `–`), by agent id.
    pub times: BTreeMap<String, String>,
    /// Each timeline bar's start and end, as fractions of the axis, by agent id.
    pub bars: BTreeMap<String, (f32, f32)>,
    /// The timeline axis's length (`4m 12s`).
    pub axis_end: Option<String>,
    /// Each job's elapsed time (`38s`), by job id; absent without a start.
    pub jobs: BTreeMap<String, String>,
}

impl AgentsClock {
    /// Agent `child`'s running time; `–` when the fleet does not list it.
    pub fn time(&self, child: &str) -> String {
        self.times.get(child).cloned().unwrap_or_else(|| "–".into())
    }

    /// Job `job`'s elapsed time, when it has a start.
    pub fn job(&self, job: &str) -> Option<&str> {
        self.jobs.get(job).map(String::as_str)
    }
}

impl AgentsTab {
    /// The tab without what the clock moves: what its rows and controls are built from.
    pub fn steady(&self) -> AgentsTab {
        AgentsTab {
            clock: AgentsClock::default(),
            ..self.clone()
        }
    }
}

/// The clock's part of the tab of the session `cx` reads.
pub(super) fn clock(cx: &Cx<'_>) -> AgentsClock {
    let now = cx.feed.now;
    let mut out = AgentsClock {
        jobs: jobs::elapsed_by_job(cx),
        ..AgentsClock::default()
    };
    let Some(fleet) = fleet(cx) else {
        return out;
    };
    out.times = fleet
        .children
        .iter()
        .map(|child| (child.id.clone(), time(child, now)))
        .collect();
    let Some(first) = fleet
        .children
        .iter()
        .filter_map(|child| child.started_at)
        .min()
    else {
        return out;
    };
    let last = fleet
        .children
        .iter()
        .filter_map(|child| child.ended_at)
        .chain((now > 0).then_some(now.saturating_mul(1000)))
        .max()
        .unwrap_or(first)
        .max(first + 1);
    let span = (last - first) as f32;
    let at = |ms: u64| (ms.saturating_sub(first)) as f32 / span;
    out.bars = fleet
        .children
        .iter()
        .filter_map(|child| {
            let start = child.started_at?;
            let end = child.ended_at.unwrap_or(last);
            Some((child.id.clone(), (at(start), at(end).max(at(start)))))
        })
        .collect();
    out.axis_end = Some(duration((last - first) / 1000));
    out
}
