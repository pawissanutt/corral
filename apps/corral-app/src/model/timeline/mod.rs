//! Timeline fixtures and canvas-ready geometry. Times are minutes after midnight; horizontal
//! coordinates include the label column. The seven-day control changes selection only because the
//! mockup supplies no seven-day dataset.

use crate::model::effect::{Effect, Env};
use crate::model::hue::Hue;

/// Selected time range in the page switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Range {
    /// Today's snapshot.
    #[default]
    Today,
    /// Seven-day selection, using the mockup's same snapshot.
    SevenDays,
}

/// Selected desktop grouping; the fixture remains task-based.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Grouping {
    /// Group by task.
    #[default]
    ByTask,
    /// Group by agent.
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
    /// Stalled hatch.
    Stall,
    /// Waiting-on-you hatch.
    Ask,
    /// Stale-verdict hatch.
    Stale,
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
        }
    }
    /// State colour; the kind also tells the renderer which hatch or mixed fill to use.
    pub fn hue(self) -> Hue {
        match self {
            Self::Run => Hue::Worker,
            Self::Gate | Self::Review => Hue::Corral,
            Self::Land => Hue::Ok,
            Self::Idle => Hue::Neutral,
            Self::Stall => Hue::Fail,
            Self::Ask => Hue::You,
            Self::Stale => Hue::Held,
        }
    }
}

/// One fixture segment, with its optional explanatory note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    /// Segment state.
    pub kind: Kind,
    /// Start time in HH:MM.
    pub start: &'static str,
    /// End time in HH:MM.
    pub end: &'static str,
    /// Additional accessible explanation.
    pub note: Option<&'static str>,
}
const fn seg(
    kind: Kind,
    start: &'static str,
    end: &'static str,
    note: Option<&'static str>,
) -> Segment {
    Segment {
        kind,
        start,
        end,
        note,
    }
}

/// A task's timeline fixture; an ask overlay is drawn above its normal segments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lane {
    /// Task id.
    pub id: &'static str,
    /// Task title.
    pub title: &'static str,
    /// Normal segments in chronological order.
    pub segments: &'static [Segment],
    /// Optional question overlay.
    pub ask: Option<Segment>,
}
/// One owner action marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnerAction {
    /// Time in HH:MM.
    pub time: &'static str,
    /// Accessible description.
    pub description: &'static str,
}
/// One supervisor interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PmInterval {
    /// Start in HH:MM.
    pub start: &'static str,
    /// End in HH:MM.
    pub end: &'static str,
    /// Whether this interval is labelled relaunching.
    pub relaunching: bool,
}
/// Start of the timeline, minutes after midnight.
pub const START: u32 = 540;
/// End of the timeline, minutes after midnight.
pub const END: u32 = 1332;
/// Last visit, minutes after midnight.
pub const LAST_VISIT: u32 = 1120;
/// Task lanes in display order.
pub const LANES: &[Lane] = &[
    Lane {
        id: "T-852",
        title: "Co-work C1",
        segments: &[
            seg(Kind::Run, "09:43", "12:10", None),
            seg(Kind::Gate, "12:10", "12:52", None),
            seg(Kind::Review, "12:52", "13:40", None),
            seg(Kind::Run, "13:40", "16:05", None),
            seg(Kind::Gate, "16:05", "16:40", None),
            seg(
                Kind::Stall,
                "16:40",
                "19:28",
                Some("held · external op · 2h48 nobody moved"),
            ),
            seg(Kind::Gate, "19:28", "19:49", None),
            seg(Kind::Idle, "19:49", "22:03", None),
            seg(Kind::Stale, "22:03", "22:12", None),
        ],
        ask: None,
    },
    Lane {
        id: "T-854",
        title: "Co-work C3",
        segments: &[
            seg(Kind::Run, "09:00", "11:40", None),
            seg(Kind::Idle, "11:40", "13:05", None),
            seg(Kind::Run, "13:05", "22:12", None),
        ],
        ask: None,
    },
    Lane {
        id: "T-885",
        title: "plan P2",
        segments: &[
            seg(Kind::Review, "10:02", "10:33", None),
            seg(Kind::Run, "10:40", "11:58", None),
            seg(Kind::Review, "11:58", "12:10", None),
            seg(
                Kind::Ask,
                "12:10",
                "19:12",
                Some("awaiting your acceptance · 7h02"),
            ),
        ],
        ask: None,
    },
    Lane {
        id: "T-868",
        title: "host-queue-ledger",
        segments: &[
            seg(Kind::Run, "18:58", "21:40", None),
            seg(Kind::Idle, "21:40", "22:12", None),
        ],
        ask: Some(seg(
            Kind::Ask,
            "19:12",
            "22:12",
            Some("needs you · T-868/R1/Q1 · 3h"),
        )),
    },
    Lane {
        id: "T-890",
        title: "pm-bounded-read",
        segments: &[
            seg(Kind::Run, "19:31", "22:03", None),
            seg(Kind::Gate, "22:03", "22:12", None),
        ],
        ask: None,
    },
    Lane {
        id: "T-894",
        title: "api-connections",
        segments: &[
            seg(Kind::Run, "19:33", "22:06", None),
            seg(Kind::Gate, "22:06", "22:12", None),
        ],
        ask: None,
    },
    Lane {
        id: "T-867",
        title: "queue-contract-core",
        segments: &[
            seg(Kind::Run, "20:40", "21:52", None),
            seg(Kind::Gate, "21:52", "22:05", None),
            seg(Kind::Stale, "22:05", "22:12", None),
        ],
        ask: None,
    },
    Lane {
        id: "T-871",
        title: "annotation-service",
        segments: &[seg(Kind::Run, "21:17", "22:12", None)],
        ask: None,
    },
];
/// Owner actions shown as dots.
pub const OWNER: &[OwnerAction] = &[
    OwnerAction {
        time: "10:35",
        description: "commented on T-885 P1",
    },
    OwnerAction {
        time: "12:14",
        description: "read T-885 P2 review",
    },
    OwnerAction {
        time: "19:12",
        description: "accepted T-885 P2",
    },
    OwnerAction {
        time: "21:58",
        description: "asked in T-868 thread",
    },
];
/// Supervisor intervals, including the relaunch.
pub const PM: &[PmInterval] = &[
    PmInterval {
        start: "09:00",
        end: "14:20",
        relaunching: false,
    },
    PmInterval {
        start: "14:20",
        end: "15:05",
        relaunching: true,
    },
    PmInterval {
        start: "15:05",
        end: "22:12",
        relaunching: false,
    },
];

fn mins(time: &str) -> u32 {
    let b = time.as_bytes();
    u32::from(b[0] - b'0') * 600
        + u32::from(b[1] - b'0') * 60
        + u32::from(b[3] - b'0') * 10
        + u32::from(b[4] - b'0')
}
/// An axis tick with canvas x and optional label (odd hours are blank on phone).
#[derive(Debug, Clone, PartialEq)]
pub struct Tick {
    /// Horizontal canvas coordinate.
    pub x: f32,
    /// Tick text.
    pub label: String,
}
/// A vertical reference line.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    /// Horizontal canvas coordinate.
    pub x: f32,
    /// Line caption.
    pub label: &'static str,
}
/// A positioned task segment.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedSegment {
    /// Horizontal start.
    pub x: f32,
    /// Width, including the mockup's one-unit gap and minimum width.
    pub width: f32,
    /// Segment state and hatch selection.
    pub kind: Kind,
    /// State colour.
    pub hue: Hue,
    /// Hover or accessibility description.
    pub accessible_label: String,
    /// Additional visible annotation, if any.
    pub annotation: Option<&'static str>,
    /// Whether this is the thin overlay rather than a normal segment.
    pub overlay: bool,
}
/// A positioned task lane.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedLane {
    /// Task id.
    pub id: &'static str,
    /// Task title.
    pub title: &'static str,
    /// Top of the lane.
    pub y: f32,
    /// Normal and overlay segments.
    pub segments: Vec<PlacedSegment>,
}
/// An owner action dot with its accessibility text.
#[derive(Debug, Clone, PartialEq)]
pub struct OwnerMarker {
    /// Horizontal coordinate.
    pub x: f32,
    /// Time and action.
    pub accessible_label: String,
}
/// A positioned supervisor interval.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedPm {
    /// Horizontal start.
    pub x: f32,
    /// Interval width.
    pub width: f32,
    /// State colour.
    pub hue: Hue,
    /// Optional visible label.
    pub label: Option<&'static str>,
    /// Screen-reader description of the interval.
    pub accessible_label: String,
}
/// Canvas-ready timeline, including owner and supervisor lanes.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    /// Total canvas width.
    pub width: f32,
    /// Total canvas height.
    pub height: f32,
    /// Width reserved for desktop labels.
    pub label_width: f32,
    /// Width available to time coordinates.
    pub plot_width: f32,
    /// Height of each lane.
    pub lane_height: f32,
    /// First lane's y coordinate.
    pub top: f32,
    /// Hour ticks.
    pub ticks: Vec<Tick>,
    /// Owner lane title.
    pub owner_label: (&'static str, &'static str),
    /// Owner markers.
    pub owner: Vec<OwnerMarker>,
    /// Supervisor lane title.
    pub pm_label: (&'static str, &'static str),
    /// Supervisor intervals.
    pub pm: Vec<PlacedPm>,
    /// Task lanes after the six-unit supervisor gap.
    pub lanes: Vec<PlacedLane>,
    /// Last visit line.
    pub last_visit: Line,
    /// Current time line.
    pub now: Line,
    /// Canvas accessibility description.
    pub accessible_label: &'static str,
}
/// Lay out the illustrative snapshot at the given total canvas width. Phone suppresses odd
/// hour labels and moves labels above the tracks; range does not change the mockup's data.
pub fn layout(_state: &State, width: f32, phone: bool) -> Layout {
    let label_width = if phone { 0.0 } else { 190.0 };
    let lane_height = if phone { 46.0 } else { 34.0 };
    let top = 26.0;
    let plot_width = width - label_width - 10.0;
    let x = |m: u32| label_width + (m as f32 - START as f32) / (END - START) as f32 * plot_width;
    let ticks = (9..=22)
        .map(|h| Tick {
            x: x(h * 60),
            label: if phone && h % 2 != 0 {
                String::new()
            } else {
                format!("{h:02}")
            },
        })
        .collect();
    let owner = OWNER
        .iter()
        .map(|a| OwnerMarker {
            x: x(mins(a.time)),
            accessible_label: format!("{} {}", a.time, a.description),
        })
        .collect();
    let pm = PM
        .iter()
        .map(|p| PlacedPm {
            x: x(mins(p.start)),
            width: x(mins(p.end)) - x(mins(p.start)),
            hue: if p.relaunching {
                Hue::Held
            } else {
                Hue::Neutral
            },
            label: p.relaunching.then_some("relaunching"),
            accessible_label: format!(
                "PM {}–{}{}",
                p.start,
                p.end,
                if p.relaunching { " · relaunching" } else { "" }
            ),
        })
        .collect();
    let lanes = LANES
        .iter()
        .enumerate()
        .map(|(i, lane)| {
            let place = |s: &Segment, overlay: bool| {
                let x1 = x(mins(s.start));
                let raw_width = x(mins(s.end)) - x1;
                let accessible_label = if overlay {
                    s.note.unwrap_or("").to_string()
                } else {
                    format!(
                        "{} {} {}–{}{}",
                        lane.id,
                        s.kind.label(),
                        s.start,
                        s.end,
                        s.note.map(|n| format!(" · {n}")).unwrap_or_default()
                    )
                };
                let annotation = if overlay && !phone {
                    Some("needs you 3h")
                } else if !overlay && s.kind == Kind::Stall {
                    Some(if phone {
                        "stall 16:40–19:28 · 2h48"
                    } else {
                        "stall 16:40–19:28 · nobody moved"
                    })
                } else if !overlay && s.kind == Kind::Ask && !phone {
                    Some("waiting on you · 7h02")
                } else {
                    None
                };
                PlacedSegment {
                    x: x1,
                    width: if overlay {
                        raw_width
                    } else {
                        (raw_width - 1.0).max(2.0)
                    },
                    kind: s.kind,
                    hue: s.kind.hue(),
                    accessible_label,
                    annotation,
                    overlay,
                }
            };
            let mut segments: Vec<_> = lane.segments.iter().map(|s| place(s, false)).collect();
            if let Some(ask) = &lane.ask {
                segments.push(place(ask, true));
            }
            PlacedLane {
                id: lane.id,
                title: lane.title,
                y: top + 2.0 * lane_height + 6.0 + i as f32 * lane_height,
                segments,
            }
        })
        .collect();
    Layout {
        width,
        height: top + (2 + LANES.len()) as f32 * lane_height + 6.0 + 18.0,
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
        last_visit: Line {
            x: x(LAST_VISIT),
            label: "your last visit 18:40",
        },
        now: Line {
            x: x(END),
            label: "now",
        },
        accessible_label: "Run timeline 09:00 to 22:12",
    }
}

/// One page range option.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RangeOption {
    /// Option text.
    pub label: &'static str,
    /// Reducer argument.
    pub range: Range,
    /// Current selection.
    pub selected: bool,
}
/// One desktop grouping option.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroupOption {
    /// Option text.
    pub label: &'static str,
    /// Reducer argument.
    pub grouping: Grouping,
    /// Current selection.
    pub selected: bool,
}
/// Page heading and controls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    /// Page title.
    pub title: &'static str,
    /// Desktop subtitle.
    pub subtitle: &'static str,
    /// Range options.
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
/// A legend entry, with kind identifying hatch and mixed-fill variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LegendItem {
    /// Legend wording.
    pub label: &'static str,
    /// Segment representation.
    pub kind: Kind,
    /// State colour.
    pub hue: Hue,
}
/// Timeline legend in mockup order.
pub fn legend() -> Vec<LegendItem> {
    [
        ("run", Kind::Run),
        ("gate", Kind::Gate),
        ("review", Kind::Review),
        ("land", Kind::Land),
        ("waiting on you", Kind::Ask),
        ("idle", Kind::Idle),
        ("stale verdict", Kind::Stale),
        ("stall", Kind::Stall),
    ]
    .into_iter()
    .map(|(label, kind)| LegendItem {
        label,
        kind,
        hue: kind.hue(),
    })
    .collect()
}
/// A noteworthy interval beneath the chart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gap {
    /// Time span as displayed.
    pub time: &'static str,
    /// Task id.
    pub task: &'static str,
    /// Duration as displayed.
    pub duration: &'static str,
    /// Interval state.
    pub kind: Kind,
    /// Description.
    pub wording: &'static str,
}
/// Gap section heading.
pub const GAPS_TITLE: &str = "Gaps worth a look today";
/// Gap section's illustrative-data disclosure.
pub const GAPS_DISCLOSURE: &str = "Segment boundaries are illustrative, reconstructed around real start times and change events. Real records show 4 stalls of 4–8.6 h ended only by the owner asking (docs/records/pm-runtime-redesign.md).";
/// Noteworthy gaps in display order.
pub const GAPS: &[Gap] = &[
    Gap {
        time: "16:40–19:28",
        task: "T-852",
        duration: "2h48",
        kind: Kind::Stall,
        wording: "held · external op; no sync tick moved it for 2h48. Today the daemon would have re-checked on its own loop.",
    },
    Gap {
        time: "12:10–19:12",
        task: "T-885",
        duration: "7h02",
        kind: Kind::Ask,
        wording: "plan P2 waited 7h02 for your acceptance; a push would have reached the phone at 12:10.",
    },
    Gap {
        time: "19:12–now",
        task: "T-868",
        duration: "3h",
        kind: Kind::Ask,
        wording: "question open 3h; the worker kept producing until 21:40, then idled.",
    },
];
/// Scope values over fourteen days.
pub const SCOPE: [u32; 14] = [41, 41, 44, 44, 47, 47, 50, 53, 53, 53, 56, 56, 56, 57];
/// Landed values over fourteen days.
pub const LANDED: [u32; 14] = [18, 19, 19, 22, 24, 24, 27, 29, 31, 31, 33, 35, 36, 37];
/// A plotted chart point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    /// Horizontal coordinate.
    pub x: f32,
    /// Vertical coordinate.
    pub y: f32,
    /// Source count.
    pub value: u32,
}
/// Burn-up chart data and canvas coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct Burnup {
    /// Section heading.
    pub title: &'static str,
    /// Time period caption.
    pub period: &'static str,
    /// Screen-reader summary.
    pub accessible_label: &'static str,
    /// Illustrative data disclosure.
    pub disclosure: &'static str,
    /// Canvas dimensions.
    pub size: (f32, f32),
    /// Scope series.
    pub scope: Vec<Point>,
    /// Landed series.
    pub landed: Vec<Point>,
    /// Horizontal grid values and y coordinates.
    pub grid: Vec<(u32, f32)>,
    /// Last scope count.
    pub scope_total: u32,
    /// Last landed count.
    pub landed_total: u32,
    /// Labels at the ends of the series.
    pub series_labels: [(&'static str, f32, f32); 2],
    /// Start and end axis labels and their positions.
    pub axis_labels: [(&'static str, f32, f32); 2],
}
/// Build chart points for the requested canvas dimensions (desktop 404×150, phone 354×140).
pub fn burnup(width: f32, height: f32) -> Burnup {
    let x = |i: usize| 26.0 + i as f32 / 13.0 * (width - 32.0);
    let y = |v: u32| 8.0 + (1.0 - v as f32 / 60.0) * (height - 30.0);
    let series = |values: &[u32]| {
        values
            .iter()
            .enumerate()
            .map(|(i, &value)| Point {
                x: x(i),
                y: y(value),
                value,
            })
            .collect()
    };
    Burnup {
        title: "Goal burn-up · Co-work and PM goals",
        period: "14 days",
        accessible_label: "Burn-up: 37 of 57 landed",
        disclosure: "burn-up numbers illustrative",
        size: (width, height),
        scope: series(&SCOPE),
        landed: series(&LANDED),
        grid: [0, 20, 40, 60].into_iter().map(|v| (v, y(v))).collect(),
        scope_total: 57,
        landed_total: 37,
        series_labels: [
            ("scope 57", x(13), y(57) - 5.0),
            ("landed 37", x(13), y(37) + 14.0),
        ],
        axis_labels: [
            ("10-26", 26.0, height - 4.0),
            ("today", width - 6.0, height - 4.0),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn segment_coordinates_use_plot_width_not_canvas_width() {
        let view = layout(&State::default(), 1190.0, false);
        let segment = &view.lanes[0].segments[0];
        let scale = (1190.0 - 190.0 - 10.0) / (END - START) as f32;
        assert!((segment.x - (190.0 + 43.0 * scale)).abs() < 0.001);
        assert!((segment.width - (147.0 * scale - 1.0)).abs() < 0.001);
    }
}
