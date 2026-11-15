//! One agent session from corrald: its row on the wall, its bounded transcript page, its pane's
//! screen (read live where corrald serves it, else a recorded capture such as a gate's log), its
//! action bar (`controls`) and its reply box (`live_reply`) or, for an assistant, the thread its
//! composer chats in. A session without a pane (a daemon-hosted assistant) has no terminal; none
//! is drawn for it.

use corral_protocol::detail::{DetailBody, DetailRef, SessionItem, SessionPage, TerminalSnapshot};
use corral_protocol::rows::AgentRow;

use crate::model::board::STAGES;
use crate::model::daemon::{self, DetailCache, Link};
use crate::model::effect::Env;
use crate::model::hue::Hue;
use crate::model::text::{Rich, Span, span};
use crate::model::{ActStatus, Id};

use super::live::{clock, role_label, shown_id, stale, unix, usage};
use super::replay::transcript;
use super::{
    Action, FactValue, Feed, Keys, LineClass, Run, RunFact, Session, State, Terminal, View,
    controls, keys, line, live_reply, screen, tail_line,
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

/// The session pane's screen: read live where corrald serves it, or a recorded capture (a
/// gate's log). Live rows stay as read; only a current read is `Live`. Keys go to the pane only
/// through `keys`, the bar under it, while corrald offers them.
fn terminal(
    row: &AgentRow,
    pane: &str,
    cache: Option<&DetailCache>,
    feed: &daemon::Feed,
    keys: Option<Keys>,
) -> Terminal {
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
    let writable = matches!(&keys, Some(Keys::Open(bar)) if bar.enabled);
    let status = match capture {
        Some(capture) if capture.live => {
            let read = read_when(read_at(cache, capture), feed);
            match (error, unavailable) {
                _ if current => format!("live terminal · read {read}"),
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
            "snapshot · captured {} · read-only",
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
            _ if gone => format!("The pane is gone: herdr no longer has {pane}."),
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
        heading: format!(
            "{} · {pane}",
            row.runtime.as_deref().unwrap_or("runtime unknown")
        ),
        status,
        input: if writable {
            "keys go to the pane from the bar below"
        } else {
            "keys are not sent"
        }
        .into(),
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
        keys,
        refresh: Some(Action::Refresh(DetailRef::Terminal { id: row.id.clone() })),
    }
}

fn fact(label: &'static str, value: Rich) -> RunFact {
    RunFact {
        label,
        value: FactValue::Text(value),
    }
}

/// A profile, runtime, pane or machine name: an identifier (mono, cut rather than broken where
/// room is short) when known, else the plain words saying it is not.
fn identifier(name: Option<String>, unknown: &str) -> Span {
    match name {
        Some(name) => span(name).mono(),
        None => span(unknown),
    }
}

/// The thread scope an assistant row chats in: the quick thread's, or its own `A-n`.
pub fn chat_scope(feed: &daemon::Feed, row: &AgentRow) -> Id {
    let quick = feed
        .assistants
        .get(row.id.as_str())
        .is_some_and(|assistant| assistant.kind == corral_protocol::rows::AssistantKind::Quick);
    if quick {
        Id::from(crate::model::assistant::QUICK)
    } else {
        Id::from(row.id.as_str())
    }
}

/// The key of the session's latest `assistant.session` control operation, whose reply the
/// screen reads for [`session`]'s `op`.
pub fn op_key(state: &State, id: &Id) -> Option<Id> {
    state
        .sent
        .get(id.as_str())
        .filter(|sent| sent.channel == controls::Channel::Op)
        .map(|sent| sent.key.clone())
}

/// Build the session page for a daemon session id. `via` is this device's authenticated
/// attribution, as replies carry it; `op` is the status of the session's latest
/// `assistant.session` control operation ([`op_key`]).
pub fn session(
    state: &State,
    id: &Id,
    feed: &daemon::Feed,
    env: Env,
    via: &str,
    op: Option<&ActStatus>,
) -> Session {
    let row = feed.agents.get(id.as_str());
    let session_cache = feed.detail(&DetailRef::Session { id: id.to_string() });
    let body = page(session_cache);
    let role = row
        .map(|row| row.role.clone())
        .or_else(|| body.map(|page| page.role.clone()))
        .unwrap_or_else(|| "agent".into());
    let task = row
        .and_then(|row| row.task.clone())
        .or_else(|| body.and_then(|page| page.task.clone()));
    let run = row
        .and_then(|row| row.run.clone())
        .or_else(|| body.and_then(|page| page.run.clone()));
    let board_task = task.as_deref().and_then(|task| feed.board.task(task));
    let shown = row
        .map(shown_id)
        .or_else(|| task.clone())
        .unwrap_or_else(|| id.to_string());
    let title = row
        .map(|row| row.title.clone())
        .or_else(|| board_task.map(|task| task.title.clone()))
        .unwrap_or_else(|| role_label(&role));
    let pane = row.and_then(|row| row.pane.clone());
    let terminal = row.zip(pane.as_deref()).map(|(row, pane)| {
        terminal(
            row,
            pane,
            feed.detail(&DetailRef::Terminal { id: id.to_string() }),
            feed,
            keys::keys(state, id, row, feed, env),
        )
    });
    let profile = row.and_then(|row| row.profile.clone());
    let mut sub = vec![
        [Some(role_label(&role)), run.clone()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" "),
    ];
    sub.extend(profile.clone());
    if terminal.is_none() {
        sub.push("no terminal".into());
    }
    let open_question = body.is_some_and(|page| {
        page.messages
            .iter()
            .flat_map(|m| &m.items)
            .any(|item| matches!(item, SessionItem::QuestionCard { answer: None, .. }))
    });
    let notice = match (row, body) {
        (None, Some(_)) => Some((
            Hue::Neutral,
            "This session is no longer on the wall; this is its last recorded transcript.".into(),
        )),
        (None, None) if !stale(feed, "agents") && feed.topics.contains_key("agents") => {
            Some((Hue::Held, format!("corrald lists no session {id} now.")))
        }
        _ => None,
    };
    let since = row.and_then(|row| clock(row.since.as_deref(), feed));
    let machine = row
        .and_then(|row| row.machine.clone())
        .or_else(|| body.and_then(|page| page.machine.clone()));
    let mut facts = Vec::new();
    if let Some(task) = &task {
        let mut value = vec![span(task.clone()).mono()];
        if let Some(found) = board_task {
            value.extend([span(" "), span(found.title.clone())]);
        }
        facts.push(fact("Task", value));
    }
    if let Some(found) = board_task {
        facts.push(RunFact {
            label: "Rail",
            value: FactValue::Rail(std::array::from_fn(|at| found.rail[at].mark)),
        });
    }
    facts.extend([
        fact("Agent", vec![identifier(profile, "profile unknown")]),
        fact(
            "Runtime",
            vec![
                identifier(row.and_then(|row| row.runtime.clone()), "runtime unknown"),
                span(" · "),
                identifier(pane.clone(), "no pane"),
            ],
        ),
        fact("Machine", vec![identifier(machine, "unknown")]),
        fact(
            "Started",
            vec![span(since.unwrap_or_else(|| "unknown".into()))],
        ),
        fact(
            "Usage",
            vec![span(
                row.and_then(|row| usage(row.cost_usd, row.cost_basis, row.tokens))
                    .unwrap_or_else(|| "not reported".into()),
            )],
        ),
    ]);
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
    let view = if terminal.is_some() {
        state.live_view
    } else {
        View::Transcript
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
        switch_labels: ["Transcript", "Terminal"],
        switch_actions: [
            Action::SetView(View::Transcript),
            Action::SetView(View::Terminal),
        ],
        toggle: Action::ToggleView,
        notice,
        controls: controls::live_bar(state, id, row, feed, env, op),
        transcript: transcript(id, state, session_cache, feed),
        terminal,
        reply: live_reply::reply(id, row, &role, &shown, open_question, state, feed, env, via),
        chat: row
            .filter(|row| row.role == "assistant" && row.controls.send.available)
            .map(|row| chat_scope(feed, row)),
        run_details: Run {
            title: "Run",
            facts,
            steps_title: "Stages",
            steps,
        },
    }
}

#[cfg(test)]
mod tests {
    use corral_protocol::detail::{DetailBody, DetailRef, TerminalSnapshot};
    use corral_protocol::rows::{AgentRow, AgentStatus};

    use super::super::{LineClass, State};
    use super::session;
    use crate::model::daemon::{self, DetailCache};
    use crate::model::health::daemon_env;
    use crate::model::{Form, Id};

    #[test]
    fn live_screen_rows_are_not_reclassed_or_trimmed() {
        let id = "T-1/run/R1";
        let rows = vec![
            "  ╭─ PASS · ok 12 tests ─╮".to_owned(),
            "Edit    src/lib.rs".to_owned(),
            String::new(),
            "  │ $ cargo test        │".to_owned(),
        ];
        let mut feed = daemon::Feed::default();
        feed.agents.insert(
            Id::from(id),
            AgentRow {
                id: id.into(),
                role: "worker".into(),
                task: Some("T-1".into()),
                run: Some("R1".into()),
                title: "Live screen".into(),
                profile: None,
                runtime: Some("omp".into()),
                machine: None,
                pane: Some("w1:p1".into()),
                status: AgentStatus::Working,
                now: String::new(),
                since: None,
                tail: Vec::new(),
                cost_usd: None,
                cost_basis: None,
                tokens: None,
                rev: 1,
                generation: None,
                turn: None,
                controls: Default::default(),
            },
        );
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
            None,
        );
        let terminal = page.terminal.expect("a session with a pane has a terminal");
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
    }
}
