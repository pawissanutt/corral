//! Feed-backed usage views for desktop tables, phone blocks and terminal lines.
//! Usage is visibility only.

mod completeness;
mod daily;
mod live;
mod quota;
mod totals;

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
pub const PROFILE_TITLE: &str = "By profile · current window";
/// Session table heading.
pub const SESSION_TITLE: &str = "By session";
/// Session table column labels.
pub const SESSION_HEADERS: [&str; 5] = ["Session", "Working on", "Profile", "Tokens", "Cost"];
/// The share meter's marker, as a percentage.
pub const SHARE_MARKER_PERCENT: u32 = 80;

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
fn money(value: f64) -> String {
    if value >= 10.0 {
        format!("${:.0}", value.round())
    } else {
        format!("${value:.2}")
    }
}

/// Samples of a window's use (fractions) with one tick label per sample.
#[derive(Debug, Clone, PartialEq)]
pub struct Chart {
    pub series: Vec<f32>,
    pub ticks: Vec<String>,
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
    /// Every third tick label.
    pub hour_labels: Vec<AxisLabel>,
    pub line: Vec<Point>,
    /// Series closed to the zero baseline for an area renderer.
    pub area: Vec<Point>,
    pub last: Option<Point>,
    pub accessibility_label: String,
}

/// Project a window's samples and threshold bands into the requested chart dimensions.
pub fn band_chart(chart: &Chart, width: f32, height: f32) -> BandChart {
    let plot_width = width - 26.0;
    let count = chart.series.len();
    let x = |i: usize| {
        22.0 + if count > 1 {
            i as f32 / (count - 1) as f32 * plot_width
        } else {
            0.0
        }
    };
    let y = |v: f32| 4.0 + (1.0 - v.clamp(0.0, 1.0)) * (height - 26.0);
    let zones = [
        Zone {
            level: Level::Bad,
            origin: Point { x: 22.0, y: y(1.0) },
            width: plot_width,
            height: y(0.9) - y(1.0),
        },
        Zone {
            level: Level::Warn,
            origin: Point { x: 22.0, y: y(0.9) },
            width: plot_width,
            height: y(0.7) - y(0.9),
        },
    ];
    let vertical_labels = [("90", 0.95, 3.0), ("70", 0.8, 3.0), ("0", 0.0, 0.0)]
        .into_iter()
        .map(|(text, at, lift)| AxisLabel {
            text: text.to_owned(),
            position: Point {
                x: 0.0,
                y: y(at) + lift,
            },
        })
        .collect();
    let hour_labels = chart
        .ticks
        .iter()
        .enumerate()
        .filter(|(i, _)| i % 3 == 0)
        .map(|(i, text)| AxisLabel {
            text: text.clone(),
            position: Point {
                x: x(i),
                y: height - 2.0,
            },
        })
        .collect();
    let line: Vec<Point> = chart
        .series
        .iter()
        .enumerate()
        .map(|(i, &v)| Point { x: x(i), y: y(v) })
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

/// One provider cell, also used as the basis of a stacked phone block and a terminal line.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderView {
    pub id: String,
    pub label: String,
    pub window: String,
    /// The big figure: window use, or the period's cost when no quota is known.
    pub big: String,
    /// Qualification displayed with the figure, never clipped into the figure column.
    pub big_note: Option<String>,
    /// The quiet word after the big figure.
    pub big_label: &'static str,
    /// Fraction of the window used, when known.
    pub used: Option<f32>,
    pub headroom: String,
    /// Provider threshold, used for headroom and profile meters.
    pub level: Level,
    /// Desktop reset sentence, empty when unknown.
    pub resets: String,
    /// Phone reset sentence.
    pub phone_resets: String,
    /// Cost line and optional weekly-cap line.
    pub costs: String,
    pub chart: Option<Chart>,
    /// Said in the chart's place when there is nothing to chart.
    pub chart_missing: Option<&'static str>,
    pub profiles: Vec<PhoneProfile>,
    pub quotas: Vec<QuotaView>,
}
/// One provider quota observation; unavailable facts are explicitly unknown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaView {
    pub profile: String,
    pub source: String,
    pub scope: String,
    pub window: String,
    pub used: String,
    pub limit: String,
    pub headroom: String,
    pub resets_at: String,
    pub observed_at: String,
    pub exhausted: String,
    pub completeness: Option<String>,
}

pub const QUOTA_TITLE: &str = "Quota";
pub const UNKNOWN: &str = "unknown";

impl QuotaView {
    pub fn facts(&self) -> [(&'static str, &str); 10] {
        [
            ("Profile", &self.profile),
            ("Source", &self.source),
            ("Scope", &self.scope),
            ("Window", &self.window),
            ("Used", &self.used),
            ("Limit", &self.limit),
            ("Headroom", &self.headroom),
            ("Resets", &self.resets_at),
            ("Observed", &self.observed_at),
            ("Exhausted", &self.exhausted),
        ]
    }
}

/// A profile line inside a phone provider block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhoneProfile {
    pub name: String,
    pub detail: String,
}
/// One profile table row, with repeated provider and headroom cells suppressed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileRow {
    /// Provider label on the first row only.
    pub provider: String,
    pub profile: String,
    pub window: String,
    /// Meter fill as a percentage; None when the share is unknown.
    pub share_percent: Option<u32>,
    /// Meter level follows provider use, not profile use.
    pub level: Level,
    pub today: String,
    pub today_note: Option<String>,
    /// Provider headroom on the first row only.
    pub headroom: String,
}
/// One stacked daily cost bar: segment widths in percent of the track, in legend order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CostBar {
    pub day: String,
    pub values: [u32; 3],
    pub total_label: String,
    pub total_note: Option<String>,
}
/// The cost-per-day section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Costs {
    pub title: String,
    pub legend: Vec<String>,
    pub bars: Vec<CostBar>,
    /// Said in place of the bars when there are none.
    pub empty: Option<String>,
    /// Coverage or retained-series status; independent of whether bars exist.
    pub note: Option<String>,
}
/// One session's recorded tokens and cost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRow {
    pub session: String,
    pub what: String,
    pub profile: String,
    pub tokens: String,
    pub cost: String,
}
/// The whole page.
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub providers: Vec<ProviderView>,
    /// Said when no provider has recorded usage.
    pub providers_empty: Option<&'static str>,
    pub profile_headers: [&'static str; 6],
    pub profiles: Vec<ProfileRow>,
    pub costs: Costs,
    /// Live only: per-session tokens and cost.
    pub sessions: Option<Vec<SessionRow>>,
    pub sessions_note: Option<String>,
    pub disclaimer: &'static str,
    pub phone_disclaimer: &'static str,
    /// Deduplicated live read errors; render once above the content.
    pub notices: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::{Level, level};

    #[test]
    fn usage_thresholds_do_not_shift_at_band_edges() {
        assert_eq!(level(0.69), Level::None);
        assert_eq!(level(0.70), Level::Warn);
        assert_eq!(level(0.90), Level::Bad);
    }
}
