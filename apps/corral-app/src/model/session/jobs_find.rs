//! Reading an opened job's output as the terminal's job log does: finding words in the loaded
//! output (every match marked, the current one apart, `3 of 12`, and what was not searched
//! because it is not loaded), and following its newest lines or holding still while the owner
//! reads (`⏸ paused · 14 new lines`), with the lines that arrived since counted rather than
//! scrolled to. Also a job's peek in the list: its last lines without leaving it.

use corral_protocol::detail::SessionJob;

use crate::model::hue::Hue;
use crate::model::text::{Span, flat, span};

use super::super::super::transcript::{Line, LineClass, line};
use super::JobOutputView;

/// How many of the newest lines a peek shows.
pub(super) const PEEK_LINES: usize = 12;

/// A search of the loaded output.
#[derive(Debug, Clone, PartialEq)]
pub struct JobSearch {
    /// `3 of 12`, `no matches`.
    pub count: String,
    pub total: usize,
    /// The current match, by its place among the matches.
    pub current: Option<usize>,
    /// The line the current match is on.
    pub line: Option<usize>,
    /// The output's lines with every match marked and the current one apart.
    pub lines: Vec<Line>,
    /// What the search did not cover because it is not loaded.
    pub note: Option<String>,
    /// Loads the output before the loaded bytes, to search it too.
    pub earlier: Option<super::super::super::Action>,
}

/// Whether the opened output follows its newest lines, and what arrived while it did not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FollowLine {
    pub hue: Hue,
    /// `▼ following the newest output`, `⏸ paused · 14 new lines`.
    pub words: String,
    pub paused: bool,
    /// Lines that started after the output was paused.
    pub new_lines: usize,
    /// The toggle's label: `Pause`, or `Follow` while paused.
    pub toggle: &'static str,
}

/// A job's last lines in the list.
#[derive(Debug, Clone, PartialEq)]
pub struct Peek {
    pub lines: Vec<Line>,
    /// Said when only the job's last two lines are known here.
    pub note: Option<String>,
}

/// Where `query` occurs in `text`, ignoring ASCII case, as byte ranges. Lower-casing ASCII
/// keeps every byte where it was, so the ranges hold in the original text.
fn matches(text: &str, query: &str) -> Vec<(usize, usize)> {
    let needle = query.to_ascii_lowercase();
    text.to_ascii_lowercase()
        .match_indices(needle.as_str())
        .map(|(at, found)| (at, at + found.len()))
        .collect()
}

/// `text` as runs: each match bold in the held hue, the current one in the owner's.
fn marked(text: &str, hits: &[(usize, usize)], current: Option<usize>) -> Vec<Span> {
    let mut runs = Vec::new();
    let mut from = 0;
    for (n, &(start, end)) in hits.iter().enumerate() {
        if start > from {
            runs.push(span(&text[from..start]));
        }
        let hue = if Some(n) == current {
            Hue::You
        } else {
            Hue::Held
        };
        runs.push(span(&text[start..end]).bold().hue(hue));
        from = end;
    }
    if from < text.len() {
        runs.push(span(&text[from..]));
    }
    runs
}

/// The search of `query` in the loaded output of `view`, standing on match `at` (clamped to the
/// last, so `usize::MAX` stands on the newest); none for an empty query.
pub fn search(view: &JobOutputView, query: &str, at: usize) -> Option<JobSearch> {
    if query.is_empty() {
        return None;
    }
    let texts: Vec<String> = view.lines.iter().map(|line| flat(&line.text)).collect();
    let found: Vec<Vec<(usize, usize)>> = texts.iter().map(|text| matches(text, query)).collect();
    let total: usize = found.iter().map(Vec::len).sum();
    let current = (total > 0).then(|| at.min(total - 1));
    let mut seen = 0;
    let mut current_line = None;
    let lines = view
        .lines
        .iter()
        .zip(&texts)
        .zip(&found)
        .enumerate()
        .map(|(n, ((shown, text), hits))| {
            let here = current
                .filter(|current| (seen..seen + hits.len()).contains(current))
                .map(|current| current - seen);
            if here.is_some() {
                current_line = Some(n);
            }
            seen += hits.len();
            if hits.is_empty() {
                shown.clone()
            } else {
                Line {
                    text: marked(text, hits, here),
                    class: shown.class,
                }
            }
        })
        .collect();
    let mut unsearched = Vec::new();
    if view.older.is_some() {
        unsearched.push("Earlier output is not loaded, so it is not searched.");
    }
    if view.newest.is_some() {
        unsearched.push("Newer output is not shown, so it is not searched.");
    }
    Some(JobSearch {
        count: match current {
            Some(current) => format!("{} of {total}", current + 1),
            None => "no matches".into(),
        },
        total,
        current,
        line: current_line,
        lines,
        note: (!unsearched.is_empty()).then(|| unsearched.join(" ")),
        earlier: view.older.clone(),
    })
}

/// How many lines of `view` started at or after byte `since` of the job's output.
pub fn arrived(view: &JobOutputView, since: u64) -> usize {
    view.at.iter().filter(|start| **start >= since).count()
}

/// The follow line of a live output: following, or paused since byte `paused_since` with what
/// arrived after it counted; none once the output does not grow here (ended, or an older page).
pub fn follow_line(view: &JobOutputView, paused_since: Option<u64>) -> Option<FollowLine> {
    if !view.live {
        return None;
    }
    Some(match paused_since {
        None => FollowLine {
            hue: Hue::Worker,
            words: "▼ following the newest output".into(),
            paused: false,
            new_lines: 0,
            toggle: "Pause",
        },
        Some(since) => {
            let new_lines = arrived(view, since);
            FollowLine {
                hue: Hue::Neutral,
                words: match new_lines {
                    0 => "⏸ paused".into(),
                    1 => "⏸ paused · 1 new line".into(),
                    n => format!("⏸ paused · {n} new lines"),
                },
                paused: true,
                new_lines,
                toggle: "Follow",
            }
        }
    })
}

/// The start byte of each line of `text`, which starts at byte `offset` of the job's output.
pub(super) fn starts(text: &str, offset: u64) -> Vec<u64> {
    let mut at = offset;
    text.split_inclusive('\n')
        .map(|piece| {
            let start = at;
            at = at.saturating_add(piece.len() as u64);
            start
        })
        .collect()
}

/// A job's peek: the newest lines of its read output when what is read is the newest, else the
/// last lines the list carries.
pub(super) fn peek(job: &SessionJob, newest: Option<&str>) -> Peek {
    if let Some(text) = newest {
        let all: Vec<&str> = text.lines().collect();
        let from = all.len().saturating_sub(PEEK_LINES);
        return Peek {
            lines: all[from..]
                .iter()
                .map(|text| line(text, LineClass::Plain))
                .collect(),
            note: all.is_empty().then(|| "No output yet.".to_owned()),
        };
    }
    Peek {
        lines: job
            .tail
            .iter()
            .map(|text| line(text, LineClass::Plain))
            .collect(),
        note: Some(if job.tail.is_empty() {
            "No output yet, or it is still being read.".to_owned()
        } else {
            "Its last lines as the job list has them.".to_owned()
        }),
    }
}
