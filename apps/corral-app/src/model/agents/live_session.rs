//! One live run from corrald: its row in Agents, its terminal (its herdr pane's screen, or the
//! tail of a headless session's log, read live where corrald serves it, else a recorded
//! capture) with the pane's keys bar and type field (`pane`), the strip under its head, its
//! Session menu and, from a corrald that serves them (`session.v1`), its bounded transcript
//! page with the reply box (`live_reply`). A run without a recorded pane or process has no
//! terminal; none is drawn for it.

use corral_protocol::app::SESSION_V1;
use corral_protocol::detail::{DetailBody, DetailRef, SessionPage, TerminalSnapshot};
use corral_protocol::rows::AgentRow;
use corral_types::SessionRef;

use crate::model::Id;
use crate::model::board::STAGES;
use crate::model::daemon::{self, DetailCache, Link};
use crate::model::effect::Env;
use crate::model::hue::Hue;
use crate::model::text::{Rich, Span, span};
use crate::model::work::session_task;

use super::live::{
    clock, clock_at, excerpt, quiet, role_label, session_name, shown_id, stale, unix,
};
use super::replay::transcript;
use super::{
    Action, Bar, FactValue, Feed, Fill, LineClass, Run, RunFact, Session, State, Terminal,
    Transcript, View, line, live_reply, pane, screen, tail_line,
};

/// Seconds within which a screen read counts as "just now": it is read about once a second.
const JUST_NOW: u64 = 5;

/// The page token of the messages before the shown page, when the daemon has older ones.
pub(super) fn older_token(cache: &DetailCache) -> Option<String> {
    match cache.body()? {
        DetailBody::Session(page) => page.older.clone(),
        _ => None,
    }
}

pub(super) fn page(cache: Option<&DetailCache>) -> Option<&SessionPage> {
    match cache?.body()? {
        DetailBody::Session(page) => Some(page),
        _ => None,
    }
}

fn snapshot(cache: Option<&DetailCache>) -> Option<&TerminalSnapshot> {
    match cache?.body()? {
        DetailBody::Terminal(snapshot) => Some(snapshot),
        _ => None,
    }
}

/// What failed, from the cache's error.
pub(super) fn failure(cache: Option<&DetailCache>) -> Option<String> {
    cache?
        .error
        .as_ref()
        .map(|error| format!("{} ({})", error.message, error.code))
}

/// When the shown screen was last confirmed: the latest reply's time, which a not-modified
/// answer advances, else the time the rows were read.
fn read_at<'a>(cache: Option<&'a DetailCache>, capture: &'a TerminalSnapshot) -> &'a str {
    cache
        .and_then(|cache| cache.pages.first())
        .map_or(capture.captured_at.as_str(), |page| page.as_of.as_str())
}

/// "just now" while reads keep arriving, else the read's clock (`21:40`, `3d ago`): words that
/// change once when reads stop, not with every tick, so the page is not redrawn each second.
fn read_when(at: &str, feed: &daemon::Feed) -> String {
    match unix(at) {
        Some(when) if feed.now > 0 && feed.now.saturating_sub(when) < JUST_NOW => "just now".into(),
        _ => clock(Some(at), feed).unwrap_or_else(|| "at an unknown time".into()),
    }
}

/// The daemon's reason without its machine prefix: `session has ended` from
/// `terminal_snapshot_unavailable: session has ended`.
fn reason(message: &str) -> &str {
    message
        .split_once(": ")
        .filter(|(head, _)| {
            !head.is_empty() && head.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
        })
        .map_or(message, |(_, rest)| rest)
        .trim_end_matches('.')
}

/// The run's screen: its herdr pane read live where corrald serves it, or the tail of a process
/// session's log. Live rows stay as read; only a current read is `Live`. Keys go to a pane from
/// the session page's keys bar; a log takes none.
pub(super) fn terminal(
    id: &Id,
    row: &AgentRow,
    cache: Option<&DetailCache>,
    feed: &daemon::Feed,
) -> Terminal {
    let pane = session_name(row).unwrap_or_else(|| "no session".into());
    let log = matches!(row.run.session, Some(SessionRef::Process { .. }));
    let current_words = if log { "log tail" } else { "live terminal" };
    let capture = snapshot(cache);
    let loading = cache.is_none_or(|cache| cache.loading);
    let error = cache.and_then(|cache| cache.error.as_ref());
    // A request in flight marks the cache stale until it answers, so only a settled stale cache
    // or a link that is down means the rows shown are no longer current.
    let current = feed.link == Link::Up
        && error.is_none()
        && cache.is_some_and(|cache| cache.loading || !cache.stale);
    let gone = error.is_some_and(|error| error.code == "not_found");
    let unavailable = error.filter(|error| error.code == "unavailable");
    let live = capture.is_some_and(|capture| capture.live);
    let status = match capture {
        Some(capture) if capture.live => {
            let read = read_when(read_at(cache, capture), feed);
            match (error, unavailable) {
                _ if current => format!("{current_words} · read {read}"),
                _ if gone => format!("pane gone · last screen read {read}"),
                (_, Some(why)) => format!("{} · last screen read {read}", reason(&why.message)),
                (Some(error), _) => format!(
                    "last screen received · read {read} · refreshing failed: {} ({})",
                    error.message, error.code
                ),
                (None, _) => {
                    format!("last screen received · read {read} · corrald is not answering")
                }
            }
        }
        Some(capture) => format!(
            "snapshot · captured {}",
            clock(Some(capture.captured_at.as_str()), feed)
                .unwrap_or_else(|| "at an unknown time".into())
        ),
        None if gone => "pane gone".to_owned(),
        None if unavailable.is_some() => "unavailable".to_owned(),
        None if error.is_some() => "not loaded".to_owned(),
        None if loading => "loading".to_owned(),
        None => "no screen yet".to_owned(),
    };
    let empty = match capture {
        Some(capture) if capture.lines.is_empty() => Some(if capture.live {
            "The pane's screen is blank.".to_owned()
        } else {
            "The capture is empty.".to_owned()
        }),
        Some(_) => None,
        None => Some(match (error, unavailable) {
            _ if gone => format!("The session is gone: the runner no longer has {pane}."),
            (_, Some(why)) => format!("The terminal is unavailable: {}.", reason(&why.message)),
            (Some(error), _) => format!(
                "The terminal could not be loaded: {} ({}).",
                error.message, error.code
            ),
            (None, _) if loading => "Loading the terminal…".to_owned(),
            (None, _) => "No screen has arrived; it shows once corrald answers.".to_owned(),
        }),
    };
    Terminal {
        feed: if live && current {
            Feed::Live
        } else {
            Feed::Snapshot
        },
        heading: if log {
            format!("{} · {pane} · session.log", row.run.harness)
        } else {
            format!("{} · {pane}", row.run.harness)
        },
        status,
        input: if log { "read-only log" } else { "herdr pane" }.into(),
        lines: capture
            .map(|capture| {
                if capture.live {
                    capture
                        .lines
                        .iter()
                        .map(|row| line(row, LineClass::Plain))
                        .collect()
                } else {
                    capture.lines.iter().map(|text| tail_line(text)).collect()
                }
            })
            .unwrap_or_default(),
        cursor: false,
        verbatim: live,
        styled: capture
            .filter(|capture| capture.live)
            .map(|capture| screen::rows(&capture.rows))
            .unwrap_or_default(),
        empty,
        refresh: Some(Action::Refresh(DetailRef::Terminal { id: id.to_string() })),
    }
}

fn fact(label: &'static str, value: Rich) -> RunFact {
    RunFact {
        label,
        value: FactValue::Text(value),
    }
}

/// A profile, model, harness or session name: an identifier (mono, cut rather than broken where
/// room is short) when known, else the plain words saying it is not.
fn identifier(name: Option<String>, unknown: &str) -> Span {
    match name {
        Some(name) => span(name).mono(),
        None => span(unknown),
    }
}

/// The run's facts beside its transcript: its task and rail, role, profile, model, session,
/// branch, start and how long it has been quiet.
fn facts(
    row: Option<&AgentRow>,
    board_task: Option<&daemon::Task>,
    task: &str,
    feed: &daemon::Feed,
) -> Vec<RunFact> {
    let mut value = vec![span(task.to_owned()).mono()];
    if let Some(found) = board_task {
        value.extend([span(" "), span(found.title().to_owned())]);
    }
    let mut facts = vec![fact("Task", value)];
    if let Some(found) = board_task {
        facts.push(RunFact {
            label: "Rail",
            value: FactValue::Rail(std::array::from_fn(|at| found.rail[at].mark)),
        });
    }
    let Some(row) = row else {
        return facts;
    };
    let run = &row.run;
    facts.extend([
        fact(
            "Agent",
            vec![
                span(format!("{} ", role_label(run.role.as_str()))),
                span(run.id.clone()).mono(),
            ],
        ),
        fact("Profile", vec![span(run.profile.clone()).mono()]),
        fact("Model", vec![identifier(run.model.clone(), "not recorded")]),
        fact(
            "Session",
            vec![
                span(run.harness.clone()).mono(),
                span(" · "),
                identifier(session_name(row), "none recorded"),
            ],
        ),
        fact("Branch", vec![span(run.branch.clone()).mono()]),
        fact(
            "Started",
            vec![span(
                clock_at(run.started, feed).unwrap_or_else(|| "unknown".into()),
            )],
        ),
    ]);
    if let Some(quiet) = quiet(row, feed) {
        facts.push(fact("Output", vec![span(quiet)]));
    }
    facts
}

/// Build the session page for a run session id (`T-1_R1`). `via` is this device's authenticated
/// attribution, as replies carry it.
pub fn session(state: &State, id: &Id, feed: &daemon::Feed, env: Env, via: &str) -> Session {
    let row = feed.agents.get(id.as_str());
    let transcripts = feed.capable(SESSION_V1);
    let session_cache = transcripts
        .then(|| feed.detail(&DetailRef::Session { id: id.to_string() }))
        .flatten();
    let body = page(session_cache);
    let role = row
        .map(|row| row.run.role.as_str().to_owned())
        .or_else(|| body.map(|page| page.role.clone()))
        .unwrap_or_else(|| "agent".into());
    let task = row
        .map(|row| row.run.task.clone())
        .or_else(|| body.and_then(|page| page.task.clone()))
        .or_else(|| crate::model::work::session_task(id).map(str::to_owned));
    let run = row
        .map(|row| row.run.id.clone())
        .or_else(|| body.and_then(|page| page.run.clone()));
    let board_task = task.as_deref().and_then(|task| feed.board.task(task));
    let shown = row
        .map(shown_id)
        .or_else(|| task.clone())
        .unwrap_or_else(|| id.to_string());
    let title = board_task
        .map(|task| task.title().to_owned())
        .unwrap_or_else(|| role_label(&role));
    let terminal = row.filter(|row| row.run.session.is_some()).map(|row| {
        terminal(
            id,
            row,
            feed.detail(&DetailRef::Terminal { id: id.to_string() }),
            feed,
        )
    });
    let mut sub = vec![
        [Some(role_label(&role)), run.clone()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" "),
    ];
    sub.extend(row.map(|row| row.run.profile.clone()));
    if terminal.is_none() {
        sub.push("no terminal".into());
    }
    let notice = match (row, body) {
        (None, Some(_)) => Some((
            Hue::Neutral,
            "This run is no longer live; this is its last recorded transcript.".into(),
        )),
        (None, None) if !stale(feed, "agents") && feed.topics.contains_key("agents") => {
            Some((Hue::Held, format!("corrald lists no live run {id} now.")))
        }
        _ => None,
    };
    let steps = board_task
        .map(|found| {
            STAGES
                .iter()
                .zip(&found.rail)
                .map(|(stage, step)| {
                    let detail = [
                        step.sha7.clone(),
                        step.at
                            .filter(|_| feed.now > 0)
                            .map(|at| format!("{} ago", daemon::age(Some(at), feed.now))),
                    ]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" · ");
                    let words = if detail.is_empty() {
                        (*stage).to_owned()
                    } else {
                        format!("{stage} · {detail}")
                    };
                    (step.mark, words)
                })
                .collect()
        })
        .unwrap_or_default();
    let switch = transcripts && terminal.is_some();
    let view = match &terminal {
        Some(_) if !transcripts => View::Terminal,
        Some(_) => state.view_for(feed, id.as_str()),
        None => View::Transcript,
    };
    Session {
        id: shown.clone(),
        title,
        sub: sub.join(" · "),
        switch_key: "`",
        switch_hint: "switch",
        phone_title: format!("{shown} session"),
        desktop_back_label: "Agents",
        back: Action::Back,
        view,
        switch,
        switch_labels: ["Transcript", "Terminal"],
        switch_actions: [
            Action::SetView(View::Transcript),
            Action::SetView(View::Terminal),
        ],
        toggle: Action::ToggleView,
        notice,
        controls: Bar {
            state: row.map(|row| super::live::status(&row.status).chip()),
            lead: row.and_then(excerpt).map(str::to_owned),
            menu: session_task(id).map(|_| id.clone()),
        },
        transcript: if transcripts {
            transcript(id, state, session_cache, feed)
        } else {
            unserved()
        },
        pane: row.and_then(|row| pane::input(state, id, row, feed)),
        terminal,
        reply: live_reply::reply(id, row, &shown, state, feed, env, via),
        ended: row.is_some_and(|row| row.run.ended.is_some()),
        run_details: Run {
            title: "Run",
            facts: task
                .as_deref()
                .map(|task| facts(row, board_task, task, feed))
                .unwrap_or_default(),
            steps_title: "Stages",
            steps,
        },
        fleet: super::fleet::fleet(state, id, feed),
    }
}

/// What a run without a terminal shows while corrald serves no transcripts.
fn unserved() -> Transcript {
    Transcript {
        messages: Vec::new(),
        older: None,
        newer: None,
        paging: false,
        fill: Fill::Empty {
            heading: "No terminal",
            paragraph: "corrald recorded no pane or process for this run, and it serves no session transcripts yet.".into(),
        },
        stale: None,
        disclaimer: None,
        live: Vec::new(),
        after: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use corral_protocol::detail::{DetailBody, DetailRef, TerminalSnapshot};
    use corral_types::Role;

    use super::super::roster_tests::agent;
    use super::super::{LineClass, State};
    use super::session;
    use crate::model::daemon::{self, DetailCache};
    use crate::model::health::daemon_env;
    use crate::model::{Form, Id};

    #[test]
    fn live_screen_rows_are_not_reclassed_or_trimmed() {
        let id = "T-1_R1";
        let rows = vec![
            "  ╭─ PASS · ok 12 tests ─╮".to_owned(),
            "Edit    src/lib.rs".to_owned(),
            String::new(),
            "  │ $ cargo test        │".to_owned(),
        ];
        let mut feed = daemon::Feed::default();
        feed.agents
            .insert(Id::from(id), agent("T-1", "R1", Role::Worker));
        let target = DetailRef::Terminal { id: id.into() };
        feed.details.insert(
            daemon::detail_key(&target),
            DetailCache {
                target,
                pages: Vec::new(),
                loading: false,
                stale: false,
                error: None,
                merged: Some(DetailBody::Terminal(TerminalSnapshot {
                    lines: rows.clone(),
                    captured_at: "2026-11-14T10:00:00Z".into(),
                    live: true,
                    rows: Vec::new(),
                })),
            },
        );
        let page = session(
            &State::default(),
            &Id::from(id),
            &feed,
            daemon_env(&feed, Form::Desktop),
            "app:test",
        );
        let Some(terminal) = page.terminal else {
            panic!("a run with a pane has a terminal");
        };
        let shown: Vec<String> = terminal
            .lines
            .iter()
            .map(|line| line.text.iter().map(|span| span.text.as_str()).collect())
            .collect();
        assert_eq!(shown, rows);
        assert!(
            terminal
                .lines
                .iter()
                .all(|line| line.class == LineClass::Plain)
        );
        assert!(terminal.verbatim);
        assert!(terminal.styled.is_empty());
        assert_eq!(page.controls.menu, Some(Id::from(id)));
    }
}
