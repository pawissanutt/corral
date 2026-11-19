//! Panel notices: what the session page says briefly in its header's state-word place (a
//! refusal, `Removed queued input · not sent`, `Messages now go to a2`, a model change), and the
//! last 50 kept with their time and exact detail for the Recent notices sheet. The host's own
//! notices (`SessionState.notices`) are shown beside this device's while their `until` holds.

use std::collections::VecDeque;

use corral_protocol::session_state::{HostNotice, SessionState};

use crate::model::hue::Hue;

/// How many notices the Recent notices sheet keeps.
pub const KEPT: usize = 50;

/// Seconds a notice holds the header's state-word place.
pub const SHOWN_FOR: u64 = 4;

/// One notice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub id: String,
    pub hue: Hue,
    pub text: String,
    /// Unix seconds it was raised; zero before the clock's first tick.
    pub at: u64,
    /// Unix seconds it stops showing in the header; none for a host notice that stays.
    pub until: Option<u64>,
}

/// This device's notices for one session, newest last.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ring {
    items: VecDeque<Notice>,
    next: u64,
}

impl Default for Ring {
    fn default() -> Self {
        Self::EMPTY
    }
}

impl Ring {
    pub const EMPTY: Ring = Ring {
        items: VecDeque::new(),
        next: 0,
    };

    /// Raises a notice at `now`; the oldest leaves once 50 are kept.
    pub fn push(&mut self, hue: Hue, text: impl Into<String>, now: u64) {
        if self.items.len() >= KEPT {
            self.items.pop_front();
        }
        self.next += 1;
        self.items.push_back(Notice {
            id: format!("notice-{}", self.next),
            hue,
            text: text.into(),
            at: now,
            until: Some(now + SHOWN_FOR),
        });
    }

    /// Newest first.
    pub fn recent(&self) -> impl Iterator<Item = &Notice> {
        self.items.iter().rev()
    }
}

/// A host notice's tone as a hue: `error` fails, `warning` holds, anything else is quiet.
fn tone(tone: &str) -> Hue {
    match tone {
        "error" => Hue::Fail,
        "warning" | "warn" => Hue::Held,
        "success" | "ok" => Hue::Ok,
        _ => Hue::Neutral,
    }
}

fn unix(at: &str) -> Option<u64> {
    let stamp: jiff::Timestamp = at.parse().ok()?;
    u64::try_from(stamp.as_second()).ok()
}

fn host(notice: &HostNotice) -> Notice {
    Notice {
        id: format!("host-{}", notice.id),
        hue: tone(&notice.tone),
        text: notice.text.clone(),
        at: 0,
        until: notice.until.as_deref().and_then(unix),
    }
}

/// The notice the header shows in its state word's place now: this device's newest one still
/// within its time, else the host's newest timed one still within its `until`.
pub fn current(ring: &Ring, state: Option<&SessionState>, now: u64) -> Option<Notice> {
    let live = |notice: &Notice| notice.until.is_some_and(|until| now < until);
    ring.recent()
        .find(|notice| live(notice))
        .cloned()
        .or_else(|| {
            state?
                .notices
                .iter()
                .rev()
                .map(host)
                .find(|notice| live(notice))
        })
}

/// The host's notices without an end: they stay in the status bar while the host lists them.
pub fn standing(state: Option<&SessionState>) -> Vec<Notice> {
    state
        .map(|state| {
            state
                .notices
                .iter()
                .filter(|notice| notice.until.is_none())
                .map(host)
                .collect()
        })
        .unwrap_or_default()
}

/// The Recent notices sheet: this device's notices, newest first, with their time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentNotices {
    pub title: &'static str,
    pub rows: Vec<Notice>,
    /// Said when there is nothing to list.
    pub empty: &'static str,
}

pub fn recent(ring: &Ring) -> RecentNotices {
    RecentNotices {
        title: "Recent notices",
        rows: ring.recent().cloned().collect(),
        empty: "No notices yet.",
    }
}
