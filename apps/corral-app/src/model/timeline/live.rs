//! The daemon's recorded runs and events as a [`Plot`]. Every time is corral's recorded instant
//! in the viewer's zone. A run is a span (a worker's run, a reviewer's review); an open run runs
//! to now, idle from when its output went quiet. Events are instants: gates and landings on
//! their task's lane, the owner's moves on the owner lane, the pipeline's and assistants' on the
//! automatic lane. A block waits, and a ready review is held for acceptance, until the task's
//! next move.

use std::collections::{BTreeMap, BTreeSet};

use corral_protocol::detail::{Burnup as WireBurnup, DetailRef, TimelineBy, TimelinePage};
use corral_types::{Actor, Event, EventKind, Outcome, Role, Run, SessionRef};
use jiff::tz::TimeZone;
use jiff::{Timestamp, ToSpan, Zoned};

use super::{
    BURNUP_COUNTS, BurnupLine, BurnupSeries, Gap, Grouping, Kind, Marker, Opens, Plot, PlotLane,
    PlotSegment, PlotTick, Range, State, TallyEntry,
};
use crate::model::daemon::Feed;
use crate::model::work::{age, stamp, when};
use crate::model::{Id, Live};

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
    /// Whole minutes from the window's start.
    fn minutes(&self, at: Timestamp) -> i64 {
        (at.as_second() - self.from.as_second()).div_euclid(60)
    }

    fn now_minutes(&self) -> i64 {
        self.minutes(self.now)
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

fn outcome(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Done => "done",
        Outcome::Landed => "landed",
        Outcome::Failed => "failed",
        Outcome::Stopped => "stopped",
        Outcome::Superseded => "superseded",
    }
}

/// An event kind as words: "sent back", "review started".
fn moved(kind: EventKind) -> String {
    kind.as_str().replace('_', " ")
}

/// Events that are a task's next move, ending a block or an acceptance hold.
fn ends_wait(kind: EventKind) -> bool {
    !matches!(
        kind,
        EventKind::Created
            | EventKind::Edited
            | EventKind::Commented
            | EventKind::Artifact
            | EventKind::Error
            | EventKind::Gate
            | EventKind::Verdict
            | EventKind::ReviewStarted
    )
}

/// A lane while it is being filled; `first` orders the lanes by their earliest mark.
struct Lane {
    title: String,
    first: i64,
    segments: Vec<PlotSegment>,
    waits: Vec<String>,
    opens: Opens,
}

/// Lays recorded instants and spans on the window's clock, clipped to the window up to now.
struct Clock<'a> {
    window: &'a Window,
    feed: &'a Feed,
    now: i64,
    lanes: BTreeMap<String, Lane>,
}

/// A task's lane opens the task.
fn opens_task(task: &str) -> Opens {
    Opens {
        live: Live::Task(Id::new(task)),
        label: format!("Open {task}"),
    }
}

/// A run's lane opens its session terminal while the run is live, and the retained log of a
/// headless run that ended; any other ended run has no session to show, so its lane opens the
/// task.
fn opens_run(run: &Run, feed: &Feed) -> Opens {
    let session = format!("{}_{}", run.task, run.id);
    let logged = run.ended.is_some() && matches!(run.session, Some(SessionRef::Process { .. }));
    if feed.agents.contains_key(session.as_str()) {
        Opens {
            label: format!("Open the terminal of {} {}", run.task, run.id),
            live: Live::Session(Id::from(session.as_str())),
        }
    } else if logged {
        Opens {
            label: format!("Open the log of {} {} (ended)", run.task, run.id),
            live: Live::Session(Id::from(session.as_str())),
        }
    } else {
        Opens {
            label: format!("Open {} (run {} has ended)", run.task, run.id),
            live: Live::Task(Id::new(&run.task)),
        }
    }
}

impl Clock<'_> {
    /// Lane `id`, made with its title and what it opens the first time it is named.
    fn lane(&mut self, id: &str, make: impl FnOnce() -> (String, Opens)) {
        self.lanes.entry(id.to_owned()).or_insert_with(|| {
            let (title, opens) = make();
            Lane {
                title,
                first: i64::MAX,
                segments: Vec::new(),
                waits: Vec::new(),
                opens,
            }
        });
    }

    /// Puts `kind` from `from` to `to` (None: still open, to now) on lane `id`, with `what` in
    /// its label; nothing when it lies wholly outside the window.
    fn span(&mut self, id: &str, kind: Kind, from: Timestamp, to: Option<Timestamp>, what: &str) {
        let (start, end) = (
            self.window.minutes(from).max(0),
            to.map_or(self.now, |to| self.window.minutes(to))
                .min(self.now),
        );
        if end < start {
            return;
        }
        let from_words = self.window.clock(start, self.feed);
        let label = if start == end && to.is_some() {
            format!("{id} {what} {from_words}")
        } else {
            let until = match to {
                Some(_) => self.window.clock(end, self.feed),
                None => "now (still open)".to_owned(),
            };
            format!("{id} {what} {from_words}–{until}")
        };
        let wait = matches!(kind, Kind::Ask | Kind::Held) && end - start >= GAP_MINUTES;
        let wait = wait.then(|| {
            let state = if kind == Kind::Ask {
                "blocked"
            } else {
                "awaiting acceptance"
            };
            format!("{from_words} · {state} · {}", duration(end - start))
        });
        let Some(lane) = self.lanes.get_mut(id) else {
            return;
        };
        lane.first = lane.first.min(start);
        lane.waits.extend(wait);
        lane.segments.push(PlotSegment {
            kind,
            start,
            end,
            label,
        });
    }

    /// A run as its span: idle from when its output went quiet while it is still open.
    fn run(&mut self, id: &str, run: &Run) {
        let who = format!(
            "{} {}{}",
            run.id,
            run.role.as_str(),
            run.model
                .as_deref()
                .map_or_else(|| format!(" · {}", run.profile), |m| format!(" · {m}")),
        );
        let kind = match run.role {
            Role::Worker => Kind::Run,
            Role::Reviewer => Kind::Review,
        };
        match (run.ended, run.quiet_since) {
            (None, Some(quiet)) if quiet > run.started => {
                self.span(id, kind, run.started, Some(quiet), &who);
                self.span(id, Kind::Idle, quiet, None, &format!("{who} quiet"));
            }
            (ended, _) => {
                let what = match run.outcome {
                    Some(end) => format!("{who} · {}", outcome(end)),
                    None => who,
                };
                self.span(id, kind, run.started, ended, &what);
            }
        }
    }

    /// A task's events on lane `id`: gates, landings, drops and reopens as instants, blocks and
    /// acceptance holds as waits until the next move.
    fn events(&mut self, id: &str, events: &[&Event]) {
        let mut open: Option<(Kind, Timestamp)> = None;
        for event in events {
            if ends_wait(event.kind)
                && let Some((kind, from)) = open.take()
            {
                self.span(id, kind, from, Some(event.ts), kind.label());
            }
            let instant = match event.kind {
                EventKind::Blocked => {
                    open = Some((Kind::Ask, event.ts));
                    None
                }
                EventKind::Ready => {
                    open = Some((Kind::Held, event.ts));
                    None
                }
                EventKind::Gate => Some(Kind::Gate),
                EventKind::Landed => Some(Kind::Land),
                EventKind::Dropped => Some(Kind::Drop),
                EventKind::Reopened => Some(Kind::Reopen),
                _ => None,
            };
            if let Some(kind) = instant {
                let what = if kind == Kind::Land {
                    "landed"
                } else {
                    kind.label()
                };
                self.span(id, kind, event.ts, Some(event.ts), what);
            }
        }
        if let Some((kind, from)) = open {
            self.span(id, kind, from, None, kind.label());
        }
    }

    /// An action mark: `HH:MM T-1 moved`.
    fn marker(&self, event: &Event) -> Option<Marker> {
        let at = self.window.minutes(event.ts);
        (0..=self.now).contains(&at).then(|| Marker {
            at,
            label: format!(
                "{} {}{}",
                self.window.clock(at, self.feed),
                event
                    .task
                    .as_deref()
                    .map(|task| format!("{task} "))
                    .unwrap_or_default(),
                moved(event.kind)
            ),
        })
    }
}

/// The recorded page laid on the window's clock.
pub fn plot(page: &TimelinePage, window: &Window, grouping: Grouping, feed: &Feed) -> Plot {
    let now = window.now_minutes();
    let mut clock = Clock {
        window,
        feed,
        now,
        lanes: BTreeMap::new(),
    };
    let mut events: Vec<&Event> = page.events.iter().collect();
    events.sort_by_key(|event| (event.ts, event.seq));
    match grouping {
        Grouping::ByTask => {
            for run in &page.runs {
                clock.lane(&run.task, || {
                    (task_title(&run.task, feed), opens_task(&run.task))
                });
                clock.run(&run.task, run);
            }
            let mut by_task: BTreeMap<&str, Vec<&Event>> = BTreeMap::new();
            for event in &events {
                if let Some(task) = event.task.as_deref() {
                    by_task.entry(task).or_default().push(event);
                }
            }
            for (task, events) in by_task {
                clock.lane(task, || (task_title(task, feed), opens_task(task)));
                clock.events(task, &events);
            }
        }
        Grouping::ByAgent => {
            for run in &page.runs {
                let id = format!("{}/{}", run.task, run.id);
                clock.lane(&id, || {
                    (
                        format!("{} · {}", run.role.as_str(), run.profile),
                        opens_run(run, feed),
                    )
                });
                clock.run(&id, run);
                for event in events.iter().filter(|event| {
                    event.task.as_deref() == Some(run.task.as_str())
                        && event.run.as_deref() == Some(run.id.as_str())
                }) {
                    match event.kind {
                        EventKind::Gate => {
                            clock.span(&id, Kind::Gate, event.ts, Some(event.ts), "gate")
                        }
                        EventKind::Landed => {
                            clock.span(&id, Kind::Land, event.ts, Some(event.ts), "landed")
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    let owner: Vec<Marker> = events
        .iter()
        .filter(|event| event.actor == Actor::Owner)
        .filter_map(|event| clock.marker(event))
        .collect();
    let auto: Vec<Marker> = events
        .iter()
        .filter(|event| {
            matches!(
                event.actor,
                Actor::Auto | Actor::Assistant | Actor::Extension
            )
        })
        .filter_map(|event| clock.marker(event))
        .collect();
    let mut lanes: Vec<(i64, String, Lane)> = clock
        .lanes
        .into_iter()
        .filter(|(_, lane)| !lane.segments.is_empty())
        .map(|(id, lane)| (lane.first, id, lane))
        .collect();
    lanes.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    let earliest = lanes
        .iter()
        .map(|lane| lane.0)
        .chain(owner.iter().chain(&auto).map(|marker| marker.at))
        .fold(now, i64::min);
    let start = match window.range {
        Range::Today => (earliest / 60 * 60).min(now / 60 * 60 - 60).max(0),
        Range::SevenDays => 0,
    };
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
        ticks: ticks(window, start, now),
        owner,
        auto,
        lanes: lanes
            .into_iter()
            .map(|(_, id, lane)| PlotLane {
                id,
                title: lane.title,
                segments: lane.segments,
                wait: (!lane.waits.is_empty()).then(|| lane.waits.join(" · ")),
                opens: lane.opens,
            })
            .collect(),
        now: Marker {
            at: now,
            label: "now".into(),
        },
        accessible_label,
    }
}

/// A task's title from the board; a closed task is not on it and keeps its id alone.
fn task_title(task: &str, feed: &Feed) -> String {
    feed.board
        .task(task)
        .map(|task| task.title().to_owned())
        .unwrap_or_default()
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

/// The longest recorded blocks, acceptance holds and idle stretches in the window, longest
/// first.
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
                Kind::Ask => format!("blocked for {} until the next move", duration(length)),
                Kind::Held => format!("waited for acceptance for {}", duration(length)),
                _ => format!("its run went quiet for {}", duration(length)),
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
        Range::Today => "No block, acceptance hold or idle run lasted an hour or more today.",
        Range::SevenDays => {
            "No block, acceptance hold or idle run lasted an hour or more in the last seven days."
        }
    }
}

/// How the gaps are derived from what corral records.
pub const LIVE_GAPS_NOTE: &str = "From corral's recorded runs and events: a block or a ready \
                                  review waits until the task's next move; a run is idle from \
                                  when its output went quiet.";

/// The burn-up as a step series from its first recorded event to now: scope counts the tasks
/// created, less those dropped; landed counts the tasks landed, less those reopened. A reopened
/// drop comes back into scope. None when corral has recorded none of these.
pub fn burnup_series(burnup: &WireBurnup, now: u64, feed: &Feed) -> Option<BurnupSeries> {
    let now = Timestamp::from_second(i64::try_from(now).ok()?).ok()?;
    let mut events: Vec<&Event> = burnup
        .events
        .iter()
        .filter(|event| event.ts <= now)
        .collect();
    events.sort_by_key(|event| (event.ts, event.seq));
    let mut scope = BTreeSet::new();
    let mut landed = BTreeSet::new();
    let mut points: Vec<(Timestamp, u32, u32)> = Vec::new();
    // Created, landed, dropped and reopened events, in the legend's order.
    let mut tally = [0u32; 4];
    for event in events {
        let Some(task) = event.task.as_deref() else {
            continue;
        };
        if let Some(at) = [
            EventKind::Created,
            EventKind::Landed,
            EventKind::Dropped,
            EventKind::Reopened,
        ]
        .iter()
        .position(|kind| *kind == event.kind)
        {
            tally[at] += 1;
        }
        match event.kind {
            EventKind::Created => {
                scope.insert(task);
            }
            EventKind::Landed => {
                // A task landed without its creation in the store still counts in scope.
                scope.insert(task);
                landed.insert(task);
            }
            EventKind::Dropped => {
                scope.remove(task);
                landed.remove(task);
            }
            EventKind::Reopened => {
                scope.insert(task);
                landed.remove(task);
            }
            _ => continue,
        }
        let count = |set: &BTreeSet<&str>| u32::try_from(set.len()).unwrap_or(u32::MAX);
        let point = (event.ts, count(&scope), count(&landed));
        match points.last_mut() {
            Some(last) if last.0 == event.ts => *last = point,
            _ => points.push(point),
        }
    }
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
    let scope = match burnup.scope.split_once(':') {
        Some((kind, id)) => format!("{kind} {id}"),
        None => burnup.scope.clone(),
    };
    Some(BurnupSeries {
        title: format!("Goal burn-up · {scope}"),
        period: format!("since {day}"),
        points: series,
        first: day,
        last: "today".into(),
        counts: BURNUP_COUNTS,
        tally: [
            ("created", Some(BurnupLine::Scope)),
            ("landed", Some(BurnupLine::Landed)),
            ("dropped", None),
            ("reopened", None),
        ]
        .into_iter()
        .zip(tally)
        .map(|((word, line), count)| TallyEntry {
            words: format!("{word} {count}"),
            line,
        })
        .collect(),
    })
}

#[cfg(test)]
#[path = "regression.rs"]
mod regression;
