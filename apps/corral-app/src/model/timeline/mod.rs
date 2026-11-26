//! Recorded timeline plots and canvas-ready geometry. Plot times are whole minutes on the
//! plot's own clock; horizontal coordinates include the label column.

mod inspect;
mod live;
mod plans;
mod scope;
mod words;

pub use inspect::{Inspector, Link, inspect};
pub use live::{
    LIVE_GAPS_NOTE, Window, burnup_series, empty_gaps_line, gaps as live_gaps, plot as live_plot,
    request, window,
};
pub use plans::{Edge, Landing, PlanGroup};
pub use scope::{BurnupScope, ScopeOption, burnup_request, scope_options};
pub use words::{header_spoken, lane_verb, lane_words};

pub const BURNUP_SCOPE_LABEL: &str = "Burn-up scope";

use std::collections::BTreeSet;

use crate::model::Live;
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

/// Selected grouping, available on every form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Grouping {
    /// One lane per task.
    #[default]
    ByTask,
    /// One lane per run.
    ByAgent,
}

/// The timeline range selection.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct State {
    /// Active range switch option.
    pub range: Range,
    /// Active grouping option.
    pub grouping: Grouping,
    pub burnup_scope: BurnupScope,
    /// The lane whose facts the inspector shows.
    pub selected: Option<String>,
    /// The plan groups folded to their header (a plan's id, or [`UNPLANNED`]).
    pub folded: BTreeSet<String>,
}
impl State {
    /// Select a range without navigating away. The selection goes with the range: a lane of the
    /// other range is another stretch of work, so it needs a press of its own.
    pub fn set_range(&mut self, range: Range) -> Vec<Effect> {
        if self.range != range {
            self.selected = None;
        }
        self.range = range;
        Vec::new()
    }
    /// Whether the selection names a lane `plot` no longer has; the page then deselects, so a
    /// lane that comes back later needs a press to select it again and its edges are not drawn
    /// meanwhile.
    pub fn selection_gone(&self, plot: &Plot) -> bool {
        self.selected
            .as_ref()
            .is_some_and(|lane| !plot.lanes.iter().any(|l| l.id == *lane))
    }
    /// Select the grouping on any form. Lane ids differ between the groupings, so the selection
    /// goes with the lanes it named.
    pub fn set_grouping(&mut self, grouping: Grouping) -> Vec<Effect> {
        if self.grouping != grouping {
            self.selected = None;
        }
        self.grouping = grouping;
        Vec::new()
    }
    /// A press on a lane: the first selects it, a press on the selected lane opens what it
    /// opens.
    pub fn press(&mut self, lane: &str, opens: Live) -> Vec<Effect> {
        if self.selected.as_deref() == Some(lane) {
            return self.open(opens);
        }
        self.selected = Some(lane.to_owned());
        Vec::new()
    }
    /// Leave the inspector.
    pub fn deselect(&mut self) -> Vec<Effect> {
        self.selected = None;
        Vec::new()
    }
    /// Fold or unfold a plan group.
    pub fn fold(&mut self, key: &str) -> Vec<Effect> {
        if !self.folded.remove(key) {
            self.folded.insert(key.to_owned());
        }
        Vec::new()
    }
    pub fn set_burnup_scope(&mut self, scope: BurnupScope) -> Vec<Effect> {
        self.burnup_scope = scope;
        Vec::new()
    }
    /// Open a lane's task or its run's session page.
    pub fn open(&mut self, opens: Live) -> Vec<Effect> {
        vec![Effect::OpenLive(opens)]
    }
}

/// A timeline segment's semantic state, including the hatched variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A worker run.
    Run,
    /// A gate result, an instant.
    Gate,
    /// A reviewer run.
    Review,
    /// Landed, an instant.
    Land,
    /// Dropped, an instant.
    Drop,
    /// Reopened after Done or Dropped, an instant.
    Reopen,
    /// An open run whose output has gone quiet.
    Idle,
    /// Blocked, waiting for the owner's next move.
    Ask,
    /// Ready, waiting for the owner's acceptance.
    Held,
}
impl Kind {
    /// Name used in the segment's accessible label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Gate => "gate",
            Self::Review => "review",
            Self::Land => "land",
            Self::Drop => "dropped",
            Self::Reopen => "reopened",
            Self::Idle => "idle",
            Self::Ask => "blocked",
            Self::Held => "awaiting acceptance",
        }
    }
    /// State colour; the kind also tells the renderer which hatch, mixed fill or mark to use.
    pub fn hue(self) -> Hue {
        match self {
            Self::Run | Self::Reopen => Hue::Worker,
            Self::Gate | Self::Review => Hue::Corral,
            Self::Land => Hue::Ok,
            Self::Idle | Self::Drop => Hue::Neutral,
            Self::Ask => Hue::You,
            Self::Held => Hue::Held,
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
/// One placed span of a lane; an instant has `start == end`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlotSegment {
    pub kind: Kind,
    pub start: i64,
    pub end: i64,
    /// Hover and screen-reader description.
    pub label: String,
}
/// What pressing a lane opens: its task, or a live run's session terminal; and the words that
/// name the press.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opens {
    pub live: Live,
    pub label: String,
}
/// One lane: a task's runs, waits and instants, or one run's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlotLane {
    pub id: String,
    pub title: String,
    pub segments: Vec<PlotSegment>,
    /// Wait summary anchored to this row rather than floating between bars.
    pub wait: Option<String>,
    /// Said when the task started before a dependency landed: `started before T-852 landed`.
    pub warning: Option<String>,
    pub opens: Opens,
}
/// Everything the chart draws, before geometry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plot {
    pub start: i64,
    pub end: i64,
    pub ticks: Vec<PlotTick>,
    /// The owner's recorded moves.
    pub owner: Vec<Marker>,
    /// The pipeline's and assistants' recorded moves.
    pub auto: Vec<Marker>,
    pub lanes: Vec<PlotLane>,
    /// The lanes' plans, in drawing order; none for a flat list.
    pub groups: Vec<PlanGroup>,
    /// Every dependency edge between the lanes' tasks, drawn or not.
    pub edges: Vec<Edge>,
    pub now: Marker,
    pub accessible_label: String,
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
    pub accessible_label: String,
}
/// A positioned lane.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedLane {
    pub id: String,
    pub title: String,
    pub y: f32,
    pub segments: Vec<PlacedSegment>,
    pub wait: Option<String>,
    pub warning: Option<String>,
    /// The lane the inspector shows.
    pub selected: bool,
    pub opens: Opens,
}
/// A plan group's header row: its words, and its summary bar from its lanes' earliest to
/// latest activity with the landed share filled.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedHeader {
    pub key: String,
    pub id: Option<String>,
    pub title: String,
    pub tally: String,
    pub folded: bool,
    pub y: f32,
    /// The bar's start and end x.
    pub bar: Option<(f32, f32)>,
    pub share: f32,
    pub spoken: String,
}
/// A drawn dependency edge, from the dependency's landing to the dependent's first run: each
/// end's x and the top of the lane it sits on.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedEdge {
    pub from: (f32, f32),
    pub to: (f32, f32),
    pub out_of_order: bool,
    pub label: String,
}
/// What the owner chose about the lanes: the one selected and the plans folded.
#[derive(Debug, Clone, Copy, Default)]
pub struct LaneView<'a> {
    pub selected: Option<&'a str>,
    pub folded: Option<&'a BTreeSet<String>>,
}
impl LaneView<'_> {
    fn folded(&self, key: &str) -> bool {
        self.folded.is_some_and(|folded| folded.contains(key))
    }
}
/// An action mark with its accessibility text.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedMarker {
    pub x: f32,
    pub accessible_label: String,
}
/// Canvas-ready timeline, including the owner and automatic lanes.
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
    pub owner: Vec<PlacedMarker>,
    pub auto_label: (&'static str, &'static str),
    pub auto: Vec<PlacedMarker>,
    /// Lanes after the six-unit gap under the action lanes; a folded plan's are left out.
    pub lanes: Vec<PlacedLane>,
    /// The plan headers, each above its lanes; none for a flat list.
    pub headers: Vec<PlacedHeader>,
    pub header_height: f32,
    /// The edges drawn: out-of-order ones and the selected lane's.
    pub edges: Vec<PlacedEdge>,
    pub now: Line,
    pub accessible_label: String,
}
/// Lay out a plot at the given total canvas width. Phone suppresses minor tick labels and moves
/// labels above the tracks.
pub fn layout(plot: &Plot, width: f32, phone: bool, view: LaneView) -> Layout {
    let label_width = if phone { 0.0 } else { 190.0 };
    let lane_height = if phone { 46.0 } else { 34.0 };
    let header_height = if phone { 34.0 } else { 30.0 };
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
    let place_markers = |markers: &[Marker]| {
        markers
            .iter()
            .map(|a| PlacedMarker {
                x: x(a.at),
                accessible_label: a.label.clone(),
            })
            .collect()
    };
    let place = |lane: &PlotLane, y: f32| PlacedLane {
        id: lane.id.clone(),
        title: lane.title.clone(),
        y,
        segments: lane
            .segments
            .iter()
            .map(|s| {
                let x1 = x(s.start);
                PlacedSegment {
                    x: x1,
                    width: (x(s.end) - x1 - 1.0).max(2.0),
                    kind: s.kind,
                    accessible_label: s.label.clone(),
                }
            })
            .collect(),
        wait: lane.wait.clone(),
        warning: lane.warning.clone(),
        selected: view.selected == Some(lane.id.as_str()),
        opens: lane.opens.clone(),
    };
    let mut y = top + 2.0 * lane_height + 6.0;
    let mut lanes = Vec::with_capacity(plot.lanes.len());
    let mut headers = Vec::with_capacity(plot.groups.len());
    if plot.groups.is_empty() {
        for lane in &plot.lanes {
            lanes.push(place(lane, y));
            y += lane_height;
        }
    }
    for group in &plot.groups {
        let folded = view.folded(&group.key);
        headers.push(PlacedHeader {
            key: group.key.clone(),
            id: group.id.clone(),
            title: group.title.clone(),
            tally: group.tally.clone(),
            folded,
            y,
            bar: group
                .span
                .map(|(from, to)| (x(from), x(to).max(x(from) + 2.0))),
            share: group.share(),
            spoken: group.spoken(),
        });
        y += header_height;
        if folded {
            continue;
        }
        for id in &group.lanes {
            if let Some(lane) = plot.lanes.iter().find(|lane| lane.id == *id) {
                lanes.push(place(lane, y));
                y += lane_height;
            }
        }
    }
    let lane_y = |id: &str| lanes.iter().find(|lane| lane.id == id).map(|lane| lane.y);
    let edges = plot
        .edges
        .iter()
        .filter(|edge| plans::edge_shown(edge, view.selected))
        .filter_map(|edge| {
            let to = lane_y(&edge.to)?;
            // A dependency without a lane shown here (none in the window, or folded away)
            // starts on its dependent's row.
            let from = lane_y(&edge.from).unwrap_or(to);
            Some(PlacedEdge {
                from: (x(edge.from_at), from),
                to: (x(edge.to_at), to),
                out_of_order: edge.out_of_order,
                label: edge.label.clone(),
            })
        })
        .collect();
    Layout {
        width,
        height: y + 18.0,
        label_width,
        plot_width,
        lane_height,
        top,
        ticks,
        owner_label: ("You", "owner moves"),
        owner: place_markers(&plot.owner),
        auto_label: ("Auto", "pipeline, assistants"),
        auto: place_markers(&plot.auto),
        lanes,
        headers,
        header_height,
        edges,
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
    pub ranges: [RangeOption; 2],
    /// Grouping controls on desktop, terminal and phone.
    pub grouping: Option<[GroupOption; 2]>,
}
/// Build the page heading and its selected options.
pub fn head(state: &State, _env: Env, _phone: bool) -> Head {
    Head {
        title: "Timeline",
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
        grouping: Some([
            GroupOption {
                label: "By task",
                grouping: Grouping::ByTask,
                selected: state.grouping == Grouping::ByTask,
            },
            GroupOption {
                label: "By run",
                grouping: Grouping::ByAgent,
                selected: state.grouping == Grouping::ByAgent,
            },
        ]),
    }
}
/// The page's subtitle: what the recorded runs and events show.
pub const LIVE_SUBTITLE: &str = "who worked when · runs, gates, reviews, landings · waits and idle";
/// A legend entry, with kind identifying hatch and mixed-fill variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LegendItem {
    pub label: &'static str,
    pub kind: Kind,
    pub hue: Hue,
}
/// Timeline legend, in the order a task passes through the kinds.
pub fn legend() -> Vec<LegendItem> {
    let kinds: &[(&str, Kind)] = &[
        ("run", Kind::Run),
        ("gate", Kind::Gate),
        ("review", Kind::Review),
        ("land", Kind::Land),
        ("dropped", Kind::Drop),
        ("reopened", Kind::Reopen),
        ("blocked", Kind::Ask),
        ("awaiting acceptance", Kind::Held),
        ("idle", Kind::Idle),
    ];
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
/// What the burn-up counts, said under it.
pub const BURNUP_COUNTS: &str = "Scope counts the tasks created in this scope, less those \
                                 dropped; landed counts the tasks landed, less those reopened. \
                                 Each step is a recorded event.";
/// Burn-up input: points at fractions of the period with their scope and landed counts.
#[derive(Debug, Clone, PartialEq)]
pub struct BurnupSeries {
    pub title: String,
    pub period: String,
    /// `(fraction of the period, scope, landed)` in time order.
    pub points: Vec<(f32, u32, u32)>,
    pub first: String,
    pub last: String,
    /// What the two lines count.
    pub counts: &'static str,
    /// The burn-up's legend: how many tasks were created, landed, dropped and reopened.
    pub tally: Vec<TallyEntry>,
}
/// Which of the burn-up's two lines a legend entry names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BurnupLine {
    Scope,
    Landed,
}
/// One entry of the burn-up's legend: `created 31`, with the line it names, if it draws one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TallyEntry {
    pub words: String,
    pub line: Option<BurnupLine>,
}
/// Said at the Timeline burn-up's foot: the range switch above narrows the lanes only.
pub const BURNUP_RANGE_NOTE: &str = "counts events; it does not narrow with the range above";
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
/// Build chart points for the requested canvas dimensions (desktop 404×150, phone 354×140).
pub fn burnup(series: &BurnupSeries, width: f32, height: f32) -> Burnup {
    let most = series
        .points
        .iter()
        .map(|p| p.1.max(p.2))
        .max()
        .unwrap_or(0);
    let step = u32::try_from(crate::model::chart::step(u64::from(most), 3)).unwrap_or(u32::MAX);
    let top = step.saturating_mul(3);
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
        accessible_label: format!("Burn-up: {landed_total} of {scope_total} tasks landed"),
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
    fn burnup_end_labels_clear_the_day_labels_and_each_other() {
        let series = BurnupSeries {
            title: String::new(),
            period: String::new(),
            points: vec![(0.0, 9, 0), (1.0, 9, 0)],
            first: "11-14".into(),
            last: "today".into(),
            counts: BURNUP_COUNTS,
            tally: Vec::new(),
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
