//! Timeline plots and canvas-ready geometry. A [`Plot`] is either the fixture preview's
//! illustrative day (`data`) or the daemon's recorded spans (`live`); both are laid out by the
//! same geometry. Plot times are whole minutes on the plot's own clock; horizontal coordinates
//! include the label column.

mod data;
mod live;

pub use data::GAPS_DISCLOSURE;
pub use live::{
    LIVE_GAPS_NOTE, Window, burnup_request, burnup_series, empty_gaps_line, gaps as live_gaps,
    plot as live_plot, request, window,
};

use crate::model::effect::{Effect, Env};
use crate::model::hue::Hue;

/// Selected time range in the page switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Range {
    /// Today, from local midnight to now.
    #[default]
    Today,
    /// The last seven local calendar days, including today.
    SevenDays,
}

/// Selected desktop grouping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Grouping {
    /// Group by task.
    #[default]
    ByTask,
    /// Group by agent session.
    ByAgent,
}

/// The timeline range selection.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct State {
    /// Active range switch option.
    pub range: Range,
    /// Active desktop grouping option.
    pub grouping: Grouping,
}
impl State {
    /// Select a range without navigating away.
    pub fn set_range(&mut self, range: Range) -> Vec<Effect> {
        self.range = range;
        Vec::new()
    }
    /// Select the desktop grouping.
    pub fn set_grouping(&mut self, grouping: Grouping) -> Vec<Effect> {
        self.grouping = grouping;
        Vec::new()
    }
}

/// A timeline segment's semantic state, including the hatched variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Worker running.
    Run,
    /// Corral gate.
    Gate,
    /// Review.
    Review,
    /// Landed.
    Land,
    /// Idle hatch.
    Idle,
    /// Stalled hatch (illustrative preview only).
    Stall,
    /// Waiting-on-you hatch.
    Ask,
    /// Stale-verdict hatch (illustrative preview only).
    Stale,
    /// Waiting for the owner's acceptance.
    Held,
    /// A phase whose times corral did not record.
    Unavailable,
}
impl Kind {
    /// Name used in the segment's accessible label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Gate => "gate",
            Self::Review => "review",
            Self::Land => "land",
            Self::Idle => "idle",
            Self::Stall => "stall",
            Self::Ask => "ask",
            Self::Stale => "stale",
            Self::Held => "awaiting acceptance",
            Self::Unavailable => "not recorded",
        }
    }
    /// State colour; the kind also tells the renderer which hatch or mixed fill to use.
    pub fn hue(self) -> Hue {
        match self {
            Self::Run => Hue::Worker,
            Self::Gate | Self::Review => Hue::Corral,
            Self::Land => Hue::Ok,
            Self::Idle | Self::Unavailable => Hue::Neutral,
            Self::Stall => Hue::Fail,
            Self::Ask => Hue::You,
            Self::Stale | Self::Held => Hue::Held,
        }
    }
}

/// An hour or day tick on the plot's clock; phone leaves minor ticks unlabelled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlotTick {
    pub at: i64,
    pub label: String,
    pub minor: bool,
}
/// An instant with its words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marker {
    pub at: i64,
    pub label: String,
}
/// One placed span of a lane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlotSegment {
    pub kind: Kind,
    pub start: i64,
    pub end: i64,
    /// Hover and screen-reader description.
    pub label: String,
    /// Visible desktop annotation above the bar.
    pub note: Option<String>,
    /// Visible phone annotation under the bar.
    pub phone_note: Option<String>,
}
/// One lane: its placed spans, its question strip, and what it could not place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlotLane {
    pub id: String,
    pub title: String,
    pub segments: Vec<PlotSegment>,
    pub ask: Option<PlotSegment>,
    /// Phases corral recorded without times, said instead of drawn.
    pub missing: Option<String>,
}
/// A supervisor interval or instant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlotPm {
    pub start: i64,
    pub end: i64,
    pub hue: Hue,
    pub label: Option<String>,
    pub accessible_label: String,
}
/// Everything the chart draws, before geometry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plot {
    pub start: i64,
    pub end: i64,
    pub ticks: Vec<PlotTick>,
    pub owner: Vec<Marker>,
    pub pm: Vec<PlotPm>,
    pub lanes: Vec<PlotLane>,
    pub last_visit: Option<Marker>,
    pub now: Marker,
    pub accessible_label: String,
}

/// The illustrative day of the fixture preview.
pub fn fixture_plot() -> Plot {
    data::plot()
}

/// An axis tick with canvas x and optional label (minor ticks are blank on phone).
#[derive(Debug, Clone, PartialEq)]
pub struct Tick {
    pub x: f32,
    pub label: String,
}
/// A vertical reference line.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub x: f32,
    pub label: String,
}
/// A positioned task segment.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedSegment {
    pub x: f32,
    /// Width, including the mockup's one-unit gap and minimum width.
    pub width: f32,
    pub kind: Kind,
    pub hue: Hue,
    pub accessible_label: String,
    /// Additional visible annotation, if any.
    pub annotation: Option<String>,
    /// Whether this is the thin overlay rather than a normal segment.
    pub overlay: bool,
}
/// A positioned lane.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedLane {
    pub id: String,
    pub title: String,
    pub y: f32,
    pub segments: Vec<PlacedSegment>,
    /// Phases without recorded times, written in the track.
    pub missing: Option<String>,
}
/// An owner action dot with its accessibility text.
#[derive(Debug, Clone, PartialEq)]
pub struct OwnerMarker {
    pub x: f32,
    pub accessible_label: String,
}
/// A positioned supervisor interval.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedPm {
    pub x: f32,
    pub width: f32,
    pub hue: Hue,
    pub label: Option<String>,
    pub accessible_label: String,
}
/// Canvas-ready timeline, including owner and supervisor lanes.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub width: f32,
    pub height: f32,
    pub label_width: f32,
    pub plot_width: f32,
    pub lane_height: f32,
    pub top: f32,
    pub ticks: Vec<Tick>,
    pub owner_label: (&'static str, &'static str),
    pub owner: Vec<OwnerMarker>,
    pub pm_label: (&'static str, &'static str),
    pub pm: Vec<PlacedPm>,
    /// Lanes after the six-unit supervisor gap.
    pub lanes: Vec<PlacedLane>,
    pub last_visit: Option<Line>,
    pub now: Line,
    pub accessible_label: String,
}
/// Lay out a plot at the given total canvas width. Phone suppresses minor tick labels and moves
/// labels above the tracks.
pub fn layout(plot: &Plot, width: f32, phone: bool) -> Layout {
    let label_width = if phone { 0.0 } else { 190.0 };
    let lane_height = if phone { 46.0 } else { 34.0 };
    let top = 26.0;
    let plot_width = width - label_width - 10.0;
    let span = (plot.end - plot.start).max(1) as f32;
    let x = |m: i64| label_width + (m - plot.start) as f32 / span * plot_width;
    let ticks = plot
        .ticks
        .iter()
        .map(|tick| Tick {
            x: x(tick.at),
            label: if phone && tick.minor {
                String::new()
            } else {
                tick.label.clone()
            },
        })
        .collect();
    let owner = plot
        .owner
        .iter()
        .map(|a| OwnerMarker {
            x: x(a.at),
            accessible_label: a.label.clone(),
        })
        .collect();
    let pm = plot
        .pm
        .iter()
        .map(|p| PlacedPm {
            x: x(p.start),
            width: (x(p.end) - x(p.start)).max(2.0),
            hue: p.hue,
            label: p.label.clone(),
            accessible_label: p.accessible_label.clone(),
        })
        .collect();
    let lanes = plot
        .lanes
        .iter()
        .enumerate()
        .map(|(i, lane)| {
            let place = |s: &PlotSegment, overlay: bool| {
                let x1 = x(s.start);
                let raw_width = x(s.end) - x1;
                PlacedSegment {
                    x: x1,
                    width: if overlay {
                        raw_width
                    } else {
                        (raw_width - 1.0).max(2.0)
                    },
                    kind: s.kind,
                    hue: s.kind.hue(),
                    accessible_label: s.label.clone(),
                    annotation: if phone {
                        s.phone_note.clone()
                    } else {
                        s.note.clone()
                    },
                    overlay,
                }
            };
            let mut segments: Vec<_> = lane.segments.iter().map(|s| place(s, false)).collect();
            if let Some(ask) = &lane.ask {
                segments.push(place(ask, true));
            }
            PlacedLane {
                id: lane.id.clone(),
                title: lane.title.clone(),
                y: top + 2.0 * lane_height + 6.0 + i as f32 * lane_height,
                segments,
                missing: lane.missing.clone(),
            }
        })
        .collect();
    Layout {
        width,
        height: top + (2 + plot.lanes.len()) as f32 * lane_height + 6.0 + 18.0,
        label_width,
        plot_width,
        lane_height,
        top,
        ticks,
        owner_label: ("You", "owner actions"),
        owner,
        pm_label: ("PM", "supervisor"),
        pm,
        lanes,
        last_visit: plot.last_visit.as_ref().map(|visit| Line {
            x: x(visit.at),
            label: visit.label.clone(),
        }),
        now: Line {
            x: x(plot.now.at),
            label: plot.now.label.clone(),
        },
        accessible_label: plot.accessible_label.clone(),
    }
}

/// One page range option.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RangeOption {
    pub label: &'static str,
    pub range: Range,
    pub selected: bool,
}
/// One desktop grouping option.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroupOption {
    pub label: &'static str,
    pub grouping: Grouping,
    pub selected: bool,
}
/// Page heading and controls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    pub title: &'static str,
    pub subtitle: &'static str,
    pub ranges: [RangeOption; 2],
    /// Desktop grouping controls, absent on phone.
    pub grouping: Option<[GroupOption; 2]>,
}
/// Build the page heading and its selected options.
pub fn head(state: &State, _env: Env, phone: bool) -> Head {
    Head {
        title: "Timeline",
        subtitle: "who worked when · run, gate, review, land · idle and stalls",
        ranges: [
            RangeOption {
                label: "Today",
                range: Range::Today,
                selected: state.range == Range::Today,
            },
            RangeOption {
                label: "7 days",
                range: Range::SevenDays,
                selected: state.range == Range::SevenDays,
            },
        ],
        grouping: (!phone).then_some([
            GroupOption {
                label: "By task",
                grouping: Grouping::ByTask,
                selected: state.grouping == Grouping::ByTask,
            },
            GroupOption {
                label: "By agent",
                grouping: Grouping::ByAgent,
                selected: state.grouping == Grouping::ByAgent,
            },
        ]),
    }
}
/// The live page's subtitle: corral records no stalls or stale verdicts as spans.
pub const LIVE_SUBTITLE: &str = "who worked when · run, gate, review, land · waits and idle";
/// A legend entry, with kind identifying hatch and mixed-fill variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LegendItem {
    pub label: &'static str,
    pub kind: Kind,
    pub hue: Hue,
}
/// Timeline legend: the mockup's order in the preview; recorded kinds only when live.
pub fn legend(live: bool) -> Vec<LegendItem> {
    let kinds: &[(&str, Kind)] = if live {
        &[
            ("run", Kind::Run),
            ("gate", Kind::Gate),
            ("review", Kind::Review),
            ("land", Kind::Land),
            ("waiting on you", Kind::Ask),
            ("awaiting acceptance", Kind::Held),
            ("idle", Kind::Idle),
            ("times not recorded", Kind::Unavailable),
        ]
    } else {
        &[
            ("run", Kind::Run),
            ("gate", Kind::Gate),
            ("review", Kind::Review),
            ("land", Kind::Land),
            ("waiting on you", Kind::Ask),
            ("idle", Kind::Idle),
            ("stale verdict", Kind::Stale),
            ("stall", Kind::Stall),
        ]
    };
    kinds
        .iter()
        .map(|&(label, kind)| LegendItem {
            label,
            kind,
            hue: kind.hue(),
        })
        .collect()
}
/// A noteworthy interval beneath the chart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gap {
    pub time: String,
    pub task: String,
    pub duration: String,
    pub kind: Kind,
    pub wording: String,
}
/// Gap section heading for a range.
pub fn gaps_title(range: Range) -> &'static str {
    match range {
        Range::Today => "Gaps worth a look today",
        Range::SevenDays => "Gaps worth a look · 7 days",
    }
}
/// The illustrative gaps of the fixture preview.
pub fn fixture_gaps() -> Vec<Gap> {
    data::gaps()
}

/// Burn-up input: points at fractions of the period with their scope and landed counts.
#[derive(Debug, Clone, PartialEq)]
pub struct BurnupSeries {
    pub title: String,
    pub period: String,
    /// `(fraction of the period, scope, landed)` in time order.
    pub points: Vec<(f32, u32, u32)>,
    pub first: String,
    pub last: String,
    pub disclosure: Option<&'static str>,
}
/// The illustrative burn-up of the fixture preview.
pub fn fixture_burnup() -> BurnupSeries {
    data::burnup()
}
/// A plotted chart point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
    pub value: u32,
}
/// Burn-up chart data and canvas coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct Burnup {
    pub title: String,
    pub period: String,
    pub accessible_label: String,
    pub disclosure: Option<&'static str>,
    pub size: (f32, f32),
    pub scope: Vec<Point>,
    pub landed: Vec<Point>,
    /// Horizontal grid values and y coordinates.
    pub grid: Vec<(u32, f32)>,
    pub scope_total: u32,
    pub landed_total: u32,
    /// Labels at the ends of the series.
    pub series_labels: [(String, f32, f32); 2],
    /// Start and end axis labels and their positions.
    pub axis_labels: [(String, f32, f32); 2],
}
/// Baseline distance between the burn-up's two end labels.
const LABEL_LINE: f32 = 12.0;
/// The smallest round grid step that puts the top of the series under three steps.
fn grid_step(max: u32) -> u32 {
    let need = max.div_ceil(3).max(1);
    let mut magnitude = 1;
    loop {
        for factor in [1, 2, 5] {
            if factor * magnitude >= need {
                return factor * magnitude;
            }
        }
        magnitude *= 10;
    }
}
/// Build chart points for the requested canvas dimensions (desktop 404×150, phone 354×140).
pub fn burnup(series: &BurnupSeries, width: f32, height: f32) -> Burnup {
    let step = grid_step(
        series
            .points
            .iter()
            .map(|p| p.1.max(p.2))
            .max()
            .unwrap_or(0),
    );
    let top = step * 3;
    let x = |fraction: f32| 26.0 + fraction.clamp(0.0, 1.0) * (width - 32.0);
    let y = |v: u32| 8.0 + (1.0 - v as f32 / top as f32) * (height - 30.0);
    let scope: Vec<Point> = series
        .points
        .iter()
        .map(|&(at, value, _)| Point {
            x: x(at),
            y: y(value),
            value,
        })
        .collect();
    let landed: Vec<Point> = series
        .points
        .iter()
        .map(|&(at, _, value)| Point {
            x: x(at),
            y: y(value),
            value,
        })
        .collect();
    let (scope_total, landed_total) = series.points.last().map_or((0, 0), |p| (p.1, p.2));
    let end = x(series.points.last().map_or(1.0, |p| p.0));
    // The end labels stay above the zero line (the day labels sit under it), inside the top
    // edge, and a line apart when the series end close together.
    let landed_y = (y(landed_total) + 14.0).min(y(0) - 4.0);
    let scope_y = (y(scope_total) - 5.0).min(landed_y - LABEL_LINE).max(9.0);
    let landed_y = landed_y.max(scope_y + LABEL_LINE);
    Burnup {
        title: series.title.clone(),
        period: series.period.clone(),
        accessible_label: format!("Burn-up: {landed_total} of {scope_total} landed"),
        disclosure: series.disclosure,
        size: (width, height),
        scope,
        landed,
        grid: (0..=3).map(|i| (i * step, y(i * step))).collect(),
        scope_total,
        landed_total,
        series_labels: [
            (format!("scope {scope_total}"), end, scope_y),
            (format!("landed {landed_total}"), end, landed_y),
        ],
        axis_labels: [
            (series.first.clone(), 26.0, height - 4.0),
            (series.last.clone(), width - 6.0, height - 4.0),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn segment_coordinates_use_plot_width_not_canvas_width() {
        let view = layout(&fixture_plot(), 1190.0, false);
        let segment = &view.lanes[0].segments[0];
        let scale = (1190.0 - 190.0 - 10.0) / (data::END - data::START) as f32;
        assert!((segment.x - (190.0 + 43.0 * scale)).abs() < 0.001);
        assert!((segment.width - (147.0 * scale - 1.0)).abs() < 0.001);
    }
    #[test]
    fn burnup_end_labels_clear_the_day_labels_and_each_other() {
        let series = BurnupSeries {
            title: String::new(),
            period: String::new(),
            points: vec![(0.0, 9, 0), (1.0, 9, 0)],
            first: "11-14".into(),
            last: "today".into(),
            disclosure: None,
        };
        let chart = burnup(&series, 404.0, 150.0);
        let [scope, landed] = &chart.series_labels;
        let day = chart.axis_labels[1].2;
        assert!(
            landed.2 <= day - LABEL_LINE,
            "landed label over the day label"
        );
        assert!(landed.2 - scope.2 >= LABEL_LINE, "end labels overlap");
        assert!(scope.2 >= 9.0, "scope label above the canvas");
    }
}
