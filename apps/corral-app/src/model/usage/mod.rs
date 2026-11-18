//! The Usage page: what agent hosts reported with `corral usage add` (one record per model
//! request), totalled by profile, model, task or day beside the runs corral recorded, and which
//! share of the runs those totals cover. Usage is visibility only.

mod live;

pub use live::page;

use corral_protocol::detail::DetailRef;
use corral_types::UsageBy;

use crate::model::Effect;

/// Usage page title.
pub const TITLE: &str = "Usage";
/// Desktop page subtitle.
pub const SUBTITLE: &str = "tokens and cost the agents reported · visibility only, no routing";
/// Stands in for a figure corral does not hold; the page's closing note says so once.
pub const UNKNOWN: &str = "—";

/// The groupings in display order, with their switch labels.
pub const GROUPINGS: [(UsageBy, &str); 4] = [
    (UsageBy::Profile, "Profile"),
    (UsageBy::Model, "Model"),
    (UsageBy::Task, "Task"),
    (UsageBy::Day, "Day"),
];

/// Selection state for the usage screen.
#[derive(Debug, Clone, PartialEq)]
pub struct State {
    pub by: UsageBy,
}

impl Default for State {
    fn default() -> Self {
        Self {
            by: UsageBy::Profile,
        }
    }
}

impl State {
    /// Select a grouping without emitting effects.
    pub fn select_by(&mut self, by: UsageBy) -> Vec<Effect> {
        self.by = by;
        Vec::new()
    }
}

/// The usage series to request for a grouping: its totals, every report and every run.
pub fn series_request(by: UsageBy) -> DetailRef {
    DetailRef::UsageSeries { by: Some(by) }
}

/// Where the recorded usage stands. A failed read is not a status here: the page's own
/// `notices` say it once, with the daemon's cause.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ready,
    /// The last usage received, kept while corrald is unreachable or failing.
    Stale,
    Loading,
}

/// What the totals cover: said first, in the held tone when runs are missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub line: String,
    pub partial: bool,
}

/// One total: its name, its figure and what qualifies it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Figure {
    pub label: &'static str,
    pub value: String,
    pub detail: Option<String>,
}

/// One group's totals beside its recorded runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupRow {
    pub key: String,
    /// False for a group without a key ("no profile recorded"), drawn quieter than a name.
    pub named: bool,
    /// A task's title, after its id.
    pub detail: Option<String>,
    pub runs: String,
    pub run_time: String,
    pub requests: String,
    pub input: String,
    pub output: String,
    pub cost: String,
}

impl GroupRow {
    /// The figures in one line, for a form with no table.
    pub fn summary(&self) -> String {
        format!(
            "{} runs · {} · {} requests · {} in · {} out · {}",
            self.runs, self.run_time, self.requests, self.input, self.output, self.cost
        )
    }
}

/// The whole page.
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub status: Status,
    /// None until corral has said how many runs it holds.
    pub coverage: Option<Coverage>,
    pub figures: Vec<Figure>,
    /// "By profile".
    pub group_title: String,
    pub headers: [&'static str; 7],
    pub rows: Vec<GroupRow>,
    /// Said in place of the rows.
    pub rows_empty: Option<&'static str>,
    /// One short line on where the figures come from and what [`UNKNOWN`] means.
    pub disclaimer: &'static str,
    /// Deduplicated read errors; render once above the content.
    pub notices: Vec<String>,
}

/// Dollars: whole above ten (`$1,061`), cents below (`$7.60`).
fn money(value: f64) -> String {
    if value >= 10.0 {
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
