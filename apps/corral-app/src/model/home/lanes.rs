//! Working as lanes over today: each working task's runs, checks and waits drawn from the
//! Timeline detail corrald records (the request Activity › Timeline makes for Today by task, so
//! both share one read). A chat or a build has no recorded spans and keeps an empty track.

use corral_protocol::detail::{DetailBody, DetailRef};

use super::working;
use crate::model::daemon::Feed;
use crate::model::hue::Hue;
use crate::model::projects::Scope;
use crate::model::status::{Look, session_look, status_look, status_word};
use crate::model::timeline::{self, Grouping, Range};
use crate::model::work::age;
use crate::model::{Id, Live, agents::session_line, pipeline::phrase};

/// One span of a lane's track, as fractions of the plotted part of today.
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub from: f32,
    pub to: f32,
    pub hue: Hue,
    /// `run`, `gate`, `blocked`: the span's words for a screen reader.
    pub label: String,
}

/// One working thing as a lane.
#[derive(Debug, Clone, PartialEq)]
pub struct Lane {
    pub id: Id,
    pub live: Live,
    pub look: Look,
    pub word: &'static str,
    pub title: String,
    /// The pipeline's phrase, or what the session is doing.
    pub now: String,
    pub track: Vec<Segment>,
    /// How long it has been in its state: `2h05m`.
    pub age: String,
}

/// The Timeline detail the lanes read: today, by task; none before the clock is known.
pub fn lanes_request(feed: &Feed) -> Option<DetailRef> {
    let window = window(feed)?;
    Some(timeline::request(&timeline::State::default(), &window))
}

fn window(feed: &Feed) -> Option<timeline::Window> {
    let zone = feed.time_zone.clone().unwrap_or(jiff::tz::TimeZone::UTC);
    timeline::window(Range::Today, feed.now / 60 * 60, &zone)
}

/// The working things in scope as lanes, with the clock the tracks start at (`06:00`) once the
/// day's spans are loaded.
pub fn lanes(feed: &Feed, scope: &Scope) -> (Vec<Lane>, Option<String>) {
    let plot = window(feed).and_then(|window| {
        let request = timeline::request(&timeline::State::default(), &window);
        match feed.detail(&request)?.body()? {
            DetailBody::Timeline(page) => Some((
                timeline::live_plot(page, &window, Grouping::ByTask, feed),
                window,
            )),
            _ => None,
        }
    });
    let from = plot.as_ref().map(|(plot, window)| {
        let at = window.from.as_second() + plot.start * 60;
        jiff::Timestamp::from_second(at)
            .map(|at| crate::model::work::stamp_at(at, feed))
            .unwrap_or_default()
    });
    let lanes = working(feed, scope)
        .into_iter()
        .filter_map(|id| {
            let track = plot
                .as_ref()
                .and_then(|(plot, _)| {
                    let lane = plot.lanes.iter().find(|lane| lane.id == id.as_str())?;
                    let span = (plot.end - plot.start).max(1) as f32;
                    Some(
                        lane.segments
                            .iter()
                            .map(|segment| Segment {
                                from: ((segment.start - plot.start) as f32 / span).clamp(0.0, 1.0),
                                to: ((segment.end - plot.start) as f32 / span).clamp(0.0, 1.0),
                                hue: segment.kind.hue(),
                                label: segment.label.clone(),
                            })
                            .collect(),
                    )
                })
                .unwrap_or_default();
            lane(feed, id, track)
        })
        .collect();
    (lanes, from.filter(|from| !from.is_empty()))
}

fn lane(feed: &Feed, id: Id, track: Vec<Segment>) -> Option<Lane> {
    if let Some(task) = feed.board.task(&id) {
        return Some(Lane {
            live: Live::Task(id.clone()),
            look: status_look(task.status()),
            word: status_word(task.status()),
            title: task.title().to_owned(),
            now: phrase(task, feed),
            age: age(task.since(), feed.now),
            id,
            track,
        });
    }
    let session = session_line(feed, &id)?;
    Some(Lane {
        live: Live::Session(id.clone()),
        look: session_look(session.state),
        word: session.state.word(),
        title: if session.name.is_empty() {
            session.role.clone()
        } else {
            session.name.clone()
        },
        now: session.doing.unwrap_or_default(),
        age: String::new(),
        id,
        track,
    })
}
