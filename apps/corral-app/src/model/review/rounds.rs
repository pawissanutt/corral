//! The rounds of a candidate: every gate and review check corrald stored for the task, in the
//! order it recorded them, split into the submissions they judged, each with the reviewer's
//! notes; the task's round against its limit; and the reviewer session, which stays with the task
//! across rounds.

use corral_types::{Check, CheckKind, CheckResult, Status};

use crate::model::daemon::Feed;

use super::live::{Shown, when};
use super::readiness::{ReadyLink, recorded};
use super::view::{ButtonStyle, button};
use super::{Action, Button};

/// One submission and the checks that judged it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Round {
    /// "Round 1"; the engine's round number, which the owner's own send-back resets.
    pub title: String,
    /// The round the task is in now.
    pub current: bool,
    pub links: Vec<ReadyLink>,
}

/// The live reviewer session of the task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewerLine {
    pub text: String,
    pub hint: String,
    pub open: Button,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rounds {
    pub heading: &'static str,
    /// "sent back 0 of 2": the engine's round (automatic send-backs since the owner last
    /// acted, from 0) against their limit, as counted, not the meta line's round number.
    pub count: Option<String>,
    pub hint: &'static str,
    /// Oldest first; empty while the task is in its first round, which the readiness chain
    /// already shows.
    pub rounds: Vec<Round>,
    pub reviewer: Option<ReviewerLine>,
}

/// The checks in insertion order, split where a new submission was judged. A check that names
/// its round starts the next group when the round changes, or when it gates another worker run
/// than the round's gate did. An older check without one is inferred: a review closes its
/// round, and a gate on another commit than the round's gate starts the next one. A rerun gate
/// on the same commit stays in its round.
fn split(checks: &[Check]) -> Vec<Vec<&Check>> {
    let mut rounds: Vec<Vec<&Check>> = Vec::new();
    for check in checks {
        let starts = rounds.last().is_none_or(|round| starts(round, check));
        if starts {
            rounds.push(vec![check]);
        } else if let Some(round) = rounds.last_mut() {
            round.push(check);
        }
    }
    rounds
}

fn starts(round: &[&Check], check: &Check) -> bool {
    let other_gate = |differs: fn(&Check, &Check) -> bool| {
        check.kind == CheckKind::Gate
            && round
                .iter()
                .any(|seen| seen.kind == CheckKind::Gate && differs(seen, check))
    };
    match (check.round, round.iter().find_map(|seen| seen.round)) {
        (Some(number), Some(known)) => {
            number != known
                || other_gate(|seen, check| {
                    seen.run.is_some() && check.run.is_some() && seen.run != check.run
                })
        }
        _ => {
            round.iter().any(|seen| seen.kind == CheckKind::Review)
                || other_gate(|seen, check| seen.key != check.key)
        }
    }
}

/// Each round's number: the round its checks name; else its review's key; else the task's
/// round for the one in progress; else one past the round before it when that one ended failed
/// (it was sent back), the same when it passed (a landing's rebase gated again).
fn numbers(rounds: &[Vec<&Check>], now: u32) -> Vec<u32> {
    let mut numbers: Vec<u32> = Vec::with_capacity(rounds.len());
    for (at, round) in rounds.iter().enumerate() {
        let served = round.iter().find_map(|check| check.round);
        let reviewed = round
            .iter()
            .find(|check| check.kind == CheckKind::Review)
            .and_then(|check| check.key.parse().ok());
        let number = served.or(reviewed).unwrap_or_else(|| {
            if at + 1 == rounds.len() {
                return now;
            }
            match (
                numbers.last(),
                at.checked_sub(1).map(|before| &rounds[before]),
            ) {
                (Some(before), Some(checks)) => {
                    let failed = checks
                        .last()
                        .is_some_and(|check| check.result == CheckResult::Fail);
                    before + u32::from(failed)
                }
                _ => 0,
            }
        });
        numbers.push(number);
    }
    numbers
}

/// "Round 1", with the worker run its gate names when it says, and "· now" for the current one.
fn title(number: u32, checks: &[&Check], current: bool) -> String {
    let mut title = format!("Round {number}");
    let gated = checks
        .iter()
        .find(|check| check.kind == CheckKind::Gate)
        .and_then(|check| check.run.as_deref());
    if let Some(run) = gated {
        title.push_str(&format!(" · {run}"));
    }
    if current {
        title.push_str(" · now");
    }
    title
}

pub(super) fn rounds(feed: &Feed, shown: &Shown) -> Option<Rounds> {
    let task = shown.task.task();
    let max = shown
        .task
        .row
        .max_rounds
        .or_else(|| shown.review.map(|review| review.max_rounds));
    let reviewer = shown.task.row.reviewer.as_ref().map(|run| {
        let since = when(run.started, feed);
        let mut text = format!("Reviewer {} · {}", run.id, run.profile);
        if !since.is_empty() {
            text.push_str(&format!(" · live since {since}"));
        }
        ReviewerLine {
            text,
            hint: "This reviewer stays with the task across rounds: a send-back keeps the session, and the next round's review goes to it with what it already said.".into(),
            open: button(
                "Open reviewer session",
                Action::OpenReviewer,
                ButtonStyle::Quiet,
            ),
        }
    });
    let checks = shown
        .review
        .map(|review| review.checks.as_slice())
        .unwrap_or_default();
    let split = split(checks);
    let numbers = numbers(&split, task.round);
    let open = !matches!(task.status, Status::Done | Status::Dropped);
    let last = split.len().saturating_sub(1);
    let rounds: Vec<Round> = if split.len() < 2 {
        Vec::new()
    } else {
        split
            .iter()
            .zip(numbers)
            .enumerate()
            .map(|(at, (checks, number))| {
                let current = open && at == last;
                Round {
                    title: title(number, checks, current),
                    current,
                    links: checks
                        .iter()
                        .map(|check| recorded(feed, check, true))
                        .collect(),
                }
            })
            .collect()
    };
    if rounds.is_empty() && reviewer.is_none() {
        return None;
    }
    Some(Rounds {
        heading: "Rounds",
        // `task.round` counts automatic send-backs against their budget, from 0; it is not the
        // meta line's round number, so the caption names what it counts.
        count: max.map(|max| format!("sent back {} of {max}", task.round)),
        hint: "Each round is one submission and the checks that judged it. A red gate or a review asking changes sends the work back for another round; once the rounds are used, the task waits for you.",
        rounds,
        reviewer,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use corral_types::Actor;
    use jiff::Timestamp;

    fn check(kind: CheckKind, key: &str, pass: bool) -> Check {
        Check {
            task: "T-1".into(),
            kind,
            key: key.into(),
            result: if pass {
                CheckResult::Pass
            } else {
                CheckResult::Fail
            },
            notes: None,
            log: None,
            by: Actor::Auto,
            at: Timestamp::UNIX_EPOCH,
            round: None,
            run: None,
        }
    }

    /// A task sent back once by its reviewer, then gated green on the new commit and approved,
    /// then gated again after the landing's rebase: three rounds numbered 0, 1 and 1, and a gate
    /// rerun on one commit stays in its round. Checks that name their round and run are grouped
    /// by them: the rebase gate of the approved run stays in round 1.
    #[test]
    fn checks_split_into_the_submissions_they_judged() {
        let checks = [
            check(CheckKind::Gate, "aaa", false),
            check(CheckKind::Gate, "aaa", true),
            check(CheckKind::Review, "0", false),
            check(CheckKind::Gate, "bbb", true),
            check(CheckKind::Review, "1", true),
            check(CheckKind::Gate, "ccc", true),
        ];
        let rounds = split(&checks);
        assert_eq!(rounds.iter().map(Vec::len).collect::<Vec<_>>(), [3, 2, 1]);
        assert_eq!(numbers(&rounds[..2], 1), [0, 1]);
        assert_eq!(numbers(&rounds, 1), [0, 1, 1]);
        let red = [
            check(CheckKind::Gate, "aaa", false),
            check(CheckKind::Gate, "bbb", true),
        ];
        assert_eq!(numbers(&split(&red), 1), [0, 1]);
        let served: Vec<Check> = [(0, "R1"), (0, "R9"), (1, "R2"), (1, "R9"), (1, "R2")]
            .into_iter()
            .zip(&checks[1..])
            .map(|((round, run), check)| Check {
                round: Some(round),
                run: Some(run.into()),
                ..check.clone()
            })
            .collect();
        let rounds = split(&served);
        assert_eq!(rounds.iter().map(Vec::len).collect::<Vec<_>>(), [2, 3]);
        assert_eq!(numbers(&rounds, 1), [0, 1]);
    }
}
