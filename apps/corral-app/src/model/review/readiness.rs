//! Current-candidate evidence stays in the chain; older candidates only inform its hints.

use corral_protocol::detail::ReviewDetail;

use crate::model::board::Mark;
use crate::model::daemon::{Feed, Task};
use crate::model::glance::{Stat, StatIcon};
use crate::model::hue::Hue;

use super::live::when;
use super::outline::sha7;

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
    pub stats: Vec<Stat>,
    pub quote: Option<ReviewQuote>,
    pub hint: String,
}

fn mark(verdict: &str, stale: bool) -> Mark {
    if stale {
        return Mark::Stale;
    }
    match verdict.to_ascii_lowercase().as_str() {
        "pass" | "approve" | "approved" => Mark::Done,
        "fail" | "changes" | "request_changes" | "request-changes" | "escalate" | "rejected" => {
            Mark::Fail
        }
        "running" => Mark::Active,
        "stale" => Mark::Stale,
        "cancelled" => Mark::Na,
        _ => Mark::Next,
    }
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

fn title(mark: Mark, gate: bool) -> &'static str {
    match (gate, mark) {
        (true, Mark::Done) => "Gate passed",
        (true, Mark::Active) => "Gate running",
        (true, Mark::Fail) => "Gate failed",
        (true, Mark::Stale) => "Gate stale",
        (true, Mark::Na) => "Gate not applicable",
        (true, Mark::Next) => "Gate pending",
        (false, Mark::Done) => "Review approves",
        (false, Mark::Fail) => "Review asks changes",
        (false, Mark::Active) => "Review running",
        (false, Mark::Stale) => "Review stale",
        (false, Mark::Na) => "Review not applicable",
        (false, Mark::Next) => "Review pending",
    }
}

fn receipt_hint(
    by: &str,
    verdict: &str,
    at: Option<&str>,
    text: Option<&str>,
    feed: &Feed,
) -> String {
    let mut hint = format!("{by}: {verdict}");
    if let Some(at) = at {
        hint.push_str(" at ");
        hint.push_str(&when(at, feed));
    }
    if let Some(text) = text.filter(|text| !text.trim().is_empty()) {
        hint.push_str(" · ");
        hint.push_str(text);
    }
    hint
}

fn earlier_hint(
    sha: &str,
    by: &str,
    verdict: &str,
    at: Option<&str>,
    text: Option<&str>,
    feed: &Feed,
) -> String {
    let outcome = match mark(verdict, false) {
        Mark::Fail => "failed",
        Mark::Done => "passed",
        Mark::Active => "was running",
        Mark::Stale => "was stale",
        Mark::Na => "was cancelled",
        Mark::Next => verdict,
    };
    let mut hint = format!("An earlier candidate {} {outcome}", sha7(sha));
    if let Some(at) = at {
        hint.push_str(" at ");
        hint.push_str(&when(at, feed));
    }
    hint.push_str(" · ");
    hint.push_str(by);
    if let Some(text) = text.filter(|text| !text.trim().is_empty()) {
        hint.push_str(": ");
        hint.push_str(text);
    }
    hint.push_str(". This result is stale for the current candidate.");
    hint
}

fn stale_stat(key: &str, hints: Vec<String>) -> Vec<Stat> {
    if hints.is_empty() {
        Vec::new()
    } else {
        vec![Stat::new(
            key,
            StatIcon::Stale,
            hints.len().to_string(),
            Hue::Held,
            hints.join("\n"),
        )]
    }
}

/// Full SHA equality is deliberate: receipt identities never borrow the board's shortened
/// prefix. A stale receipt on the same SHA (for example, a moved base) stays visibly stale.
pub fn readiness_chain(feed: &Feed, task: &Task, review: &ReviewDetail) -> Vec<ReadyLink> {
    let current = review.candidate_sha.as_deref();
    let mut gate_history = Vec::new();
    let mut review_history = Vec::new();
    let mut gates = Vec::new();
    let mut verdicts = Vec::new();
    for gate in &review.gates {
        if current.is_none() {
            continue;
        }
        if current != Some(gate.sha.as_str()) {
            gate_history.push(earlier_hint(
                &gate.sha,
                &gate.by,
                &gate.verdict,
                gate.at.as_deref(),
                gate.text.as_deref(),
                feed,
            ));
            continue;
        }
        let mark = mark(&gate.verdict, gate.stale);
        gates.push(ReadyLink {
            mark,
            hue: hue(mark),
            title: title(mark, true),
            sha7: Some(sha7(&gate.sha)),
            stats: Vec::new(),
            quote: None,
            hint: receipt_hint(
                &gate.by,
                &gate.verdict,
                gate.at.as_deref(),
                gate.text.as_deref(),
                feed,
            ),
        });
    }
    for verdict in &review.verdicts {
        if current.is_none() {
            continue;
        }
        if current != Some(verdict.sha.as_str()) {
            review_history.push(earlier_hint(
                &verdict.sha,
                &verdict.by,
                &verdict.verdict,
                verdict.at.as_deref(),
                verdict.text.as_deref(),
                feed,
            ));
            continue;
        }
        let mark = mark(&verdict.verdict, verdict.stale);
        verdicts.push(ReadyLink {
            mark,
            hue: hue(mark),
            title: title(mark, false),
            sha7: Some(sha7(&verdict.sha)),
            stats: Vec::new(),
            quote: verdict
                .text
                .as_ref()
                .filter(|text| !text.trim().is_empty())
                .map(|text| ReviewQuote {
                    by: verdict.by.clone(),
                    text: text.clone(),
                }),
            hint: receipt_hint(
                &verdict.by,
                &verdict.verdict,
                verdict.at.as_deref(),
                None,
                feed,
            ),
        });
    }
    // Rails can describe in-flight work before its receipt exists. Do not turn a completed
    // rail into an invented receipt: only running/stale/failure marks describe this gap.
    let rail_matches = current
        .zip(task.rail[0].sha7.as_deref())
        .is_some_and(|(current, board)| current.starts_with(board) || board.starts_with(current));
    let awaiting = |rail: Mark, gate: bool| {
        let mark =
            if rail_matches && matches!(rail, Mark::Active | Mark::Stale | Mark::Fail | Mark::Na) {
                rail
            } else {
                Mark::Next
            };
        ReadyLink {
            mark,
            hue: hue(mark),
            title: title(mark, gate),
            sha7: current.map(sha7),
            stats: Vec::new(),
            quote: None,
            hint: if gate {
                "No gate receipt is recorded for this candidate."
            } else {
                "No reviewer verdict is recorded for this candidate."
            }
            .into(),
        }
    };
    if gates.is_empty() {
        gates.push(awaiting(task.rail[1].mark, true));
    }
    if verdicts.is_empty() {
        verdicts.push(awaiting(task.rail[2].mark, false));
    }
    if let Some(gate) = gates.first_mut() {
        gate.stats = stale_stat("earlier-gates", gate_history);
    }
    if let Some(verdict) = verdicts.first_mut() {
        verdict.stats = stale_stat("earlier-reviews", review_history);
    }
    gates.extend(verdicts);
    gates.push(ReadyLink {
        mark: Mark::Next,
        hue: if review.approvable {
            Hue::You
        } else {
            Hue::Neutral
        },
        title: "Your verdict",
        sha7: current.map(sha7),
        stats: Vec::new(),
        quote: None,
        hint: match current {
            Some(sha) if review.approvable => format!(
                "Your verdict applies to {} only; a new candidate makes the earlier results stale.",
                sha7(sha)
            ),
            Some(sha) => format!("Your verdict on {} is not available yet.", sha7(sha)),
            None => "The candidate SHA is not known yet.".into(),
        },
    });
    gates
}
