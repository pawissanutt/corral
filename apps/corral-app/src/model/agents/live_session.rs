//! One agent session from corrald: its row on the wall, its bounded transcript page, the
//! recorded terminal capture where its pane has one, and replies through `task.reply`. A session
//! without a pane (a daemon-hosted assistant) has no terminal; none is drawn for it.

use corral_protocol::assistant::ToolLine;
use corral_protocol::detail::{
    DetailBody, DetailRef, SessionItem, SessionMessage, SessionPage, TerminalSnapshot,
};
use corral_protocol::rows::AgentRow;

use crate::model::board::{Mark, STAGES};
use crate::model::daemon::{self, DetailCache};
use crate::model::effect::Env;
use crate::model::hue::Hue;
use crate::model::text::{Rich, span};
use crate::model::{ActStatus, Id, act};

use super::live::{clock, role_label, shown_id, stale, usage};
use super::{
    Action, FactValue, Feed, Fill, Line, LineClass, Message, MessageItem, QuestionCard, Reply,
    ReplyMode, Run, RunFact, Session, State, Terminal, Tool, Transcript, View, line, reply_busy,
    tail_line,
};

/// Output lines an expanded tool call shows; the rest are counted, not drawn.
const TOOL_LINES: usize = 200;

/// The page token of the messages before the shown page, when the daemon has older ones.
pub(super) fn older_token(cache: &DetailCache) -> Option<String> {
    match cache.body()? {
        DetailBody::Session(page) => page.older.clone(),
        _ => None,
    }
}

fn page(cache: Option<&DetailCache>) -> Option<&SessionPage> {
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
fn failure(cache: Option<&DetailCache>) -> Option<String> {
    cache?
        .error
        .as_ref()
        .map(|error| format!("{} ({})", error.message, error.code))
}

/// The avatar letter and hue of a transcript author.
fn avatar(author: &str) -> (&'static str, Hue) {
    let who = author.to_ascii_lowercase();
    if who.starts_with("corral") || who == "system" {
        ("C", Hue::Corral)
    } else if who.starts_with("owner") || who == "you" {
        ("Y", Hue::You)
    } else if who == "pm" || who.starts_with("pm ") {
        ("P", Hue::Corral)
    } else if who.starts_with("assistant") {
        ("A", Hue::Worker)
    } else if who.starts_with("gate") {
        ("G", Hue::Corral)
    } else if who.starts_with("reviewer") {
        ("R", Hue::Worker)
    } else {
        ("W", Hue::Worker)
    }
}

/// A finished, failed or still-running tool call; its output shows only when expanded.
fn tool(tool: &ToolLine, key: String, state: &State) -> Tool {
    let (result, mark) = match (tool.failed, &tool.output) {
        (Some(true), _) => ("failed".to_owned(), Mark::Fail),
        (None, None) => ("running".to_owned(), Mark::Active),
        _ => (
            match tool.elapsed_ms {
                Some(ms) => format!("ok {:.1}s", ms as f64 / 1000.0),
                None => "ok".to_owned(),
            },
            Mark::Done,
        ),
    };
    let open = state.expanded.contains(&key);
    let output = match (&tool.output, open) {
        (Some(text), true) => {
            let lines: Vec<&str> = text.lines().collect();
            let cut = lines.len().saturating_sub(TOOL_LINES);
            let mut shown: Vec<Line> = Vec::with_capacity(lines.len() - cut + 1);
            if cut > 0 {
                shown.push(line(&format!("… {cut} earlier lines"), LineClass::Dim));
            }
            shown.extend(lines[cut..].iter().map(|text| tail_line(text)));
            shown
        }
        _ => Vec::new(),
    };
    Tool {
        kind: tool.tool.clone(),
        argument: tool.input.lines().next().unwrap_or_default().to_owned(),
        result,
        mark,
        output,
        expanded: tool.output.as_ref().map(|_| open),
        action: Action::Expand(key),
    }
}

/// The inline handover for a question the agent asked, open or settled.
fn question(
    key: &str,
    text: &str,
    options: &[corral_protocol::detail::QuestionOption],
    answer: Option<&corral_protocol::detail::QuestionAnswer>,
    feed: &daemon::Feed,
) -> QuestionCard {
    let recommended = options
        .iter()
        .position(|option| option.recommended)
        .map(|at| format!(" · recommends {}", at + 1))
        .unwrap_or_default();
    let count = match options.len() {
        0 => "free answer".to_owned(),
        1 => "1 option".to_owned(),
        n => format!("{n} options"),
    };
    match answer {
        None => QuestionCard {
            label: format!("{key} · waiting on you"),
            question: text.to_owned(),
            button: "Open handover · decide",
            hint: format!("{count}{recommended}"),
            open: true,
            action: Action::OpenQuestion(Id::from(key)),
        },
        Some(answer) => QuestionCard {
            label: format!("{key} · answered by {}", answer.by),
            question: text.to_owned(),
            button: "Open handover",
            hint: [
                clock(Some(answer.at.as_str()), feed),
                Some(answer.option.clone().unwrap_or_else(|| answer.text.clone())),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · "),
            open: false,
            action: Action::OpenQuestion(Id::from(key)),
        },
    }
}

fn message(message: &SessionMessage, state: &State, feed: &daemon::Feed) -> Message {
    let (letter, hue) = avatar(&message.author);
    let author = if letter == "Y" {
        "you".to_owned()
    } else {
        message.author.clone()
    };
    let at = clock(message.at.as_deref(), feed);
    let mut head: Rich = vec![span(author.clone()).bold()];
    if let Some(at) = &at {
        head.push(span(format!(" · {at}")).dim());
    }
    let stamp = message.at.clone().unwrap_or_default();
    let mut items = Vec::new();
    for (n, item) in message.items.iter().enumerate() {
        match item {
            SessionItem::Text { text } => items.extend(
                text.split("\n\n")
                    .map(str::trim)
                    .filter(|part| !part.is_empty())
                    .map(|part| MessageItem::Paragraph(vec![span(part)])),
            ),
            SessionItem::Tool { tool: call } => items.push(MessageItem::Tool(tool(
                call,
                format!("{stamp}|{}|{n}", message.author),
                state,
            ))),
            SessionItem::QuestionCard {
                key,
                text,
                options,
                answer,
                ..
            } => items.push(MessageItem::Question(question(
                key,
                text,
                options,
                answer.as_ref(),
                feed,
            ))),
            SessionItem::Other => items.push(MessageItem::Paragraph(vec![
                span("An item this version of the app cannot show.").dim(),
            ])),
        }
    }
    Message {
        avatar: letter,
        hue,
        head,
        author,
        detail: at.unwrap_or_default(),
        items,
    }
}

fn transcript(
    id: &Id,
    state: &State,
    cache: Option<&DetailCache>,
    feed: &daemon::Feed,
) -> Transcript {
    let body = page(cache);
    let loading = cache.is_none_or(|cache| cache.loading);
    let earlier = state.earlier.contains(id.as_str());
    let fill = match body {
        Some(page) if page.messages.is_empty() => Fill::Empty {
            heading: "No transcript yet",
            paragraph: "corrald has recorded no messages for this session.".into(),
        },
        Some(_) => Fill::Ready,
        None => match failure(cache) {
            Some(why) if !loading => Fill::Failed(format!(
                "The transcript could not be loaded: {why}. It loads again when the session changes or corrald reconnects."
            )),
            _ => Fill::Loading,
        },
    };
    let stale_note = body.and(cache).and_then(|cache| {
        if let Some(why) = failure(Some(cache)) {
            Some(format!(
                "Showing the last transcript received; refreshing failed: {why}."
            ))
        } else {
            (cache.stale && !cache.loading).then(|| {
                format!(
                    "Showing the transcript as of {}; corrald is not answering.",
                    feed.synced.as_deref().unwrap_or("the last update")
                )
            })
        }
    });
    Transcript {
        messages: body
            .map(|page| {
                page.messages
                    .iter()
                    .map(|m| message(m, state, feed))
                    .collect()
            })
            .unwrap_or_default(),
        older: cache
            .and_then(older_token)
            .map(|_| Action::Older(id.clone())),
        newer: earlier.then(|| Action::Newer(id.clone())),
        paging: body.is_some() && loading,
        fill,
        stale: stale_note,
        disclaimer: None,
    }
}

/// The recorded capture of the session's pane. It is never a stream and takes no keys.
fn terminal(
    row: &AgentRow,
    pane: &str,
    cache: Option<&DetailCache>,
    feed: &daemon::Feed,
) -> Terminal {
    let capture = snapshot(cache);
    let loading = cache.is_none_or(|cache| cache.loading);
    let status = match capture {
        Some(capture) => format!(
            "snapshot · captured {} · read-only",
            clock(Some(capture.captured_at.as_str()), feed)
                .unwrap_or_else(|| "at an unknown time".into())
        ),
        None if loading => "loading the latest capture".to_owned(),
        None => "no capture".to_owned(),
    };
    let empty = match capture {
        Some(capture) if capture.lines.is_empty() => Some("The capture is empty.".to_owned()),
        Some(_) => None,
        None if loading => Some("Loading the latest capture…".to_owned()),
        None => Some(match cache.and_then(|cache| cache.error.as_ref()) {
            Some(error) if matches!(error.code.as_str(), "not_found" | "unavailable") => {
                "corrald has no recorded capture of this pane. Captures are recorded, not read live."
                    .to_owned()
            }
            Some(error) => format!("The capture could not be loaded: {} ({}).", error.message, error.code),
            None => "corrald has no recorded capture of this pane.".to_owned(),
        }),
    };
    Terminal {
        feed: Feed::Snapshot,
        heading: format!(
            "{} · {pane}",
            row.runtime.as_deref().unwrap_or("runtime unknown")
        ),
        status,
        input: "read-only · keys are not sent".into(),
        lines: capture
            .map(|capture| capture.lines.iter().map(|text| tail_line(text)).collect())
            .unwrap_or_default(),
        cursor: false,
        empty,
        phone_keys: Vec::new(),
        refresh: Some(Action::Refresh(DetailRef::Terminal { id: row.id.clone() })),
    }
}

/// The reply box: open for a replyable session with a task, otherwise closed with the reason
/// and, for an assistant, the thread it answers in.
#[allow(clippy::too_many_arguments)]
fn reply(
    id: &Id,
    role: &str,
    shown: &str,
    body: Option<&SessionPage>,
    open_question: bool,
    state: &State,
    feed: &daemon::Feed,
    via: &str,
) -> Reply {
    let task = body.and_then(|page| page.task.clone());
    let mode = match body {
        None => ReplyMode::Closed {
            reason: "Replies open once the transcript has loaded.".into(),
            action: None,
        },
        Some(page) if page.replyable && task.is_some() => {
            if reply_busy(state, id, feed) {
                ReplyMode::Busy
            } else {
                ReplyMode::Open
            }
        }
        Some(page) if role == "assistant" => ReplyMode::Closed {
            reason: page
                .reply_refusal
                .clone()
                .unwrap_or_else(|| "This assistant answers in its thread.".into()),
            action: Some(("Open thread", Action::OpenThread(id.clone()))),
        },
        Some(page) => ReplyMode::Closed {
            reason: page.reply_refusal.clone().unwrap_or_else(|| match role {
                "reviewer" | "plan_reviewer" => {
                    "A reviewer takes no replies; replies go to the task's worker.".into()
                }
                _ => "corrald does not take replies for this session now.".into(),
            }),
            action: None,
        },
    };
    let key = state.replies.get(id.as_str());
    let outcome = key.and_then(|key| act::status(feed, key));
    let status = outcome.as_ref().map(|status| match status {
        ActStatus::Done => (
            Hue::Ok,
            format!("Delivered to {}", task.as_deref().unwrap_or(shown)),
        ),
        ActStatus::Failed(_) | ActStatus::Refused(_) | ActStatus::Conflict(_) => {
            (Hue::Fail, status.line())
        }
        ActStatus::Unknown => (
            Hue::Held,
            "Delivery unknown · it may have arrived; check before sending again".into(),
        ),
        ActStatus::Abandoned => (Hue::Held, status.line()),
        _ => (Hue::Neutral, status.line()),
    });
    Reply {
        placeholder: format!("Message {} {shown}…", role_label(role)),
        accessibility_label: "Reply to agent",
        button: "Send",
        hint: vec![
            span("Arrives in "),
            span(task.clone().unwrap_or_else(|| shown.to_owned())).mono(),
            span(" as owner input · "),
            span(via.to_owned()).mono(),
        ],
        question_hint: if open_question {
            "Answering the open question happens on its handover sheet".into()
        } else {
            String::new()
        },
        mode,
        status,
        recover: matches!(outcome, Some(ActStatus::Unknown))
            .then(|| key.map(|key| Action::Recover(key.clone())))
            .flatten(),
        delivered: matches!(outcome, Some(ActStatus::Done))
            .then(|| key.map(ToString::to_string))
            .flatten(),
    }
}

fn fact(label: &'static str, value: Rich) -> RunFact {
    RunFact {
        label,
        value: FactValue::Text(value),
    }
}

/// Build the session page for a daemon session id. `via` is this device's authenticated
/// attribution, as replies carry it.
pub fn session(state: &State, id: &Id, feed: &daemon::Feed, _env: Env, via: &str) -> Session {
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
        fact(
            "Agent",
            vec![span(profile.unwrap_or_else(|| "profile unknown".into()))],
        ),
        fact(
            "Runtime",
            vec![
                span(format!(
                    "{} · ",
                    row.and_then(|row| row.runtime.clone())
                        .unwrap_or_else(|| "runtime unknown".into())
                )),
                span(pane.clone().unwrap_or_else(|| "no pane".into())).mono(),
            ],
        ),
        fact(
            "Machine",
            vec![span(machine.unwrap_or_else(|| "unknown".into()))],
        ),
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
        state.view
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
        back_label: "Back",
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
        transcript: transcript(id, state, session_cache, feed),
        terminal,
        reply: reply(id, &role, &shown, body, open_question, state, feed, via),
        run_details: Run {
            title: "Run",
            facts,
            steps_title: "Stages",
            steps,
        },
    }
}
