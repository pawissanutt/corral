//! Builds' words and figures: what an outcome is called, counts, costs and the notes that make
//! a shortened list visible. A build is an interactive session that implemented, verified,
//! reviewed and landed work itself; nothing here calls it a task.

use crate::model::board::Mark;
use crate::model::hue::Hue;
use crate::model::text::{Rich, span};

/// A build's outcome as the list groups it and the detail names it. Landing over open review
/// findings is its own outcome, so the override is never folded into an ordinary landing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Outcome {
    InFlight,
    Override,
    Landed,
    Kept,
    Abandoned,
    Other,
}

impl Outcome {
    /// List order: what is still running, then what landed over findings, then the rest.
    pub(super) const ORDER: [Outcome; 6] = [
        Outcome::InFlight,
        Outcome::Override,
        Outcome::Landed,
        Outcome::Kept,
        Outcome::Abandoned,
        Outcome::Other,
    ];

    /// From the served outcome state and override reason.
    pub(super) fn of(state: &str, override_reason: Option<&str>) -> Outcome {
        match state {
            "open" => Outcome::InFlight,
            "landed" if override_reason.is_some() => Outcome::Override,
            "landed" => Outcome::Landed,
            "kept" => Outcome::Kept,
            "abandoned" => Outcome::Abandoned,
            _ => Outcome::Other,
        }
    }

    pub(super) fn rank(self) -> usize {
        Self::ORDER
            .iter()
            .position(|outcome| *outcome == self)
            .unwrap_or(Self::ORDER.len())
    }

    /// The list's group heading.
    pub(super) const fn section(self) -> &'static str {
        match self {
            Outcome::InFlight => "In flight",
            Outcome::Override => "Landed over open findings",
            Outcome::Landed => "Landed",
            Outcome::Kept => "Kept, not landed",
            Outcome::Abandoned => "Abandoned",
            Outcome::Other => "Other",
        }
    }

    /// The outcome in a row or chip.
    pub(super) const fn label(self) -> &'static str {
        match self {
            Outcome::InFlight => "in flight",
            Outcome::Override => "landed by override",
            Outcome::Landed => "landed",
            Outcome::Kept => "kept, not landed",
            Outcome::Abandoned => "abandoned",
            Outcome::Other => "state not known to this app",
        }
    }

    pub(super) const fn hue(self) -> Hue {
        match self {
            Outcome::InFlight => Hue::Worker,
            Outcome::Override => Hue::Held,
            Outcome::Landed => Hue::Ok,
            Outcome::Kept | Outcome::Abandoned | Outcome::Other => Hue::Neutral,
        }
    }
}

/// The callout for a build landed over open review findings, with the owner's reason.
pub(super) fn override_line(reason: &str) -> Rich {
    let reason = reason.trim();
    vec![
        span("Landed over open review findings")
            .bold()
            .hue(Hue::Held),
        span(if reason.is_empty() {
            " · no reason recorded".to_owned()
        } else {
            format!(" · “{reason}”")
        }),
    ]
}

/// `1 slice`, `4 slices`.
pub(crate) fn count(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// What the gate did, and whether that needs a look: failures are held, none ran is quiet.
pub(super) fn gate(attempts: u64, failures: u64) -> (String, Hue) {
    match (attempts, failures) {
        (0, _) => ("no gate run".into(), Hue::Neutral),
        (n, 0) => (count(n, "gate run", "gate runs"), Hue::Neutral),
        (n, f) => (
            format!("{}, {f} failed", count(n, "gate run", "gate runs")),
            Hue::Held,
        ),
    }
}

/// `1 review round`, `no review`.
pub(super) fn reviews(rounds: u64) -> String {
    if rounds == 0 {
        "no review".into()
    } else {
        count(rounds, "review round", "review rounds")
    }
}

/// A recorded cost, or "cost unknown": a missing price is never shown as zero.
pub(crate) fn cost(usd: Option<f64>) -> String {
    match usd {
        Some(value) if value.is_finite() => crate::model::usage::money(value),
        _ => "cost unknown".into(),
    }
}

/// `1.2M tokens`.
pub(crate) fn tokens(n: u64) -> String {
    format!("{} tokens", crate::model::usage::tokens(n))
}

/// "showing 100 of 240 messages" when corrald sent fewer than the record holds; nothing when
/// every entry is here.
pub(super) fn shown(len: usize, total: u64, noun: &str) -> Option<String> {
    let len = u64::try_from(len).unwrap_or(u64::MAX);
    (total > len).then(|| format!("showing {len} of {total} {noun}"))
}

/// A section's heading note: the truncation when the list is cut, else how many there are.
pub(crate) fn note(len: usize, total: u64, noun: &str) -> Option<String> {
    shown(len, total, noun).or_else(|| Some(total.to_string()))
}

/// A commit id cut to its usual seven characters.
pub(super) fn short(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}

/// A slice's state as a mark, its hue and its word.
pub(crate) fn slice_state(state: &str) -> (Mark, Hue, &'static str) {
    match state {
        "queued" => (Mark::Next, Hue::Neutral, "queued"),
        "running" => (Mark::Active, Hue::Worker, "running"),
        "needs" => (Mark::Active, Hue::You, "needs an answer"),
        "done" => (Mark::Done, Hue::Neutral, "done"),
        "failed" => (Mark::Fail, Hue::Fail, "failed"),
        "cancelled" => (Mark::Na, Hue::Neutral, "cancelled"),
        _ => (Mark::Na, Hue::Neutral, "state not known to this app"),
    }
}

/// A gate attempt's result.
pub(super) fn gate_result(result: &str) -> (Mark, Hue, &'static str) {
    match result {
        "pass" => (Mark::Done, Hue::Ok, "passed"),
        "fail" => (Mark::Fail, Hue::Fail, "failed"),
        "skipped" => (Mark::Na, Hue::Neutral, "skipped"),
        _ => (Mark::Na, Hue::Neutral, "result not known to this app"),
    }
}

/// A review round's verdict.
pub(crate) fn verdict(verdict: &str) -> (Mark, Hue, &'static str) {
    match verdict {
        "approve" => (Mark::Done, Hue::Ok, "approved"),
        "changes" => (Mark::Fail, Hue::Held, "changes requested"),
        _ => (Mark::Na, Hue::Neutral, "verdict not known to this app"),
    }
}

/// A finding's severity as its chip.
pub(super) fn severity(severity: &str) -> (String, Hue) {
    let hue = match severity {
        "blocker" => Hue::Fail,
        "major" => Hue::Held,
        _ => Hue::Neutral,
    };
    (severity.to_owned(), hue)
}

/// How a message reached its recipient: the mark, and the words when it did not simply arrive.
pub(crate) fn delivery(delivery: &str) -> (Mark, Hue, Option<&'static str>) {
    match delivery {
        "delivered" => (Mark::Done, Hue::Neutral, None),
        "queued" => (Mark::Next, Hue::Held, Some("queued, not yet delivered")),
        "undeliverable" => (Mark::Fail, Hue::Fail, Some("undeliverable")),
        "answered_from_report" => (Mark::Na, Hue::Neutral, Some("answered from its report")),
        _ => (
            Mark::Na,
            Hue::Neutral,
            Some("delivery not known to this app"),
        ),
    }
}

/// An owner decision's kind, as its chip.
pub(super) fn decision(kind: &str) -> (&'static str, Hue) {
    match kind {
        "question" => ("question", Hue::Neutral),
        "exit" => ("exit", Hue::Neutral),
        "override" => ("override", Hue::Held),
        _ => ("decision", Hue::Neutral),
    }
}

/// A timeline phase's name.
pub(super) fn phase(phase: &str) -> String {
    match phase {
        "started" => "Started".into(),
        "children" => "Child agents".into(),
        "verify" => "Verify".into(),
        "review" => "Review".into(),
        "land" => "Land".into(),
        "exit" => "Exit".into(),
        other => other.to_owned(),
    }
}
