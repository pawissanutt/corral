//! The session page's head: the way back, who the session is (`build · auth-refactor`,
//! `T-12 R2 · <task title>`), its second muted line, the needs-you and decisions chips, and the
//! state word with its mark. A build shows its exit status in the state word's place. A current
//! notice takes the state word's place for a few seconds. Every word comes from the host's
//! state, the session's row or the served page; what none of them says is not shown.

use corral_protocol::rows::{AssistantLocation, AssistantState};
use corral_protocol::session_state::{ExitBrief, ExitPhase};
use corral_types::SessionStatus;

use crate::model::Live;
use crate::model::daemon::{Group, Link};
use crate::model::hue::Hue;
use crate::model::status::status_unavailable;

use super::activity::{self, duration};
use super::notices::{self, Notice};
use super::{Action, Cx, Host, SessionRole};

/// The page head.
#[derive(Debug, Clone, PartialEq)]
pub struct Header {
    /// `‹ Sessions`.
    pub back: (&'static str, Action),
    /// `build · auth-refactor`, `assistant · Quick thread`, `T-12 R2 · Fix gate cache`: the
    /// window's title and the phone's head.
    pub target: String,
    /// The session's own id as the roster shows it (`B-4`, `A-2`, `T-12 R2`).
    pub shown_id: String,
    /// The session's name: `auth-refactor`, the task's title.
    pub name: String,
    /// `build`, `worker`, `chat`.
    pub role: String,
    /// The meta line: project, branch, task, model and effort, what it does; only the facts
    /// reported.
    pub meta: Vec<Meta>,
    pub state: StateWord,
    pub needs_you: Option<NeedsChip>,
    pub decisions: Option<DecisionsChip>,
    /// A notice in the state word's place, while it lasts.
    pub notice: Option<Notice>,
    /// The window or tab title while the page is on top: `▲ needs you · <target>`.
    pub title: String,
}

/// One fact of the meta line: `project corral`, `task T-12` (which opens the task's page).
#[derive(Debug, Clone, PartialEq)]
pub struct Meta {
    /// `project`; none for the words of what the session does.
    pub label: Option<&'static str>,
    pub value: String,
    /// A path, branch, id or model: drawn in the mono face.
    pub mono: bool,
    pub action: Option<Action>,
}

/// The role as the session lists say it: the assistant's sessions are chats.
pub fn role_word(role: &SessionRole) -> String {
    match role {
        SessionRole::Assistant => "chat".into(),
        role => role.label(),
    }
}

fn meta(cx: &Cx<'_>) -> Vec<Meta> {
    let fact = |label, value: String, mono| Meta {
        label: Some(label),
        value,
        mono,
        action: None,
    };
    let build = cx.build();
    let project = build.and_then(|build| build.project.clone()).or_else(|| {
        cx.task
            .and_then(|task| cx.feed.board.task(task))
            .and_then(|task| task.project().map(str::to_owned))
    });
    let branch = build
        .and_then(|build| build.branch.clone())
        .or_else(|| cx.run.map(|run| run.run.branch.clone()))
        .filter(|branch| !branch.is_empty());
    let mut meta: Vec<Meta> = project
        .map(|project| fact("project", project, false))
        .into_iter()
        .chain(branch.map(|branch| fact("branch", branch, true)))
        .collect();
    if let Some(task) = cx.task.filter(|_| cx.role.task_bound()) {
        meta.push(Meta {
            action: Some(Action::OpenLive(Live::Task(task.clone()))),
            ..fact("task", task.to_string(), true)
        });
    }
    if let Some(model) = cx.model() {
        let words = match cx.effort() {
            Some(effort) => format!("{model} · {effort}"),
            None => model,
        };
        meta.push(fact("model", words, true));
    }
    let doing = crate::model::agents::session_line(cx.feed, cx.id.as_str())
        .and_then(|line| line.doing)
        .filter(|doing| !doing.trim().is_empty());
    meta.extend(doing.map(|doing| Meta {
        label: None,
        value: doing,
        mono: false,
        action: None,
    }));
    meta
}

/// The state word and its mark.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateWord {
    /// Drawn with the running spinner.
    pub spinner: bool,
    /// `▲ ! ○ ✓ ✗` before the words, when not a spinner.
    pub glyph: Option<&'static str>,
    pub hue: Hue,
    /// `Working · 4m 12s`, `Idle`, `Not running`, `Unavailable`, `Ended`.
    pub words: String,
    /// A build's exit status, which these words then say.
    pub exit: Option<ExitStatus>,
}

/// A build's exit status as the header and the roster say it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitStatus {
    pub phase: ExitPhase,
    /// `choose exit`, `Working · full gate`, `Working · repair 1 of 3`, `Idle · landed`.
    pub words: String,
    /// The roster's short form: `choose exit`, `gate`, `repair 1/3`, `landed`.
    pub short: String,
    pub hue: Hue,
    /// The host's detail (a failure's text), muted after the words where room allows.
    pub detail: Option<String>,
}

/// `▲ 2 needs you`: the session and its children wait on the owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NeedsChip {
    pub count: u32,
    /// `▲ 2 needs you`.
    pub label: String,
    /// `▲ 2`.
    pub short: String,
}

/// `⚑ 1 decision`: the session's task waits on the owner in Now; pressing opens it there.
#[derive(Debug, Clone, PartialEq)]
pub struct DecisionsChip {
    pub label: String,
    pub action: Action,
}

/// A build's exit status, from the host's brief.
pub fn exit_status(exit: &ExitBrief) -> Option<ExitStatus> {
    let (words, short, hue) = match exit.phase {
        ExitPhase::None | ExitPhase::Other => return None,
        ExitPhase::Pending => ("choose exit".into(), "choose exit".into(), Hue::Held),
        ExitPhase::Rebasing => ("Working · rebase".into(), "rebase".into(), Hue::Corral),
        ExitPhase::Verifying => ("Working · full gate".into(), "gate".into(), Hue::Corral),
        ExitPhase::Repairing => match exit.repair {
            Some(repair) => (
                format!("Working · repair {} of {}", repair.round, repair.of),
                format!("repair {}/{}", repair.round, repair.of),
                Hue::Corral,
            ),
            None => ("Working · repair".into(), "repair".into(), Hue::Corral),
        },
        ExitPhase::Landing => ("Working · land".into(), "land".into(), Hue::Corral),
        ExitPhase::Kept => ("Idle · kept on branch".into(), "kept".into(), Hue::Neutral),
        ExitPhase::Landed => ("Idle · landed".into(), "landed".into(), Hue::Ok),
        ExitPhase::Failed => ("Idle · exit failed".into(), "exit failed".into(), Hue::Fail),
    };
    Some(ExitStatus {
        phase: exit.phase,
        words,
        short,
        hue,
        detail: exit.detail.clone(),
    })
}

fn word(
    spinner: bool,
    glyph: Option<&'static str>,
    hue: Hue,
    words: impl Into<String>,
) -> StateWord {
    StateWord {
        spinner,
        glyph,
        hue,
        words: words.into(),
        exit: None,
    }
}

/// How long the open turn has run, when the host stamped its start.
fn elapsed(cx: &Cx<'_>) -> Option<String> {
    let started = cx.live.and_then(|live| live.started_at.as_deref());
    let start = crate::model::agents::unix(started?)?;
    (cx.feed.now > 0 && start <= cx.feed.now).then(|| duration(cx.feed.now - start))
}

fn with_elapsed(words: &str, cx: &Cx<'_>) -> String {
    match elapsed(cx) {
        Some(time) => format!("{words} · {time}"),
        None => words.to_owned(),
    }
}

/// `Ended`, or `Ended · landed` / `Ended · kept` when the build's exit has that outcome.
fn ended(cx: &Cx<'_>) -> StateWord {
    match cx.exit().map(|exit| exit.phase) {
        Some(ExitPhase::Landed) => word(false, Some("✓"), Hue::Ok, "Ended · landed"),
        Some(ExitPhase::Kept) => word(false, None, Hue::Neutral, "Ended · kept"),
        _ => word(false, None, Hue::Neutral, "Ended"),
    }
}

/// The state word, in the order the screens design gives: an ended session, then what the link
/// and the host presence say, then a build's exit status, then the turn.
pub fn state_word(cx: &Cx<'_>) -> StateWord {
    match cx.host {
        Host::Ended => return ended(cx),
        _ if cx.feed.link != Link::Up => return word(false, Some("!"), Hue::Held, "Unavailable"),
        Host::Unreachable => return word(false, Some("!"), Hue::Held, "Unavailable"),
        Host::NotRunning if cx.elsewhere() => {
            return word(false, Some("○"), Hue::Neutral, "Open in another terminal");
        }
        Host::NotRunning => return word(false, Some("○"), Hue::Neutral, "Not running"),
        Host::Running | Host::Unknown => {}
    }
    if let Some(exit) = cx.exit().and_then(exit_status) {
        let spinner = exit.hue == Hue::Corral;
        let glyph = match exit.phase {
            ExitPhase::Pending => Some("!"),
            ExitPhase::Landed => Some("✓"),
            ExitPhase::Failed => Some("✗"),
            _ => None,
        };
        return StateWord {
            spinner,
            glyph,
            hue: exit.hue,
            words: exit.words.clone(),
            exit: Some(exit),
        };
    }
    if let Some(state) = cx.state {
        if state.paused {
            return word(false, Some("○"), Hue::Held, "Paused");
        }
        if state.open {
            let line = activity::line(cx.state, cx.live, cx.feed);
            return match line {
                Some(line) if !line.spinner => word(
                    false,
                    line.glyph,
                    line.hue,
                    with_elapsed(&capitalised(&line.phase), cx),
                ),
                _ => word(true, None, Hue::Worker, with_elapsed("Working", cx)),
            };
        }
        if !state.cards.is_empty() {
            return word(false, Some("▲"), Hue::You, "Waiting for you");
        }
        return word(false, None, Hue::Neutral, "Idle");
    }
    if let Some(run) = cx.run {
        return match &run.status {
            SessionStatus::Working => word(true, None, Hue::Worker, with_elapsed("Working", cx)),
            SessionStatus::Blocked { .. } => word(false, Some("▲"), Hue::You, "Waiting for you"),
            SessionStatus::Idle if run.paused == Some(true) => {
                word(false, Some("○"), Hue::Held, "Paused")
            }
            SessionStatus::Idle => word(false, None, Hue::Neutral, "Idle"),
            SessionStatus::Gone => word(false, Some("○"), Hue::Neutral, "Not running"),
        };
    }
    if let Some(row) = cx.row {
        if status_unavailable(row) {
            return word(false, None, Hue::Neutral, "Status unavailable");
        }
        return match row.state {
            AssistantState::Working | AssistantState::Starting => {
                word(true, None, Hue::Worker, with_elapsed("Working", cx))
            }
            AssistantState::Waiting => word(false, Some("▲"), Hue::You, "Waiting for you"),
            AssistantState::Paused => word(false, Some("○"), Hue::Held, "Paused"),
            AssistantState::Stopping => word(true, None, Hue::Neutral, "Stopping"),
            AssistantState::Failed => word(false, Some("✗"), Hue::Fail, "Failed"),
            AssistantState::Cold => word(false, Some("○"), Hue::Neutral, "Not running"),
            AssistantState::Ended => word(false, None, Hue::Neutral, "Ended"),
            AssistantState::Ready | AssistantState::Other => {
                word(false, None, Hue::Neutral, "Idle")
            }
        };
    }
    if cx.cache.is_none_or(|cache| cache.loading) {
        return word(false, None, Hue::Neutral, "reading…");
    }
    word(false, None, Hue::Neutral, "state not reported")
}

fn capitalised(text: &str) -> String {
    let mut letters = text.chars();
    match letters.next() {
        Some(first) => first.to_uppercase().chain(letters).collect(),
        None => String::new(),
    }
}

/// How many things wait on the owner: the host's open cards, plus children it counts as
/// waiting that no card of their own stands for yet. Without a host state, the row's word.
pub fn needs_you(cx: &Cx<'_>) -> u32 {
    if let Some(state) = cx.state {
        let cards = u32::try_from(state.cards.len()).unwrap_or(u32::MAX);
        let child_cards = u32::try_from(
            state
                .cards
                .iter()
                .filter(|card| card.child.is_some())
                .count(),
        )
        .unwrap_or(u32::MAX);
        return cards.saturating_add(state.children.waiting.saturating_sub(child_cards));
    }
    let waiting = cx
        .run
        .is_some_and(|run| matches!(run.status, SessionStatus::Blocked { .. }))
        || cx
            .row
            .is_some_and(|row| row.state == AssistantState::Waiting);
    u32::from(waiting)
}

fn target(cx: &Cx<'_>) -> String {
    let title = cx.title();
    match &cx.role {
        SessionRole::Build => format!("build · {title}"),
        SessionRole::Assistant => format!("assistant · {title}"),
        SessionRole::Reviewer => match (&cx.task, cx.run_id()) {
            (Some(task), Some(run)) => format!("{task} review {run} · {title}"),
            _ => format!("review · {title}"),
        },
        SessionRole::Child => format!("child · {}", cx.id),
        role if role.task_bound() => match (&cx.task, cx.run_id()) {
            (Some(task), Some(run)) => format!("{task} {run} · {title}"),
            _ => format!("{} · {title}", role.label()),
        },
        role => format!("{} · {title}", role.label()),
    }
}

fn decisions(cx: &Cx<'_>) -> Option<DecisionsChip> {
    if !cx.role.task_bound() {
        return None;
    }
    let task = cx.task?;
    let waiting = cx
        .feed
        .board
        .in_group(Group::NeedsYou)
        .any(|found| found.id == *task);
    waiting.then(|| DecisionsChip {
        label: "⚑ 1 decision".into(),
        action: Action::OpenLive(Live::Task(task.clone())),
    })
}

/// The page head of the session `cx` reads.
pub fn header(cx: &Cx<'_>) -> Header {
    let target = target(cx);
    let state = state_word(cx);
    let count = needs_you(cx);
    let needs_you = (count > 0).then(|| NeedsChip {
        count,
        label: format!("▲ {count} needs you"),
        short: format!("▲ {count}"),
    });
    let title = match (&needs_you, state.spinner) {
        (Some(_), _) => format!("▲ needs you · {target}"),
        (None, true) => format!("● working · {target}"),
        (None, false) => target.clone(),
    };
    Header {
        back: ("Sessions", Action::Back),
        shown_id: cx.shown_id(),
        name: cx.title(),
        role: role_word(cx.role),
        meta: meta(cx),
        notice: notices::current(&cx.ui.notices, cx.state, cx.feed.now),
        decisions: decisions(cx),
        needs_you,
        state,
        target,
        title,
    }
}

/// Whether the build's record says its host runs in a terminal this daemon cannot reach.
pub(super) fn open_elsewhere(row: &corral_protocol::rows::AssistantRow) -> bool {
    row.state == AssistantState::Cold && row.live_in == AssistantLocation::Pane
}
