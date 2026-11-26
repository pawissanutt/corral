//! Panel notices: what the session page says in its header's state-word place (a refusal,
//! `Removed queued input · not sent`, `Messages now go to a2`, a model change, an export's
//! result), and the last 50 kept with their time and exact detail for the Recent notices sheet.
//! A notice holds that place for at least `SHOWN_FOR` seconds whatever the host's state does
//! meanwhile; a refusal or a warning holds it until the owner dismisses it. The host's own
//! notices (`SessionState.notices`) are shown beside this device's while their `until` holds.

use std::collections::{BTreeSet, VecDeque};

use corral_protocol::session_state::{HostNotice, SessionState};

use crate::model::hue::Hue;

/// How many notices the Recent notices sheet keeps.
pub const KEPT: usize = 50;
/// A host retains at most four timed notices; keep far more identities than can be live.
const SEEN_FAILURES: usize = 1024;

/// Seconds a notice holds the header's state-word place, at least.
pub const SHOWN_FOR: u64 = 8;

/// One notice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub id: String,
    pub hue: Hue,
    pub text: String,
    /// Unix seconds it was raised; zero before the clock's first tick.
    pub at: u64,
    /// Unix seconds it stops showing in the header. This device's refusals and warnings have
    /// none: they stay until dismissed. A host notice without one stays in the status bar.
    pub until: Option<u64>,
}

/// This device's notices for one session, newest last.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ring {
    items: VecDeque<Notice>,
    next: u64,
    approval_waits: Vec<(String, String)>,
    status_issues: Vec<(String, String)>,
    // Keep a bounded identity history independently of the recent ring. Clipping a live
    // snapshot or evicting a recent row must not make a still-active failure new again.
    seen_plan_failures: BTreeSet<String>,
    seen_plan_order: VecDeque<String>,
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
        approval_waits: Vec::new(),
        status_issues: Vec::new(),
        seen_plan_failures: BTreeSet::new(),
        seen_plan_order: VecDeque::new(),
    };

    /// Raises a notice at `now`; the oldest leaves once 50 are kept. A refusal or a warning
    /// stays until dismissed; anything else for `SHOWN_FOR` seconds.
    pub fn push(&mut self, hue: Hue, text: impl Into<String>, now: u64) {
        self.next += 1;
        self.push_notice(Notice {
            id: format!("notice-{}", self.next),
            hue,
            text: text.into(),
            at: now,
            until: (!matches!(hue, Hue::Fail | Hue::Held)).then_some(now + SHOWN_FOR),
        });
    }

    fn push_notice(&mut self, notice: Notice) {
        if self.items.len() >= KEPT {
            self.items.pop_front();
        }
        self.items.push_back(notice);
    }

    /// Record one failure by its host identity. A later correlated result can supply the
    /// full reason for an already promoted note, but cannot undo its dismissal.
    pub fn record_plan_failure(&mut self, host_id: &str, hue: Hue, text: String, now: u64) {
        let id = format!("notice-host-{host_id}");
        if self.seen_plan_failures.insert(host_id.to_owned()) {
            self.seen_plan_order.push_back(host_id.to_owned());
            if self.seen_plan_order.len() > SEEN_FAILURES
                && let Some(oldest) = self.seen_plan_order.pop_front()
            {
                self.seen_plan_failures.remove(&oldest);
            }
            self.push_notice(Notice {
                id,
                hue,
                text,
                at: now,
                until: None,
            });
        } else if let Some(index) = self.items.iter().position(|notice| notice.id == id) {
            let notice = &mut self.items[index];
            if notice.text != text || notice.hue != hue {
                notice.text = text;
                notice.hue = hue;
                if notice.until.is_none() {
                    // As with a new local outcome, prefer the full reason over other notices.
                    if let Some(notice) = self.items.remove(index) {
                        self.items.push_back(notice);
                    }
                }
            }
        }
    }

    /// Whether this device has already recorded or dismissed this host failure.
    pub fn has_plan_failure(&self, host_id: &str) -> bool {
        self.seen_plan_failures.contains(host_id)
    }

    /// Keep a preparation failure visible until dismissed, even if an older local refusal exists.
    pub fn observe_plan_failures(&mut self, state: Option<&SessionState>, now: u64) {
        let Some(state) = state else { return };
        for notice in &state.notices {
            if active_plan_failure(notice, now) && !self.seen_plan_failures.contains(&notice.id) {
                self.record_plan_failure(&notice.id, Hue::Fail, notice.text.clone(), now);
            }
        }
    }

    /// A duplicate answer is only relevant while that review is being answered.
    pub fn push_approval_wait(&mut self, card: String, text: impl Into<String>, now: u64) {
        self.retain_approval_wait(|id| id != card);
        self.push(Hue::Held, text, now);
        self.approval_waits
            .push((card, format!("notice-{}", self.next)));
    }

    /// Retire wait notices when their corresponding approval is no longer in flight.
    pub fn retain_approval_wait(&mut self, mut waiting: impl FnMut(&str) -> bool) {
        let retired: Vec<String> = self
            .approval_waits
            .iter()
            .filter(|(card, _)| !waiting(card))
            .map(|(_, id)| id.clone())
            .collect();
        for id in retired {
            self.dismiss(&id);
        }
    }

    /// A failed status read has one replaceable notice per review.
    pub fn status_issue(&mut self, card: &str, text: impl Into<String>, now: u64) {
        self.clear_status_issue(card);
        self.push(Hue::Held, text, now);
        self.status_issues
            .push((card.to_owned(), format!("notice-{}", self.next)));
    }

    /// Retire a status-read warning without touching the owner's answer or its failure.
    pub fn clear_status_issue(&mut self, card: &str) {
        let ids: Vec<String> = self
            .status_issues
            .iter()
            .filter(|(id, _)| id == card)
            .map(|(_, notice)| notice.clone())
            .collect();
        for id in ids {
            self.dismiss(&id);
        }
    }

    /// The owner has read the notice `id`: it leaves the header (the sheet keeps it).
    pub fn dismiss(&mut self, id: &str) {
        if let Some(notice) = self.items.iter_mut().find(|notice| notice.id == id) {
            notice.until = Some(0);
        }
        self.approval_waits.retain(|(_, notice)| notice != id);
        self.status_issues.retain(|(_, notice)| notice != id);
    }

    /// Newest first.
    pub fn recent(&self) -> impl Iterator<Item = &Notice> {
        self.items.iter().rev()
    }
}

const PLAN_FAILURE_PREFIX: &str = "plan-failure-";

/// The ID the host gives the timed note for a failed approval attempt.
pub fn plan_failure_id(host: &str, epoch: &str) -> String {
    format!("{PLAN_FAILURE_PREFIX}{host}-{epoch}")
}

pub fn is_plan_failure(notice: &HostNotice) -> bool {
    notice.tone == "error"
        && (notice.id.starts_with(PLAN_FAILURE_PREFIX)
            // Older hosts have no correlated ID. Promote their notes independently.
            || notice.text.starts_with("The plan was not started:"))
}

pub fn active_plan_failure(notice: &HostNotice, now: u64) -> bool {
    is_plan_failure(notice)
        && notice
            .until
            .as_deref()
            .and_then(unix)
            .is_some_and(|until| now < until)
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

/// The notice the header shows in its state word's place now: this device's newest one not
/// dismissed and still within its time, else the host's newest timed one within its `until`.
pub fn current(ring: &Ring, state: Option<&SessionState>, now: u64) -> Option<Notice> {
    ring.recent()
        .find(|notice| notice.until.is_none_or(|until| now < until))
        .cloned()
        .or_else(|| {
            let state = state?;
            state
                .notices
                .iter()
                .rev()
                .filter(|notice| !ring.seen_plan_failures.contains(&notice.id))
                .map(host)
                .find(|notice| notice.until.is_some_and(|until| now < until))
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
