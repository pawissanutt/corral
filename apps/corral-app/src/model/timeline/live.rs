//! The daemon's recorded spans as a [`Plot`]. Every time is corral's recorded instant in the
//! viewer's zone; a phase without a recorded start is said on its lane, never drawn at a guessed
//! time, and an open phase (recorded start, no end yet) runs to now and says so.

use corral_protocol::detail::{
    Burnup as WireBurnup, DetailRef, TimelineBy, TimelineKind, TimelinePage,
};
use jiff::tz::TimeZone;
use jiff::{Timestamp, ToSpan, Zoned};

use super::{
    BurnupSeries, Gap, Grouping, Kind, Marker, Plot, PlotLane, PlotPm, PlotSegment, PlotTick,
    Range, State,
};
use crate::model::daemon::Feed;
use crate::model::hue::Hue;
use crate::model::usage::completeness_label;
use crate::model::work::{age, stamp, when};

/// Waits and idle stretches at least this long become gaps worth a look.
const GAP_MINUTES: i64 = 60;
/// At most this many gaps are listed, longest first.
const GAP_LIMIT: usize = 5;

/// The requested local window: today from local midnight, or the seven local days ending today.
/// The bounds are whole local days, so the request stays the same for the whole day.
#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    pub from: Timestamp,
    pub to: Timestamp,
    pub start: Zoned,
    pub range: Range,
    pub now: Timestamp,
}

/// The window for `range` at `now` (Unix seconds) in `tz`; None before the clock is known.
pub fn window(range: Range, now: u64, tz: &TimeZone) -> Option<Window> {
    if now == 0 {
        return None;
    }
    let now = Timestamp::from_second(i64::try_from(now).ok()?).ok()?;
    let today = now.to_zoned(tz.clone()).start_of_day().ok()?;
    let start = match range {
        Range::Today => today.clone(),
        Range::SevenDays => today.checked_sub(6.days()).ok()?,
    };
    let end = today.checked_add(1.day()).ok()?;
    Some(Window {
        from: start.timestamp(),
        to: end.timestamp(),
        start,
        range,
        now,
    })
}

/// The Timeline detail to request for the selection and window.
pub fn request(state: &State, window: &Window) -> DetailRef {
    DetailRef::Timeline {
        from: window.from.to_string(),
        to: window.to.to_string(),
        by: match state.grouping {
            Grouping::ByTask => TimelineBy::Task,
            Grouping::ByAgent => TimelineBy::Agent,
        },
    }
}

impl Window {
    /// Whole minutes from the window's start; None for an instant corral did not record.
    fn minutes(&self, at: &str) -> Option<i64> {
        let at: Timestamp = at.parse().ok()?;
        Some((at.as_second() - self.from.as_second()).div_euclid(60))
    }

    fn now_minutes(&self) -> i64 {
        (self.now.as_second() - self.from.as_second()).div_euclid(60)
    }

    /// Timeline labels use the same timestamp wording as task and session details.
    fn clock(&self, minutes: i64, feed: &Feed) -> String {
        Timestamp::from_second(self.from.as_second() + minutes * 60)
            .map(|at| when(&at.to_string(), feed))
            .unwrap_or_default()
    }
}

fn weekday(local: &Zoned) -> &'static str {
    use jiff::civil::Weekday;
    match local.weekday() {
        Weekday::Monday => "Mon",
        Weekday::Tuesday => "Tue",
        Weekday::Wednesday => "Wed",
        Weekday::Thursday => "Thu",
        Weekday::Friday => "Fri",
        Weekday::Saturday => "Sat",
        Weekday::Sunday => "Sun",
    }
}

fn duration(minutes: i64) -> String {
    age(
        Some(0),
        u64::try_from(minutes.saturating_mul(60)).unwrap_or(0),
    )
}

fn kind(kind: TimelineKind) -> Option<Kind> {
    Some(match kind {
        TimelineKind::Run => Kind::Run,
        TimelineKind::Gate => Kind::Gate,
        TimelineKind::Review => Kind::Review,
        TimelineKind::Land => Kind::Land,
        TimelineKind::Ask => Kind::Ask,
        TimelineKind::Held => Kind::Held,
        TimelineKind::Idle => Kind::Idle,
        TimelineKind::Unavailable => Kind::Unavailable,
        TimelineKind::Other => return None,
    })
}

/// The recorded page laid on the window's clock.
pub fn plot(page: &TimelinePage, window: &Window, feed: &Feed) -> Plot {
    let now = window.now_minutes();
    let in_window = |at: i64| (0..=now).contains(&at);
    let mut earliest = now;
    let lanes: Vec<PlotLane> = page
        .lanes
        .iter()
        .map(|lane| {
            let mut segments = Vec::new();
            let mut missing = Vec::new();
            let mut waits = Vec::new();
            for segment in &lane.segments {
                let Some(kind) = kind(segment.kind) else {
                    missing.push("an unknown phase".to_owned());
                    continue;
                };
                let start = segment.from.as_deref().and_then(|at| window.minutes(at));
                let end = match &segment.to {
                    Some(at) => window.minutes(at),
                    None => Some(now),
                };
                let (Some(start), Some(end)) = (start, end) else {
                    missing.push(match &segment.note {
                        Some(note) => format!("{} ({note})", kind.label()),
                        None => kind.label().to_owned(),
                    });
                    continue;
                };
                let (start, end) = (start.max(0), end.min(now));
                if end < start || end < 0 || start > now {
                    continue;
                }
                earliest = earliest.min(start);
                let open = segment.to.is_none();
                let until = if open {
                    "now (still open)".to_owned()
                } else {
                    window.clock(end, feed)
                };
                let length = end - start;
                let wait = matches!(kind, Kind::Ask | Kind::Held) && length >= GAP_MINUTES;
                if wait {
                    let what = if kind == Kind::Ask {
                        "waiting"
                    } else {
                        "awaiting acceptance"
                    };
                    waits.push(format!(
                        "{} · {what} · {}",
                        window.clock(start, feed),
                        duration(length)
                    ));
                }
                segments.push(PlotSegment {
                    kind,
                    start,
                    end,
                    label: format!(
                        "{} {} {}–{until}{}",
                        lane.id,
                        kind.label(),
                        window.clock(start, feed),
                        segment
                            .note
                            .as_ref()
                            .map(|n| format!(" · {n}"))
                            .unwrap_or_default()
                    ),
                    note: None,
                    phone_note: None,
                });
            }
            PlotLane {
                id: lane.id.clone(),
                title: lane
                    .title
                    .strip_prefix(lane.id.as_str())
                    .and_then(|title| title.strip_prefix(" · "))
                    .unwrap_or(&lane.title)
                    .to_owned(),
                segments,
                ask: None,
                wait: (!waits.is_empty()).then(|| waits.join(" · ")),
                missing: (!missing.is_empty())
                    .then(|| format!("times not recorded: {}", missing.join(", "))),
            }
        })
        .collect();
    let owner: Vec<Marker> = page
        .owner_actions
        .iter()
        .filter_map(|action| {
            let at = window.minutes(&action.at).filter(|&at| in_window(at))?;
            Some(Marker {
                at,
                label: format!("{} {}", window.clock(at, feed), action.text),
            })
        })
        .collect();
    let pm: Vec<PlotPm> = page
        .pm
        .iter()
        .filter_map(|action| {
            let at = window.minutes(&action.at).filter(|&at| in_window(at))?;
            Some(PlotPm {
                start: at,
                end: at,
                hue: Hue::Neutral,
                label: None,
                accessible_label: format!("PM {} {}", window.clock(at, feed), action.text),
            })
        })
        .collect();
    for at in owner.iter().map(|m| m.at).chain(pm.iter().map(|p| p.start)) {
        earliest = earliest.min(at);
    }
    let start = match window.range {
        Range::Today => (earliest / 60 * 60).min(now / 60 * 60 - 60).max(0),
        Range::SevenDays => 0,
    };
    let ticks = ticks(window, start, now);
    let last_visit = page
        .last_visit
        .as_deref()
        .and_then(|at| window.minutes(at))
        .filter(|&at| (start..=now).contains(&at))
        .map(|at| Marker {
            at,
            label: format!("your last visit {}", window.clock(at, feed)),
        });
    let accessible_label = match window.range {
        Range::Today => format!(
            "Run timeline {} to {}",
            window.clock(start, feed),
            window.clock(now, feed)
        ),
        Range::SevenDays => format!("Run timeline, seven days to {}", window.clock(now, feed)),
    };
    Plot {
        start,
        end: now.max(start + 60),
        ticks,
        owner,
        pm,
        lanes,
        last_visit,
        now: Marker {
            at: now,
            label: "now".into(),
        },
        accessible_label,
        completeness: completeness_label(&page.completeness),
    }
}

fn ticks(window: &Window, start: i64, now: i64) -> Vec<PlotTick> {
    match window.range {
        Range::Today => ((start + 59) / 60..=now / 60)
            .filter_map(|hour| {
                let at = Timestamp::from_second(window.from.as_second() + hour * 3600).ok()?;
                let local = at.to_zoned(window.start.time_zone().clone());
                Some(PlotTick {
                    at: hour * 60,
                    // The zone distinguishes the two occurrences of a repeated fall-back hour.
                    label: local.strftime("%H:%M %Z").to_string(),
                    minor: local.hour() % 2 != 0,
                })
            })
            .collect(),
        Range::SevenDays => (0..7)
            .filter_map(|day| {
                let local = window.start.checked_add(day.days()).ok()?;
                Some(PlotTick {
                    at: (local.timestamp().as_second() - window.from.as_second()).div_euclid(60),
                    label: weekday(&local).to_owned(),
                    minor: day % 2 != 0,
                })
            })
            .collect(),
    }
}

/// The longest recorded waits, holds and idle stretches in the window, longest first.
pub fn gaps(plot: &Plot, window: &Window, feed: &Feed) -> Vec<Gap> {
    let mut gaps: Vec<(i64, Gap)> = plot
        .lanes
        .iter()
        .flat_map(|lane| lane.segments.iter().map(move |segment| (lane, segment)))
        .filter(|(_, s)| {
            matches!(s.kind, Kind::Ask | Kind::Held | Kind::Idle) && s.end - s.start >= GAP_MINUTES
        })
        .map(|(lane, s)| {
            let length = s.end - s.start;
            let until = if s.end == plot.now.at {
                "now".to_owned()
            } else {
                window.clock(s.end, feed)
            };
            let wording = match s.kind {
                Kind::Ask => format!("waited for {}", duration(length)),
                Kind::Held => format!("waited for acceptance for {}", duration(length)),
                _ => format!("idle for {} with no recorded work", duration(length)),
            };
            (
                length,
                Gap {
                    time: format!("{}–{until}", window.clock(s.start, feed)),
                    task: lane.id.clone(),
                    duration: duration(length),
                    kind: s.kind,
                    wording,
                },
            )
        })
        .collect();
    gaps.sort_by(|a, b| b.0.cmp(&a.0));
    gaps.into_iter()
        .take(GAP_LIMIT)
        .map(|(_, gap)| gap)
        .collect()
}

/// What the gaps section says when nothing in the window waited an hour or more.
pub fn empty_gaps_line(range: Range) -> &'static str {
    match range {
        Range::Today => "No wait, acceptance hold or idle stretch lasted an hour or more today.",
        Range::SevenDays => {
            "No wait, acceptance hold or idle stretch lasted an hour or more in the last seven days."
        }
    }
}

/// How the live gaps are derived, in place of the preview's illustrative-data note.
pub const LIVE_GAPS_NOTE: &str = "From corral's recorded phase times. Phases recorded without times are named on their lane, not drawn.";

/// The burn-up as a step series from its first recorded change to now; None when corral has
/// recorded no goal changes.
pub fn burnup_series(burnup: &WireBurnup, now: u64, feed: &Feed) -> Option<BurnupSeries> {
    let now = Timestamp::from_second(i64::try_from(now).ok()?).ok()?;
    let mut points: Vec<(Timestamp, u32, u32)> = burnup
        .points
        .iter()
        .filter_map(|p| Some((p.at.parse::<Timestamp>().ok()?, p.total, p.landed)))
        .filter(|p| p.0 <= now)
        .collect();
    points.sort_by_key(|p| p.0);
    let first = points.first()?.0;
    let span = (now.as_second() - first.as_second()).max(1) as f32;
    let at = |t: Timestamp| (t.as_second() - first.as_second()) as f32 / span;
    let mut series = Vec::with_capacity(points.len() * 2 + 1);
    let mut last: Option<(u32, u32)> = None;
    for (t, total, landed) in &points {
        if let Some((total, landed)) = last {
            series.push((at(*t), total, landed));
        }
        series.push((at(*t), *total, *landed));
        last = Some((*total, *landed));
    }
    if let Some((total, landed)) = last {
        series.push((1.0, total, landed));
    }
    let day = stamp(&first.to_string(), feed);
    let scope = match burnup.scope.as_str() {
        "all" | "workspace" => "workspace".to_owned(),
        other => other.to_owned(),
    };
    Some(BurnupSeries {
        title: format!("Goal burn-up · {scope}"),
        period: format!("since {day}"),
        points: series,
        first: day,
        last: "today".into(),
        disclosure: None,
        completeness: completeness_label(&burnup.completeness),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use corral_protocol::detail::{TimelineLane, TimelineSegment};

    #[test]
    fn segment_without_recorded_start_is_named_not_drawn() {
        let tz = TimeZone::UTC;
        let now = Timestamp::from_second(1_700_000_000).unwrap();
        let window = window(Range::Today, 1_700_000_000, &tz).unwrap();
        let page = TimelinePage {
            lanes: vec![TimelineLane {
                id: "T-1".into(),
                title: "t".into(),
                segments: vec![
                    TimelineSegment {
                        kind: TimelineKind::Gate,
                        from: None,
                        to: Some(now.to_string()),
                        note: None,
                    },
                    TimelineSegment {
                        kind: TimelineKind::Run,
                        from: Some((now - 2.hours()).to_string()),
                        to: None,
                        note: None,
                    },
                ],
            }],
            owner_actions: vec![],
            pm: vec![],
            last_visit: None,
            completeness: Default::default(),
        };
        let plot = plot(
            &page,
            &window,
            &Feed {
                now: 1_700_000_000,
                time_zone: Some(tz),
                ..Feed::default()
            },
        );
        let lane = &plot.lanes[0];
        assert_eq!(lane.segments.len(), 1);
        assert_eq!(lane.segments[0].end, plot.now.at);
        assert!(lane.missing.as_deref().is_some_and(|m| m.contains("gate")));
    }
}

#[cfg(test)]
#[path = "regression.rs"]
mod regression;
