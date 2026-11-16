//! Feed-backed usage views for desktop tables, phone blocks and terminal lines.
//! Usage is visibility only.

mod completeness;
mod coverage;
mod daily;
mod live;
mod quota;
mod totals;
mod window;

pub use completeness::completeness_label;
pub use live::{Inputs, page as live_page, series_request};

use crate::model::Effect;

/// The selected period.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Period {
    #[default]
    Today,
    Week,
    Month,
}

impl Period {
    /// Period selector options in display order.
    pub const ALL: [Period; 3] = [Period::Today, Period::Week, Period::Month];

    /// Label shown in the period selector.
    pub fn label(self) -> &'static str {
        match self {
            Period::Today => "Today",
            Period::Week => "7 days",
            Period::Month => "30 days",
        }
    }
}

/// Selection state for the usage screen.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct State {
    pub period: Period,
}

impl State {
    /// Select a period without emitting effects.
    pub fn select_period(&mut self, period: Period) -> Vec<Effect> {
        self.period = period;
        Vec::new()
    }
}

/// Usage page title.
pub const TITLE: &str = "Usage";
/// Phone back destination label.
pub const BACK_LABEL: &str = "More";
/// Desktop page subtitle.
pub const SUBTITLE: &str = "cost and quota per provider and profile · visibility only, no routing";
/// The small quiet word following a provider cell's desktop usage percentage.
pub const USED_LABEL: &str = "used";
/// Profile table heading.
pub const PROFILE_TITLE: &str = "By profile";
/// Session table heading.
pub const SESSION_TITLE: &str = "By session";
/// Session table column labels.
pub const SESSION_HEADERS: [&str; 5] = ["Session", "Working on", "Profile", "Tokens", "Cost"];
/// Heading over a provider's quota observations.
pub const QUOTA_TITLE: &str = "Quota source";
/// Stands in for a figure corral does not hold; the page's closing note says so once.
pub const UNKNOWN: &str = "—";
/// Said once in a provider's card when corral holds no quota observation for it.
pub const NO_QUOTA: &str = "No quota reported";
/// The group of usage recorded without a provider connection.
pub const UNATTRIBUTED: &str = "Unattributed";
/// Beside [`UNATTRIBUTED`], where a provider's window would be.
pub const UNATTRIBUTED_ASIDE: &str = "no provider recorded";
/// A usage row recorded without a profile.
pub const NO_PROFILE: &str = "no profile recorded";

/// Usage band for a provider or a profile meter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Below 70 percent, or no quota known.
    None,
    /// At least 70 percent but below 90 percent.
    Warn,
    /// At least 90 percent.
    Bad,
}

/// Resolve the window use thresholds used by the mockup.
pub fn level(used: f32) -> Level {
    if used >= 0.9 {
        Level::Bad
    } else if used >= 0.7 {
        Level::Warn
    } else {
        Level::None
    }
}

fn percent(value: f32) -> u32 {
    (value * 100.0).round() as u32
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

/// Samples of a window's use (fractions) at their local times of day (hours since midnight,
/// ascending).
#[derive(Debug, Clone, PartialEq)]
pub struct Chart {
    pub series: Vec<f32>,
    pub hours: Vec<f32>,
    pub label: String,
}

/// A coordinate in the chart's supplied view box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}
/// A coloured threshold rectangle in the chart.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Zone {
    pub level: Level,
    pub origin: Point,
    pub width: f32,
    pub height: f32,
}
/// A chart axis label and its baseline position.
#[derive(Debug, Clone, PartialEq)]
pub struct AxisLabel {
    pub text: String,
    /// Text position; tick labels are centered on x.
    pub position: Point,
}
/// Geometric chart data, including the filled-area polygon and final point.
#[derive(Debug, Clone, PartialEq)]
pub struct BandChart {
    /// 90 to 100 and 70 to 90 percent zones, in that order.
    pub zones: [Zone; 2],
    pub vertical_labels: Vec<AxisLabel>,
    /// Whole hours, evenly stepped across the samples' span.
    pub hour_labels: Vec<AxisLabel>,
    pub line: Vec<Point>,
    /// Series closed to the zero baseline for an area renderer.
    pub area: Vec<Point>,
    pub last: Option<Point>,
    pub accessibility_label: String,
}

/// Hour labels the chart holds at most, so they never crowd one another.
const HOUR_LABELS: f32 = 6.0;
/// The plot's left gutter (for the band labels) and right margin (for the last hour label).
const PLOT_LEFT: f32 = 22.0;
const PLOT_RIGHT: f32 = 10.0;

/// Project a window's samples and threshold bands into the requested chart dimensions. Samples
/// sit at their times, so a missing hour shows as a longer segment, not a squeezed axis; the
/// hour labels step evenly over whole hours; the band labels mark the 70 and 90 edges, as far
/// apart as the bands are tall.
pub fn band_chart(chart: &Chart, width: f32, height: f32) -> BandChart {
    let plot_width = (width - PLOT_LEFT - PLOT_RIGHT).max(0.0);
    let (start, end) = match (chart.hours.first(), chart.hours.last()) {
        (Some(&first), Some(&last)) => (first.floor(), last.ceil().max(first.floor() + 1.0)),
        _ => (0.0, 1.0),
    };
    let span = end - start;
    let x = |hour: f32| PLOT_LEFT + (hour - start) / span * plot_width;
    let y = |v: f32| 4.0 + (1.0 - v.clamp(0.0, 1.0)) * (height - 26.0);
    let zones = [
        Zone {
            level: Level::Bad,
            origin: Point {
                x: PLOT_LEFT,
                y: y(1.0),
            },
            width: plot_width,
            height: y(0.9) - y(1.0),
        },
        Zone {
            level: Level::Warn,
            origin: Point {
                x: PLOT_LEFT,
                y: y(0.9),
            },
            width: plot_width,
            height: y(0.7) - y(0.9),
        },
    ];
    // Baselines a third of a label's height under each edge, so the figure centres on it.
    let vertical_labels = [("90", 0.9, 3.0), ("70", 0.7, 3.0), ("0", 0.0, 0.0)]
        .into_iter()
        .map(|(text, at, lift)| AxisLabel {
            text: text.to_owned(),
            position: Point {
                x: 0.0,
                y: y(at) + lift,
            },
        })
        .collect();
    let step = (span / HOUR_LABELS).ceil().max(1.0);
    let steps = (span / step).floor() as u32;
    let hour_labels = (0..=steps)
        .map(|k| {
            let hour = start + k as f32 * step;
            AxisLabel {
                text: format!("{:02}", (hour as u32) % 24),
                position: Point {
                    x: x(hour),
                    y: height - 2.0,
                },
            }
        })
        .collect();
    let line: Vec<Point> = chart
        .series
        .iter()
        .zip(&chart.hours)
        .map(|(&v, &hour)| Point {
            x: x(hour),
            y: y(v),
        })
        .collect();
    let last = line.last().copied();
    let mut area = line.clone();
    if let (Some(first), Some(end)) = (line.first(), last) {
        area.push(Point {
            x: end.x,
            y: y(0.0),
        });
        area.push(Point {
            x: first.x,
            y: y(0.0),
        });
    }
    BandChart {
        zones,
        vertical_labels,
        hour_labels,
        line,
        area,
        last,
        accessibility_label: chart.label.clone(),
    }
}

/// One provider: the desktop card, the phone's stacked block and the terminal's gauge line.
/// Every window fact (window, use, headroom, reset) comes from one quota observation, the first
/// of `quotas`, so the header can never call unknown what the details under it know.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderView {
    pub id: String,
    /// The connection, or [`UNATTRIBUTED`].
    pub label: String,
    /// Beside the label: "5 h window" when the observation names one, [`UNATTRIBUTED_ASIDE`]
    /// for the unattributed group, else nothing.
    pub aside: Option<String>,
    /// The big figure: window use, the period's cost when no quota is known, or [`UNKNOWN`].
    pub big: String,
    /// The quiet words after the big figure.
    pub big_label: String,
    /// Fraction of the window used, when known.
    pub used: Option<f32>,
    /// The 70/90 band of `used`; `Level::None` when use is unknown.
    pub level: Level,
    /// Whether corral holds a quota observation for this provider.
    pub quota_known: bool,
    /// The observation in words, "34 of 100 used · 66 left · resets 20:39", or [`NO_QUOTA`].
    pub quota_line: String,
    /// The observation records the window as exhausted.
    pub exhausted: bool,
    /// Headroom alone, for the terminal's gauge line: "66 left", "exhausted", "no quota".
    pub headroom: String,
    /// The costs the big figure does not already say: "today ~$22.70 · 7 days ~$141".
    pub costs: String,
    /// Where this provider's coverage differs from the page's; the page says the rest once.
    pub note: Option<String>,
    /// Today's window samples; absent when corral recorded fewer than two.
    pub chart: Option<Chart>,
    /// The phone block's profile lines; the desktop and terminal list them in the table.
    pub profiles: Vec<PhoneProfile>,
    /// Where the quota facts come from, the header's observation first. Observations that hold
    /// no fact at all are left out: the card's [`NO_QUOTA`] already says so.
    pub quotas: Vec<QuotaLine>,
}

/// One quota observation as a single quiet provenance line: its profile, its amounts (for every
/// observation but the header's, whose amounts the card already shows), source, scope and when
/// it was observed, leaving out what the observation does not name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaLine {
    pub text: String,
    pub exhausted: bool,
    /// The observation's own coverage, when partial.
    pub completeness: Option<String>,
}

/// A profile line inside a phone provider block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhoneProfile {
    pub name: String,
    /// False for [`NO_PROFILE`], which is drawn quieter than a name.
    pub named: bool,
    /// "34% · ~$23 · 12 unpriced", or the cost alone.
    pub detail: String,
}

/// One provider's profiles in the profile table, under a row with the provider's own total.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileGroup {
    pub id: String,
    pub label: String,
    /// The provider's cost over the period.
    pub cost: String,
    pub rows: Vec<ProfileRow>,
}

/// One profile table row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileRow {
    pub profile: String,
    /// False for [`NO_PROFILE`], which is drawn quieter than a name.
    pub named: bool,
    /// The row's own unpriced or undated entries: "12 unpriced".
    pub gaps: Option<String>,
    /// Meter fill as a percentage; None when the share is unknown.
    pub share_percent: Option<u32>,
    /// "34%", or [`UNKNOWN`].
    pub share: String,
    /// Meter level follows provider use, not profile use.
    pub level: Level,
    /// The period's cost, or [`UNKNOWN`].
    pub cost: String,
}

/// One stacked daily cost bar: segment widths in percent of the track, in legend order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CostBar {
    pub day: String,
    pub values: [u32; 3],
    pub total_label: String,
}
/// The cost-per-day section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Costs {
    pub title: String,
    pub legend: Vec<String>,
    pub bars: Vec<CostBar>,
    /// Said in place of the bars when there are none.
    pub empty: Option<String>,
    /// The series is a retained copy; its coverage is the page's note.
    pub note: Option<String>,
}
/// One session's recorded tokens and cost; `None` is a fact corral does not hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRow {
    pub session: String,
    /// "role · task", or "role · title".
    pub what: String,
    pub profile: Option<String>,
    pub tokens: Option<String>,
    pub cost: Option<String>,
}

impl SessionRow {
    /// Profile, tokens and cost in one line, for a form with no table; a missing figure is
    /// [`UNKNOWN`].
    pub fn summary(&self) -> String {
        format!(
            "{} · {} · {}",
            self.profile.as_deref().unwrap_or(NO_PROFILE),
            self.tokens
                .as_deref()
                .map_or_else(|| format!("tokens {UNKNOWN}"), |t| format!("{t} tokens")),
            self.cost
                .clone()
                .unwrap_or_else(|| format!("cost {UNKNOWN}")),
        )
    }
}
/// The whole page.
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub providers: Vec<ProviderView>,
    /// Said when no provider has recorded usage.
    pub providers_empty: Option<&'static str>,
    /// How complete the totals are, said once above the content; None when whole.
    pub coverage: Option<String>,
    /// Profile, share of window, the period's cost.
    pub profile_headers: [&'static str; 3],
    /// Whether any profile's share of its window is known; without one the table leaves the
    /// share column out rather than fill it with [`UNKNOWN`].
    pub shares_known: bool,
    pub groups: Vec<ProfileGroup>,
    pub costs: Costs,
    /// Per-session tokens and cost, costliest first.
    pub sessions: Vec<SessionRow>,
    pub sessions_note: Option<String>,
    /// One short line on where the figures come from and what [`UNKNOWN`] means.
    pub disclaimer: &'static str,
    /// Deduplicated read errors; render once above the content.
    pub notices: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::{Chart, Level, band_chart, level};

    #[test]
    fn usage_thresholds_do_not_shift_at_band_edges() {
        assert_eq!(level(0.69), Level::None);
        assert_eq!(level(0.70), Level::Warn);
        assert_eq!(level(0.90), Level::Bad);
    }

    /// The 90 and 70 labels were drawn about two points apart, and hour labels came from every
    /// third sample, so a missing hour left a gap ("14 15 _ 17 18").
    #[test]
    fn chart_axis_labels_neither_overlap_nor_skip_hours() {
        let chart = Chart {
            series: vec![0.2, 0.4, 0.5, 0.6, 0.8],
            hours: vec![14.0, 15.0, 15.5, 17.0, 18.25],
            label: String::new(),
        };
        let drawn = band_chart(&chart, 380.0, 104.0);
        let bands: Vec<f32> = drawn.vertical_labels.iter().map(|l| l.position.y).collect();
        assert!(
            bands.windows(2).all(|pair| pair[1] - pair[0] >= 12.0),
            "{bands:?}"
        );
        let xs: Vec<f32> = drawn.hour_labels.iter().map(|l| l.position.x).collect();
        let gaps: Vec<f32> = xs.windows(2).map(|pair| pair[1] - pair[0]).collect();
        assert!(
            gaps.iter().all(|gap| (gap - gaps[0]).abs() < 0.01),
            "{xs:?}"
        );
        let hours: Vec<&str> = drawn.hour_labels.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(hours, ["14", "15", "16", "17", "18", "19"]);
    }
}
