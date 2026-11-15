//! The mockup's illustrative day, shown only in the explicit fixture preview. Times are minutes
//! after midnight.

use super::{BurnupSeries, Gap, Kind, Marker, Plot, PlotLane, PlotPm, PlotSegment, PlotTick};
use crate::model::hue::Hue;

struct Segment {
    kind: Kind,
    start: &'static str,
    end: &'static str,
    note: Option<&'static str>,
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

struct Lane {
    id: &'static str,
    title: &'static str,
    segments: &'static [Segment],
    ask: Option<Segment>,
}

/// Start of the illustrative day, minutes after midnight.
pub const START: u32 = 540;
/// End of the illustrative day, minutes after midnight.
pub const END: u32 = 1332;
const LAST_VISIT: u32 = 1120;

const LANES: &[Lane] = &[
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

const OWNER: &[(&str, &str)] = &[
    ("10:35", "commented on T-885 P1"),
    ("12:14", "read T-885 P2 review"),
    ("19:12", "accepted T-885 P2"),
    ("21:58", "asked in T-868 thread"),
];

const PM: &[(&str, &str, bool)] = &[
    ("09:00", "14:20", false),
    ("14:20", "15:05", true),
    ("15:05", "22:12", false),
];

fn mins(time: &str) -> i64 {
    let b = time.as_bytes();
    i64::from(b[0] - b'0') * 600
        + i64::from(b[1] - b'0') * 60
        + i64::from(b[3] - b'0') * 10
        + i64::from(b[4] - b'0')
}

/// The illustrative day's annotations: the question strip and the long waits carry words above
/// their bars on desktop, the stall on both forms.
fn notes(kind: Kind, overlay: bool) -> (Option<String>, Option<String>) {
    match (kind, overlay) {
        (_, true) => (Some("needs you 3h".into()), None),
        (Kind::Stall, false) => (
            Some("stall 16:40–19:28 · nobody moved".into()),
            Some("stall 16:40–19:28 · 2h48".into()),
        ),
        (Kind::Ask, false) => (Some("waiting on you · 7h02".into()), None),
        _ => (None, None),
    }
}

fn place(lane: &Lane, s: &Segment, overlay: bool) -> PlotSegment {
    let label = if overlay {
        s.note.unwrap_or("").to_owned()
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
    let (note, phone_note) = notes(s.kind, overlay);
    PlotSegment {
        kind: s.kind,
        start: mins(s.start),
        end: mins(s.end),
        label,
        note,
        phone_note,
    }
}

/// The illustrative day as a plot.
pub fn plot() -> Plot {
    Plot {
        start: i64::from(START),
        end: i64::from(END),
        ticks: (9..=22)
            .map(|h| PlotTick {
                at: h * 60,
                label: format!("{h:02}"),
                minor: h % 2 != 0,
            })
            .collect(),
        owner: OWNER
            .iter()
            .map(|&(time, description)| Marker {
                at: mins(time),
                label: format!("{time} {description}"),
            })
            .collect(),
        pm: PM
            .iter()
            .map(|&(start, end, relaunching)| PlotPm {
                start: mins(start),
                end: mins(end),
                hue: if relaunching { Hue::Held } else { Hue::Neutral },
                label: relaunching.then(|| "relaunching".to_owned()),
                accessible_label: format!(
                    "PM {start}–{end}{}",
                    if relaunching { " · relaunching" } else { "" }
                ),
            })
            .collect(),
        lanes: LANES
            .iter()
            .map(|lane| PlotLane {
                id: lane.id.to_owned(),
                title: lane.title.to_owned(),
                segments: lane
                    .segments
                    .iter()
                    .map(|s| place(lane, s, false))
                    .collect(),
                ask: lane.ask.as_ref().map(|s| place(lane, s, true)),
                missing: None,
                wait: None,
            })
            .collect(),
        last_visit: Some(Marker {
            at: i64::from(LAST_VISIT),
            label: "your last visit 18:40".into(),
        }),
        now: Marker {
            at: i64::from(END),
            label: "now".into(),
        },
        accessible_label: "Run timeline 09:00 to 22:12".into(),
        completeness: None,
    }
}

/// The illustrative gaps beneath the chart.
pub fn gaps() -> Vec<Gap> {
    [
        (
            "16:40–19:28",
            "T-852",
            "2h48",
            Kind::Stall,
            "held · external op; no sync tick moved it for 2h48. Today the daemon would have re-checked on its own loop.",
        ),
        (
            "12:10–19:12",
            "T-885",
            "7h02",
            Kind::Ask,
            "plan P2 waited 7h02 for your acceptance; a push would have reached the phone at 12:10.",
        ),
        (
            "19:12–now",
            "T-868",
            "3h",
            Kind::Ask,
            "question open 3h; the worker kept producing until 21:40, then idled.",
        ),
    ]
    .into_iter()
    .map(|(time, task, duration, kind, wording)| Gap {
        time: time.into(),
        task: task.into(),
        duration: duration.into(),
        kind,
        wording: wording.into(),
    })
    .collect()
}

/// The gaps section's illustrative-data disclosure.
pub const GAPS_DISCLOSURE: &str = "Segment boundaries are illustrative, reconstructed around real start times and change events. Real records show 4 stalls of 4–8.6 h ended only by the owner asking (docs/records/pm-runtime-redesign.md).";

const SCOPE: [u32; 14] = [41, 41, 44, 44, 47, 47, 50, 53, 53, 53, 56, 56, 56, 57];
const LANDED: [u32; 14] = [18, 19, 19, 22, 24, 24, 27, 29, 31, 31, 33, 35, 36, 37];

/// The illustrative fourteen-day burn-up.
pub fn burnup() -> BurnupSeries {
    BurnupSeries {
        title: "Goal burn-up · Co-work and PM goals".into(),
        period: "14 days".into(),
        points: SCOPE
            .iter()
            .zip(LANDED)
            .enumerate()
            .map(|(i, (&scope, landed))| (i as f32 / 13.0, scope, landed))
            .collect(),
        first: "10-26".into(),
        last: "today".into(),
        disclosure: Some("burn-up numbers illustrative"),
        completeness: None,
    }
}
