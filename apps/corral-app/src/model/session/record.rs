//! A build's record as its host keeps it (`DetailRef::BuildRecord`, the host's `record`): the
//! outcome, what the branch holds, each gate attempt, each review round with its findings, the
//! owner's decisions, the timeline and the usage totals. The Board tab opens it in a pager; the
//! exit's result card in the transcript lists its gate attempts and review rounds, so it is read
//! as soon as the build's exit is under way. Every line is the record's; a section the record
//! holds nothing for says so rather than disappearing.

use corral_protocol::detail::{
    BuildFinding, BuildRecord, BuildReview, BuildUsage, BuildVerification, DetailBody, DetailRef,
};
use corral_protocol::session_state::ExitPhase;

use crate::model::daemon::Feed;
use crate::model::hue::Hue;

use super::super::activity::duration;
use super::super::tabs::{PartBody, load, part};
use super::super::{Action, Cx, SessionRole};

/// The host part the record is served by.
const CAPABILITY: &str = "record";

pub fn target(cx: &Cx<'_>) -> DetailRef {
    DetailRef::BuildRecord {
        id: cx.id.to_string(),
    }
}

/// The record last read for session `id`.
pub fn loaded<'f>(feed: &'f Feed, id: &str) -> Option<&'f BuildRecord> {
    match feed
        .detail(&DetailRef::BuildRecord { id: id.to_owned() })?
        .body()?
    {
        DetailBody::BuildRecord(record) => Some(record),
        _ => None,
    }
}

/// The record in the Board tab's pager: where its read stands and, once read, its sections.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordPager {
    pub part: PartBody,
    pub view: Option<RecordView>,
    /// `‹ Board`: back to the board.
    pub close: Action,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordView {
    pub title: String,
    /// `✓ Landed 3f2a91c on main`, `○ Open`.
    pub outcome: RecordLine,
    pub sections: Vec<RecordSection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordSection {
    /// `record-<slug>`.
    pub id: &'static str,
    pub title: &'static str,
    /// On the caption's trailing side: `3 of 12 commits`.
    pub note: Option<String>,
    pub lines: Vec<RecordLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordLine {
    /// `✓`, `✗`, `○`; none for a plain line.
    pub glyph: Option<&'static str>,
    pub hue: Hue,
    pub text: String,
    /// Muted under the text: a failed command, a finding's resolution, an answer.
    pub detail: Option<String>,
    /// Under the line before it (a review's finding).
    pub nested: bool,
}

fn line(glyph: Option<&'static str>, hue: Hue, text: String) -> RecordLine {
    RecordLine {
        glyph,
        hue,
        text,
        detail: None,
        nested: false,
    }
}

fn none(what: &str) -> RecordLine {
    line(Some("○"), Hue::Neutral, format!("{what} recorded"))
}

fn sha7(sha: &str) -> String {
    sha.chars().take(7).collect()
}

/// `1m 20s` between two RFC 3339 instants; none when either does not parse.
fn took(started: &str, ended: &str) -> Option<String> {
    let started = started.parse::<jiff::Timestamp>().ok()?;
    let ended = ended.parse::<jiff::Timestamp>().ok()?;
    let seconds = ended.as_second().checked_sub(started.as_second())?;
    u64::try_from(seconds).ok().map(duration)
}

fn outcome(record: &BuildRecord) -> RecordLine {
    let outcome = &record.outcome;
    let base = outcome
        .base_branch
        .clone()
        .unwrap_or_else(|| "the base branch".into());
    let mut found = match outcome.state.as_str() {
        "landed" => line(
            Some("✓"),
            Hue::Ok,
            match outcome.landed.as_deref().or(outcome.head.as_deref()) {
                Some(sha) => format!("Landed {} on {base}", sha7(sha)),
                None => format!("Landed on {base}"),
            },
        ),
        "kept" => line(Some("✓"), Hue::Ok, "Kept on its branch".into()),
        "failed" => line(Some("✗"), Hue::Fail, "The exit failed".into()),
        "" => line(Some("○"), Hue::Neutral, "No outcome recorded yet".into()),
        state => {
            let mut words = state.replace('_', " ");
            if let Some(first) = words.get(..1) {
                words = format!("{}{}", first.to_uppercase(), words.get(1..).unwrap_or(""));
            }
            line(Some("○"), Hue::Neutral, words)
        }
    };
    found.detail = outcome
        .override_reason
        .as_ref()
        .map(|reason| format!("your reason: {reason}"));
    found
}

fn changes(record: &BuildRecord) -> RecordSection {
    let shown = record.commits.len() as u64;
    let mut lines = Vec::new();
    if let Some(diff) = &record.diff {
        lines.push(line(
            None,
            Hue::Neutral,
            format!(
                "{} files +{} −{}, {} commits",
                diff.files, diff.added, diff.removed, diff.commits
            ),
        ));
    }
    // Newest last, as the record keeps them; the pager reads newest first.
    lines.extend(record.commits.iter().rev().map(|commit| RecordLine {
        detail: commit.slice.as_ref().map(|slice| format!("by {slice}")),
        ..line(
            None,
            Hue::Neutral,
            format!("{}  {}", sha7(&commit.sha), commit.subject),
        )
    }));
    if lines.is_empty() {
        lines.push(none("No changes"));
    }
    RecordSection {
        id: "record-changes",
        title: "Changes",
        note: (record.commits_total > shown)
            .then(|| format!("newest {shown} of {} commits", record.commits_total)),
        lines,
    }
}

/// One gate attempt: `✗ G2 · full gate failed on 3f2a91c · 1m 20s`, the failed command under it.
pub fn gate_line(gate: &BuildVerification) -> RecordLine {
    let (glyph, hue, word) = match gate.result.as_str() {
        "pass" => ("✓", Hue::Ok, "passed"),
        "fail" => ("✗", Hue::Fail, "failed"),
        _ => ("○", Hue::Neutral, "did not finish"),
    };
    let mut text = format!("{} · {} gate {word}", gate.attempt, gate.tier);
    if !gate.head.is_empty() {
        text.push_str(&format!(" on {}", sha7(&gate.head)));
    }
    if let Some(took) = took(&gate.started, &gate.ended) {
        text.push_str(&format!(" · {took}"));
    }
    RecordLine {
        detail: gate
            .failed_command
            .as_ref()
            .filter(|_| hue == Hue::Fail)
            .map(|command| format!("failed: {command}")),
        ..line(Some(glyph), hue, text)
    }
}

fn finding(finding: &BuildFinding) -> RecordLine {
    let hue = match finding.severity.as_str() {
        "blocker" => Hue::Fail,
        "major" => Hue::Held,
        _ => Hue::Neutral,
    };
    let place = match (&finding.path, finding.line) {
        (Some(path), Some(at)) => format!("{path}:{at} · "),
        (Some(path), None) => format!("{path} · "),
        _ => String::new(),
    };
    let detail = finding.resolution.as_ref().map(|resolution| {
        let mut words = resolution.state.replace('_', " ");
        if let Some(commit) = &resolution.commit {
            words.push_str(&format!(" in {}", sha7(commit)));
        }
        if let Some(reason) = &resolution.reason {
            words.push_str(&format!(": {reason}"));
        }
        words
    });
    RecordLine {
        glyph: None,
        hue,
        text: format!("{} · {place}{}", finding.severity, finding.text),
        detail,
        nested: true,
    }
}

/// One review round: `✗ R1 · changes asked · opus · 1 major, 2 minor`, then each finding.
pub fn review_lines(review: &BuildReview) -> Vec<RecordLine> {
    let approve = review.verdict == "approve";
    let model = review.model.rsplit('/').next().unwrap_or(&review.model);
    let counts: Vec<String> = ["blocker", "major", "minor"]
        .into_iter()
        .filter_map(|severity| {
            let n = review
                .findings
                .iter()
                .filter(|found| found.severity == severity)
                .count();
            (n > 0).then(|| format!("{n} {severity}"))
        })
        .collect();
    let verdict = if approve {
        "approves".to_owned()
    } else {
        review.verdict.replace('_', " ")
    };
    let mut head = line(
        Some(if approve { "✓" } else { "✗" }),
        if approve { Hue::Ok } else { Hue::Fail },
        format!(
            "R{} · {verdict} · {model} · {}",
            review.round,
            if counts.is_empty() {
                "no findings".to_owned()
            } else {
                counts.join(", ")
            }
        ),
    );
    head.detail = review.summary.clone();
    std::iter::once(head)
        .chain(review.findings.iter().map(finding))
        .collect()
}

fn tokens(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.1}k", n as f64 / 1_000.0),
        _ => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
}

fn usage_line(who: &str, usage: &BuildUsage) -> RecordLine {
    let cost = usage.cost_usd.map_or_else(
        || "cost not reported".to_owned(),
        |cost| format!("${cost:.2} API-equivalent"),
    );
    line(
        None,
        Hue::Neutral,
        format!(
            "{who} · {} in · {} out · {cost}",
            tokens(usage.input),
            tokens(usage.output)
        ),
    )
}

/// The record's sections, every one present; an empty one says nothing was recorded.
pub fn view(record: &BuildRecord, feed: &Feed) -> RecordView {
    let when = |at: &str| crate::model::work::when(at, feed);
    let verification = RecordSection {
        id: "record-gate",
        title: "Gate attempts",
        note: None,
        lines: if record.verification.is_empty() {
            vec![none("No gate attempt")]
        } else {
            record.verification.iter().map(gate_line).collect()
        },
    };
    let reviews = RecordSection {
        id: "record-reviews",
        title: "Review rounds",
        note: None,
        lines: if record.reviews.is_empty() {
            vec![none("No review round")]
        } else {
            record.reviews.iter().flat_map(review_lines).collect()
        },
    };
    let decisions = RecordSection {
        id: "record-decisions",
        title: "Your decisions",
        note: None,
        lines: if record.decisions.is_empty() {
            vec![none("No decision")]
        } else {
            record
                .decisions
                .iter()
                .map(|decision| RecordLine {
                    detail: decision
                        .answer
                        .as_ref()
                        .map(|answer| format!("you answered: {answer}")),
                    ..line(
                        None,
                        Hue::Neutral,
                        format!(
                            "{} · {}: {}",
                            when(&decision.at),
                            decision.kind.replace('_', " "),
                            decision.text
                        ),
                    )
                })
                .collect()
        },
    };
    let timeline = RecordSection {
        id: "record-timeline",
        title: "Timeline",
        note: None,
        lines: if record.timeline.is_empty() {
            vec![none("No step")]
        } else {
            record
                .timeline
                .iter()
                .map(|step| RecordLine {
                    detail: step.detail.clone(),
                    ..line(
                        None,
                        Hue::Neutral,
                        format!("{} · {}", when(&step.at), step.phase.replace('_', " ")),
                    )
                })
                .collect()
        },
    };
    let usage = RecordSection {
        id: "record-usage",
        title: "Usage",
        note: Some("API-equivalent USD is not a bill".into()),
        lines: vec![
            usage_line("Parent agent", &record.usage.parent),
            usage_line("Child agents", &record.usage.children),
            usage_line("Reviewers", &record.usage.reviewers),
            usage_line("Total", &record.usage.total),
        ],
    };
    RecordView {
        title: if record.title.trim().is_empty() {
            "untitled".into()
        } else {
            record.title.clone()
        },
        outcome: outcome(record),
        sections: vec![
            changes(record),
            verification,
            reviews,
            decisions,
            timeline,
            usage,
        ],
    }
}

/// The pager over the Board tab.
pub fn pager(cx: &Cx<'_>, close: Action) -> RecordPager {
    let record = loaded(cx.feed, cx.id.as_str());
    let summary = match record {
        Some(record) => format!("Build record · {}", outcome(record).text),
        None => "Build record".into(),
    };
    RecordPager {
        part: part(cx, Ok(target(cx)), Some(CAPABILITY), summary, Hue::Neutral),
        view: record.map(|record| view(record, cx.feed)),
        close,
    }
}

/// The record's read: while the pager shows it (`open`), and once the build's exit is under way,
/// for the gate attempts and review rounds its result card lists. The host re-announces it on
/// every change, which reads it again.
pub fn fetches(cx: &Cx<'_>, open: bool) -> Option<Action> {
    if *cx.role != SessionRole::Build {
        return None;
    }
    let exiting = cx
        .exit()
        .is_some_and(|exit| !matches!(exit.phase, ExitPhase::None | ExitPhase::Other));
    (open || exiting)
        .then(|| load(cx, Ok(target(cx)), Some(CAPABILITY)))
        .flatten()
}
