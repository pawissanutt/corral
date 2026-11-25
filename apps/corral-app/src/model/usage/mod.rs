//! The Usage page: what agent hosts reported with `corral usage add` (one record per model
//! request) over a period of local days, totalled by profile, model, task or day beside the runs
//! corral recorded, which share of those runs the totals cover and why the rest are missing.
//! Usage is visibility only.

mod live;
mod period;

pub use live::page;
pub use period::{Days, Local, MOST_DAYS, PERIODS, Period, State, local, series_request};

use corral_types::UsageBy;

use crate::model::home::Tile;

/// Usage page title.
pub const TITLE: &str = "Usage";
/// Stands in for a figure corral does not hold; the page's closing note says so once.
pub const UNKNOWN: &str = "—";

/// The groupings in display order, with their switch labels.
pub const GROUPINGS: [(UsageBy, &str); 4] = [
    (UsageBy::Profile, "Profile"),
    (UsageBy::Model, "Model"),
    (UsageBy::Task, "Task"),
    (UsageBy::Day, "Day"),
];

/// Where the period's usage stands. A failed read is not a status here: the page's own
/// `notices` say it once, with the daemon's cause.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ready,
    /// The last usage received, kept while corrald is unreachable or failing.
    Stale,
    Loading,
}

/// Under the tiles: what the totals leave out in one quiet line, and, behind Details, every
/// reason in full with the notes on where the figures come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gaps {
    /// "Cost reported for 1,514 of 31,843 requests · estimate covers 23,686 of 31,843 · …",
    /// or [`ALL_COVERED`] when nothing is missing.
    pub line: String,
    /// Something is missing: the line leads with the held ◐ marker.
    pub partial: bool,
    /// The Details section is expanded.
    pub open: bool,
    /// The period's reasons in full: the run-coverage sentence first.
    pub reasons: Vec<String>,
    /// Each row with reasons, in row order: (the row's key as shown, its reasons).
    pub groups: Vec<(String, String)>,
    /// The full estimate note with rate sources, the quota note, the disclaimer, the zone.
    pub notes: Vec<String>,
}

/// The coverage line when nothing is missing.
pub const ALL_COVERED: &str = "Every request reported tokens and cost.";
/// The Details toggle's labels, closed and open.
pub const DETAILS: &str = "Details";
pub const HIDE_DETAILS: &str = "Hide details";

/// One day of the period in the per-day view: its height against the period's busiest day and
/// its facts in words.
#[derive(Debug, Clone, PartialEq)]
pub struct DayBar {
    /// 0 for a day with no tokens, 1 for the busiest day.
    pub fraction: f32,
    /// Input and output tokens reported, which a graphical chart plots against its scale.
    pub tokens: u64,
    /// The day under its bar: `25 Nov`.
    pub tick: String,
    /// The day's figures in words, as the Day grouping's row gives them.
    pub hint: String,
}

/// The period's days side by side, by tokens reported.
#[derive(Debug, Clone, PartialEq)]
pub struct DayStrip {
    pub title: &'static str,
    /// The busiest day, or that no day reported any tokens.
    pub note: String,
    pub bars: Vec<DayBar>,
    /// The most tokens any day reported, which a graphical chart's scale reaches.
    pub peak: u64,
    /// What a graphical chart says until a day is pointed at, touched or reached by the keys.
    pub prompt: &'static str,
    /// The first and last day under the strip.
    pub first: String,
    pub last: String,
    /// What a screen reader hears for the whole strip.
    pub spoken: String,
}

impl DayStrip {
    /// The bars in at most `room` columns: a column holds the busiest of its days.
    pub fn columns(&self, room: usize) -> Vec<f32> {
        let room = room.max(1);
        let per = self.bars.len().div_ceil(room).max(1);
        self.bars
            .chunks(per)
            .map(|days| days.iter().map(|day| day.fraction).fold(0.0, f32::max))
            .collect()
    }
}

/// One group's totals beside the runs it covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupRow {
    /// `usage-row-<key>`.
    pub id: String,
    pub key: String,
    /// False for a group without a key ("no profile recorded"), drawn quieter than a name.
    pub named: bool,
    /// A task's title, after its id.
    pub detail: Option<String>,
    /// The group's runs that reported usage, of those started in the period: "18" when every
    /// run did, "12 of 14" when not.
    pub covered: String,
    /// Some of the group's runs reported no usage: the coverage is held in tone.
    pub runs_missing: bool,
    /// Why this group's figures are partial (missing runs, unpriced requests), in plain words,
    /// held in tone.
    pub reasons: Option<String>,
    pub requests: String,
    pub input: String,
    pub output: String,
    /// What the agents reported as the price.
    pub cost: String,
    /// The API-equivalent estimate at configured or known rates: not a charge, not a quota.
    pub estimate: String,
}

impl GroupRow {
    /// The figures in one line, for a form with no table.
    pub fn summary(&self) -> String {
        format!(
            "{} runs · {} requests · {} in · {} out · est. {} · reported {}",
            self.covered, self.requests, self.input, self.output, self.estimate, self.cost
        )
    }
}

/// The whole page.
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub status: Status,
    /// The head's line: the period's days and how many.
    pub head: String,
    /// Tokens, the estimate, the reported cost and the runs; empty until corrald has answered
    /// for the period.
    pub stats: Vec<Tile>,
    /// What the totals leave out; None until corrald has answered for the period.
    pub gaps: Option<Gaps>,
    pub days: Option<DayStrip>,
    /// "By profile".
    pub group_title: String,
    pub headers: [&'static str; 6],
    pub rows: Vec<GroupRow>,
    /// The period's totals under the rows.
    pub total: Option<GroupRow>,
    /// One line under the table: what the estimate column is and is not.
    pub estimate_note: String,
    /// Said in place of the rows.
    pub rows_empty: Option<&'static str>,
    /// Deduplicated read errors; render once above the content.
    pub notices: Vec<String>,
    /// The custom range's fields, while Custom is chosen.
    pub custom: Option<CustomRange>,
}

/// The custom range's two fields (their values live in [`State`]), Apply, and why the typed
/// range was not applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomRange {
    pub from: &'static str,
    pub to: &'static str,
    pub placeholder: &'static str,
    pub apply: &'static str,
    pub refused: Option<String>,
    /// What the fields take, said quietly beside them.
    pub hint: String,
}

/// Dollars: whole above ten (`$1,061`), cents below (`$7.60`).
fn money(value: f64) -> String {
    if value > 0.0 && value < 0.005 {
        "<$0.01".to_owned()
    } else if value >= 10.0 {
        format!("${}", count(value.round() as u64))
    } else {
        format!("${value:.2}")
    }
}

/// A count with thousands separated: `98,249`.
fn count(n: u64) -> String {
    let digits = n.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (at, digit) in digits.chars().enumerate() {
        if at > 0 && (digits.len() - at) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// Tokens at a glance: `950`, `12.4k`, `3.1M`.
fn tokens(count: u64) -> String {
    match count {
        0..1_000 => count.to_string(),
        1_000..1_000_000 => format!("{:.1}k", count as f64 / 1_000.0),
        _ => format!("{:.1}M", count as f64 / 1_000_000.0),
    }
}

/// `1 run` or `3 runs`.
fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{} {}", count(n), if n == 1 { one } else { many })
}
