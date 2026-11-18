//! "Ready to land?": the gate check for the task's HEAD, the review check for the current
//! round, then the owner's verdict, each from what corrald recorded.

use corral_types::{Check, CheckKind, CheckResult, Status};

use crate::model::board::Mark;
use crate::model::daemon::Feed;
use crate::model::hue::Hue;

use super::live::{Shown, actor, sha7, when};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewQuote {
    pub by: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadyLink {
    pub mark: Mark,
    pub hue: Hue,
    pub title: &'static str,
    pub sha7: Option<String>,
    pub quote: Option<ReviewQuote>,
    pub hint: String,
}

fn hue(mark: Mark) -> Hue {
    match mark {
        Mark::Done => Hue::Ok,
        Mark::Active => Hue::Corral,
        Mark::Fail => Hue::Fail,
        Mark::Stale => Hue::Held,
        Mark::Next | Mark::Na => Hue::Neutral,
    }
}

fn title(kind: CheckKind, mark: Mark) -> &'static str {
    match (kind, mark) {
        (CheckKind::Gate, Mark::Done) => "Gate passed",
        (CheckKind::Gate, Mark::Fail) => "Gate failed",
        (CheckKind::Gate, Mark::Active) => "Gate running",
        (CheckKind::Gate, _) => "Gate pending",
        (CheckKind::Review, Mark::Done) => "Review approves",
        (CheckKind::Review, Mark::Fail) => "Review asks changes",
        (CheckKind::Review, Mark::Active) => "Review running",
        (CheckKind::Review, _) => "Review pending",
    }
}

/// One check's link; a missing one is running while the task waits on it in Review.
fn link(feed: &Feed, kind: CheckKind, check: Option<&Check>, waiting: bool) -> ReadyLink {
    let Some(check) = check else {
        let mark = if waiting { Mark::Active } else { Mark::Next };
        return ReadyLink {
            mark,
            hue: hue(mark),
            title: title(kind, mark),
            sha7: None,
            quote: None,
            hint: match kind {
                CheckKind::Gate => "No gate result is recorded for the task's HEAD.",
                CheckKind::Review => "No review result is recorded for this round.",
            }
            .into(),
        };
    };
    recorded(feed, check, false)
}

/// A recorded check: a gate names its commit, a review its round. The reviewer's notes are
/// quoted; a gate's failing line is quoted too when `quote_gate`, else it joins the hint.
pub(super) fn recorded(feed: &Feed, check: &Check, quote_gate: bool) -> ReadyLink {
    let kind = check.kind;
    let mark = match check.result {
        CheckResult::Pass => Mark::Done,
        CheckResult::Fail => Mark::Fail,
    };
    let on = match kind {
        CheckKind::Gate => sha7(&check.key),
        CheckKind::Review => format!("round {}", check.key),
    };
    let mut hint = format!(
        "{} recorded {} on {on}",
        actor(check.by),
        match check.result {
            CheckResult::Pass => "a pass",
            CheckResult::Fail => "a failure",
        },
    );
    let at = when(check.at, feed);
    if !at.is_empty() {
        hint.push_str(&format!(" at {at}"));
    }
    let notes = check
        .notes
        .as_ref()
        .filter(|notes| !notes.trim().is_empty());
    let quoted = kind == CheckKind::Review || quote_gate;
    if !quoted && let Some(notes) = notes {
        hint.push_str(&format!(" · {notes}"));
    }
    ReadyLink {
        mark,
        hue: hue(mark),
        title: title(kind, mark),
        sha7: (kind == CheckKind::Gate).then(|| sha7(&check.key)),
        quote: notes.filter(|_| quoted).map(|notes| ReviewQuote {
            by: actor(check.by).to_owned(),
            text: notes.clone(),
        }),
        hint,
    }
}

pub(super) fn readiness_chain(feed: &Feed, shown: &Shown) -> Vec<ReadyLink> {
    let in_review = shown.task.status() == Status::Review;
    let gate = shown.check(CheckKind::Gate);
    let gate_passed = gate.is_some_and(|gate| gate.result == CheckResult::Pass);
    let ready = shown.task.row.ready;
    vec![
        link(feed, CheckKind::Gate, gate, in_review),
        link(
            feed,
            CheckKind::Review,
            shown.check(CheckKind::Review),
            in_review && gate_passed,
        ),
        ReadyLink {
            mark: if ready { Mark::Active } else { Mark::Next },
            hue: if ready { Hue::You } else { Hue::Neutral },
            title: "Your verdict",
            sha7: shown.head().map(sha7),
            quote: None,
            hint: if ready {
                "corrald marked this candidate ready: approve and land it, or request changes."
                    .into()
            } else {
                "The pipeline has not marked this candidate ready; you can still decide now.".into()
            },
        },
    ]
}
