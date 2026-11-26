//! A task's gate attempt as corrald observes it (`GateAttempt` on the task's row and detail):
//! whether a gate is confirmed to run, its command, step and times, the runner's heartbeat apart
//! from its last output, and the attempt's log (`DetailRef::GateAttempt`), read one bounded
//! window at a time: the newest while it runs, followed, held still while the owner reads back,
//! and older windows on request. Nothing here is inferred from the task's status: without an
//! attempt there is no gate activity to show, and this device being offline makes a live
//! attempt's liveness unknown rather than running.

use corral_protocol::detail::{DetailBody, DetailRef};
use corral_protocol::rows::TaskRow;
use corral_types::{CheckResult, GateAttempt, GateAttemptPage, GateAttemptState, Role};

use crate::model::Id;
use crate::model::board::Mark;
use crate::model::daemon::{DetailCache, Feed, Link, age, seconds};
use crate::model::hue::Hue;
use crate::model::session::ReadState;
use crate::model::session::duration;
use crate::model::session::transcript::{Line, LineClass, line, size};
use crate::model::status::Stage;

/// How often an open Gate view re-reads a live attempt's newest window, in seconds. The read
/// is conditional, so a quiet gate costs an unchanged reply.
pub const POLL_SECS: u64 = 2;
/// A line longer than this is cut where it is drawn; the window bounds the bytes, not lines.
const MAX_LINE_CHARS: usize = 2000;

/// The attempt a Gate view shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateOpen {
    pub task: Id,
    pub attempt: String,
}

/// Which window of the log shows. The newest follows while the attempt runs; `held` keeps the
/// newest as it was when the owner scrolled back, so the text does not slide under them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GateUi {
    /// An older window: the bytes that end here.
    pub before: Option<u64>,
    pub held: Option<GateAttemptPage>,
}

/// Whether the attempt's runner was last said to be at work: running, or quiet past its
/// heartbeat (stale). Neither is a result.
pub fn live(attempt: &GateAttempt) -> bool {
    matches!(
        attempt.state,
        GateAttemptState::Running | GateAttemptState::Stale
    )
}

/// The attempt a task's row stands for: a live one always (it is confirmed activity), a finished
/// one only when it belongs to the task's current run.
pub fn current(row: &TaskRow) -> Option<&GateAttempt> {
    row.gate_attempt.as_ref().filter(|attempt| {
        live(attempt)
            || (row
                .run
                .as_ref()
                .filter(|run| run.role == Role::Worker)
                .is_none_or(|run| run.id == attempt.run)
                && row
                    .gate
                    .as_ref()
                    .is_none_or(|check| check.key == attempt.sha))
    })
}

/// The task's Gate view, when it has an attempt to show.
pub fn open_of(row: &TaskRow) -> Option<GateOpen> {
    current(row).map(|attempt| GateOpen {
        task: Id::new(&row.task.id),
        attempt: attempt.id.clone(),
    })
}

/// The rail's gate mark the attempt sets: a live one is at work whatever an earlier check said;
/// a finished one only stands where no check of the current candidate does.
pub fn mark(attempt: &GateAttempt) -> Mark {
    match attempt.state {
        GateAttemptState::Running | GateAttemptState::Stale => Mark::Active,
        GateAttemptState::Passed => Mark::Done,
        GateAttemptState::Failed | GateAttemptState::Interrupted | GateAttemptState::Error => {
            Mark::Fail
        }
    }
}

/// `step 2 of 5`, `5 steps`; empty when neither is known.
fn steps(attempt: &GateAttempt) -> String {
    match (attempt.step, attempt.steps) {
        (Some(step), 0) => format!("step {step}"),
        (Some(step), total) => format!("step {step} of {total}"),
        (None, 0) => String::new(),
        (None, 1) => "1 step".to_owned(),
        (None, total) => format!("{total} steps"),
    }
}

fn sha7(sha: &str) -> String {
    sha.chars().take(7).collect()
}

/// The gate stage and its sentence for an attempt that has no check to speak for it.
pub fn stage(attempt: &GateAttempt) -> (Stage, String) {
    let on = format!("on {}", sha7(&attempt.sha));
    match attempt.state {
        GateAttemptState::Running => {
            let steps = steps(attempt);
            let words = if steps.is_empty() {
                format!("running {on}")
            } else {
                format!("running {steps} {on}")
            };
            (Stage::Now, words)
        }
        GateAttemptState::Stale => (
            Stage::Stale,
            format!("runner not heard from {on}; no result yet"),
        ),
        GateAttemptState::Interrupted => (
            Stage::Fail,
            format!("interrupted {on}: the runner exited without a result"),
        ),
        GateAttemptState::Error => (Stage::Fail, format!("could not run {on}")),
        GateAttemptState::Failed => (Stage::Fail, format!("failed {on}")),
        GateAttemptState::Passed => (Stage::Ok, format!("passed {on}")),
    }
}

/// What a row says of a live attempt: `gate running · step 2 of 5`, or that its liveness is
/// unknown while this device is offline.
pub fn phrase(attempt: &GateAttempt, feed: &Feed) -> Option<String> {
    if !live(attempt) {
        return None;
    }
    if feed.link != Link::Up {
        return Some("gate liveness unknown offline".to_owned());
    }
    Some(match attempt.state {
        GateAttemptState::Stale => "gate runner not heard from".to_owned(),
        _ => match steps(attempt) {
            steps if steps.is_empty() => "gate running".to_owned(),
            steps => format!("gate running · {steps}"),
        },
    })
}

/// Where the attempt stands, as the Gate view's head says it.
pub fn standing(attempt: &GateAttempt, online: bool) -> (Hue, String) {
    match attempt.state {
        GateAttemptState::Running | GateAttemptState::Stale if !online => (
            Hue::Held,
            "◌ liveness unknown: this device is offline; last known state shown".to_owned(),
        ),
        GateAttemptState::Running => (Hue::Corral, "◐ running".to_owned()),
        GateAttemptState::Stale => (
            Hue::Held,
            "◌ stale: corrald has not heard from the runner; no result yet".to_owned(),
        ),
        GateAttemptState::Interrupted => (
            Hue::Fail,
            "✗ interrupted: the runner exited without a result".to_owned(),
        ),
        GateAttemptState::Error => (Hue::Fail, "✗ error: the gate could not run".to_owned()),
        GateAttemptState::Failed => (Hue::Fail, "✗ failed".to_owned()),
        GateAttemptState::Passed => (Hue::Ok, "✓ passed".to_owned()),
    }
}

/// `3s ago`, or `none yet`.
fn ago(at: Option<jiff::Timestamp>, now: u64) -> String {
    match at.and_then(seconds) {
        Some(at) if now > 0 => format!("{} ago", age(Some(at.min(now)), now)),
        Some(_) => "reported".to_owned(),
        None => "none yet".to_owned(),
    }
}

/// The attempt's facts in order: command, step, elapsed, heartbeat, last output, candidate.
pub fn facts(attempt: &GateAttempt, now: u64) -> Vec<(&'static str, String)> {
    let started = seconds(attempt.started);
    let ended = attempt.finished.and_then(seconds).or_else(|| {
        (attempt.state == GateAttemptState::Interrupted)
            .then(|| attempt.heartbeat.unwrap_or(attempt.started))
            .and_then(seconds)
    });
    let elapsed = match (started, ended) {
        (Some(start), Some(end)) => Some(format!("ran {}", duration(end.saturating_sub(start)))),
        (Some(start), None) if now > 0 => Some(duration(now.saturating_sub(start))),
        _ => None,
    };
    let steps = steps(attempt);
    let mut facts = vec![(
        "Command",
        attempt.command.as_deref().map_or_else(
            || "not reported yet".to_owned(),
            |command| format!("$ {command}"),
        ),
    )];
    if !steps.is_empty() {
        facts.push(("Step", steps));
    }
    facts.extend(elapsed.map(|elapsed| ("Elapsed", elapsed)));
    facts.push(("Runner heartbeat", ago(attempt.heartbeat, now)));
    facts.push(("Last output", ago(attempt.last_output, now)));
    facts.push((
        "Candidate",
        format!(
            "{} · run {} · round {}",
            sha7(&attempt.sha),
            attempt.run,
            attempt.round.max(1)
        ),
    ));
    facts
}

/// The attempt's log as the client reads it: its newest window, or the one ending at `before`.
pub fn target(task: &str, attempt: &str, before: Option<u64>) -> DetailRef {
    DetailRef::GateAttempt {
        task: task.to_owned(),
        attempt: attempt.to_owned(),
        before,
    }
}

fn page(cache: Option<&DetailCache>) -> Option<&GateAttemptPage> {
    match cache?.body()? {
        DetailBody::GateAttempt(page) => Some(page),
        _ => None,
    }
}

/// The newest attempt record this device holds: the newest window's, the task row's or its
/// detail's, a finished one over a live one, else the one heard from last.
pub fn attempt<'f>(feed: &'f Feed, open: &GateOpen) -> Option<&'f GateAttempt> {
    let id = open.task.as_str();
    let read = page(feed.detail(&target(id, &open.attempt, None))).map(|page| &page.attempt);
    let row = feed
        .board
        .task(id)
        .and_then(|task| task.row.gate_attempt.as_ref());
    let detail =
        crate::model::work::loaded_task(feed, id).and_then(|detail| detail.gate_attempt.as_ref());
    [detail, row, read]
        .into_iter()
        .flatten()
        .filter(|attempt| attempt.id == open.attempt)
        .max_by_key(|attempt| {
            (
                !live(attempt),
                attempt.heartbeat,
                attempt.state == GateAttemptState::Stale,
                attempt.last_output,
            )
        })
}

/// Whether an open view should read the newest window again now, `last` being when it last
/// did: while the attempt (or the window last read) is live, the link is up and no read is on
/// its way or failed. A failed read waits for Retry rather than looping.
pub fn due(feed: &Feed, open: &GateOpen, last: u64) -> bool {
    if feed.link != Link::Up || feed.now < last.saturating_add(POLL_SECS) {
        return false;
    }
    let cache = feed.detail(&target(open.task.as_str(), &open.attempt, None));
    if cache.is_some_and(|cache| cache.loading || cache.error.is_some()) {
        return false;
    }
    let window_live = page(cache).is_none_or(|page| live(&page.attempt));
    window_live || attempt(feed, open).is_some_and(live)
}

/// The Gate view.
#[derive(Debug, Clone, PartialEq)]
pub struct GateView {
    pub title: String,
    pub sub: String,
    pub standing: (Hue, String),
    pub facts: Vec<(&'static str, String)>,
    /// The retained check log's artifact reference (`T-12:3`), once the attempt has one.
    pub artifact: Option<String>,
    /// The check's result and notes, once it is recorded.
    pub result: Option<(Hue, String)>,
    /// `▼ following the newest output`, `⏸ paused · 1.2 KB new`, `‖ showing older output`.
    pub follow: Option<(Hue, String)>,
    /// `last 8.0 KB of 1.2 MB`.
    pub span: Option<String>,
    pub lines: Vec<Line>,
    /// The byte after the last one shown.
    pub end: u64,
    pub read: Option<(Hue, String)>,
    pub retry: Option<DetailRef>,
    pub empty: Option<String>,
    /// The bytes before the shown ones end here.
    pub older: Option<u64>,
    /// An older or held window shows: Newest returns to the live end.
    pub newest: bool,
}

/// The shown window's lines: each cut at `MAX_LINE_CHARS`, the first marked partial when the
/// window starts inside the log.
fn lines(page: &GateAttemptPage) -> Vec<Line> {
    page.text
        .lines()
        .enumerate()
        .map(|(at, text)| {
            // Keep the newest suffix of a long line (for example, a progress line).
            let skipped = text.chars().count().saturating_sub(MAX_LINE_CHARS);
            let mut shown = String::new();
            if skipped > 0 || (at == 0 && page.start > 0) {
                shown.push('…');
            }
            shown.extend(text.chars().skip(skipped).map(|ch| {
                if ch.is_control() && ch != '\t' {
                    '�'
                } else {
                    ch
                }
            }));
            line(&shown, LineClass::Plain)
        })
        .collect()
}

/// The Gate view of `open` with `ui`'s window.
pub fn view(feed: &Feed, open: &GateOpen, ui: &GateUi) -> GateView {
    let online = feed.link == Link::Up;
    let task = open.task.as_str();
    let newest_target = target(task, &open.attempt, None);
    let newest = page(feed.detail(&newest_target));
    let shown_target = ui.before.map_or_else(
        || newest_target.clone(),
        |before| target(task, &open.attempt, Some(before)),
    );
    let cache = feed.detail(&shown_target);
    let shown = match (ui.before, ui.held.as_ref()) {
        (Some(_), _) => page(cache),
        (None, Some(held)) => Some(held),
        (None, None) => newest,
    };
    let attempt = attempt(feed, open);
    let follow = match (attempt.is_some_and(live), ui.before, &ui.held) {
        (_, Some(_), _) => Some((
            Hue::Neutral,
            "‖ showing older output · Newest returns to the end".to_owned(),
        )),
        (false, None, _) => None,
        (true, None, Some(held)) => {
            let arrived = newest.map_or(0, |page| page.total.saturating_sub(held.end));
            Some((
                Hue::Neutral,
                match arrived {
                    0 => "⏸ paused".to_owned(),
                    bytes => format!("⏸ paused · {} new", size(bytes)),
                },
            ))
        }
        (true, None, None) if !online => None,
        (true, None, None) => Some((Hue::Corral, "▼ following the newest output".to_owned())),
    };
    let read = ReadState::of(cache, feed);
    let check = attempt.and_then(|attempt| attempt.check.as_ref());
    GateView {
        title: format!("Gate · {task}"),
        sub: format!("attempt {}", open.attempt),
        standing: attempt.map_or_else(
            || (Hue::Neutral, "not read yet".to_owned()),
            |attempt| standing(attempt, online),
        ),
        facts: attempt.map_or_else(Vec::new, |attempt| facts(attempt, feed.now)),
        artifact: check
            .and_then(|check| check.log.as_deref())
            .and_then(|log| {
                corral_types::references_in(log)
                    .into_iter()
                    .find(|reference| reference.artifact().is_some())
            })
            .map(|reference| reference.as_str().to_owned()),
        result: check.map(|check| {
            let (hue, word) = match check.result {
                CheckResult::Pass => (Hue::Ok, "check passed"),
                CheckResult::Fail => (Hue::Fail, "check failed"),
            };
            let words = match &check.notes {
                Some(notes) if !notes.trim().is_empty() => format!("{word} · {}", notes.trim()),
                _ => word.to_owned(),
            };
            (hue, words)
        }),
        follow,
        span: shown.and_then(|page| {
            (page.end.saturating_sub(page.start) < page.total).then(|| {
                let bytes = size(page.end.saturating_sub(page.start));
                if page.end >= page.total {
                    format!("last {bytes} of {}", size(page.total))
                } else {
                    format!("{bytes} from byte {} of {}", page.start, size(page.total))
                }
            })
        }),
        lines: shown.map_or_else(Vec::new, lines),
        end: shown.map_or(0, |page| page.end),
        read: match &read {
            ReadState::Failed(_) | ReadState::Stale { .. } => Some((read.hue(), read.words())),
            ReadState::NotRead | ReadState::Reading if shown.is_none() => {
                Some((Hue::Neutral, "Reading the gate log…".to_owned()))
            }
            _ => None,
        },
        retry: (online && cache.is_some_and(|cache| cache.error.is_some())).then_some(shown_target),
        empty: shown
            .filter(|page| page.text.is_empty())
            .map(|_| "No output yet.".to_owned()),
        older: shown.filter(|page| page.start > 0).map(|page| page.start),
        newest: ui.before.is_some() || ui.held.is_some(),
    }
}

#[cfg(test)]
#[path = "gate_tests.rs"]
pub(super) mod tests;
