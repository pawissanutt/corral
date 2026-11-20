//! Every session the daemon addresses, of every role: corrald's live runs, the assistant's
//! chats and the build sessions, one source each, in the project scope. Needs you first, then
//! working, idle, not running and ended; newest activity first within each, the pinned quick
//! chat first among its equals. `lines` turns each into a Sessions line.

use corral_protocol::rows::{
    AgentRow, AssistantKind, AssistantLocation, AssistantRow, AssistantState,
};
use corral_types::SessionStatus;

use crate::model::daemon;
use crate::model::hue::Hue;
use crate::model::projects::ProjectFilter;
use crate::model::session::exit_status;
use crate::model::{assistant, status::SessionState};

use super::live;

/// A session's source: a live run (keyed `<task>_<run>`), or a chat or build row.
#[derive(Clone, Copy)]
pub(super) struct Source<'a> {
    pub id: &'a str,
    pub run: Option<&'a AgentRow>,
    pub chat: Option<&'a AssistantRow>,
}

impl<'a> Source<'a> {
    pub fn role(self) -> &'a str {
        match (self.run, self.chat) {
            (Some(run), _) => run.run.role.as_str(),
            (None, Some(chat)) if chat.kind == AssistantKind::Build => "build",
            _ => "assistant",
        }
    }
    fn task(self, feed: &'a daemon::Feed) -> Option<&'a daemon::Task> {
        self.run.and_then(|row| feed.board.task(&row.run.task))
    }
    /// The project the session works in: a run's task's, a build's own; none for a chat.
    pub fn project(self, feed: &'a daemon::Feed) -> Option<&'a str> {
        match (self.run, self.chat) {
            (Some(_), _) => self.task(feed).and_then(|task| task.project()),
            (None, Some(chat)) => chat
                .build
                .as_ref()
                .and_then(|build| build.project.as_deref()),
            (None, None) => None,
        }
    }
    fn pinned(self, feed: &daemon::Feed) -> bool {
        self.chat
            .is_some_and(|chat| chat.kind == AssistantKind::Quick)
            && assistant::quick_id(feed).as_deref() == Some(self.id)
    }
    /// The build's record says its host runs in a terminal this daemon cannot reach.
    fn elsewhere(self) -> bool {
        self.chat.is_some_and(|chat| {
            chat.state == AssistantState::Cold && chat.live_in == AssistantLocation::Pane
        })
    }
    /// A build whose host is stopped here: its worktree, branch and record can be removed.
    pub(super) fn removable(self) -> bool {
        self.chat.is_some_and(|chat| {
            chat.kind == AssistantKind::Build
                && matches!(chat.state, AssistantState::Cold | AssistantState::Ended)
        }) && !self.elsewhere()
    }
    /// Where the session stands: waiting on the owner, working, idle, or its host gone (not
    /// running or ended), in the order the list groups them.
    pub fn state(self) -> SessionState {
        if let Some(run) = self.run {
            return match &run.status {
                SessionStatus::Working => SessionState::Working,
                SessionStatus::Blocked { .. } => SessionState::Blocked,
                SessionStatus::Idle => SessionState::Idle,
                SessionStatus::Gone => SessionState::Gone,
            };
        }
        let Some(chat) = self.chat else {
            return SessionState::Idle;
        };
        let exit = chat
            .build
            .as_ref()
            .and_then(|build| build.exit.as_ref())
            .and_then(exit_status);
        if let Some(exit) = exit.filter(|_| chat.state != AssistantState::Cold) {
            return match exit.hue {
                Hue::Held => SessionState::Blocked,
                Hue::Corral => SessionState::Working,
                _ => SessionState::Idle,
            };
        }
        match chat.state {
            AssistantState::Working | AssistantState::Starting | AssistantState::Stopping => {
                SessionState::Working
            }
            AssistantState::Waiting => SessionState::Blocked,
            AssistantState::Cold | AssistantState::Ended => SessionState::Gone,
            AssistantState::Paused
            | AssistantState::Failed
            | AssistantState::Ready
            | AssistantState::Other => SessionState::Idle,
        }
    }
    /// The order within the gone: a host not running before an ended one.
    fn ended(self) -> bool {
        self.chat
            .is_some_and(|chat| chat.state == AssistantState::Ended)
    }
    pub fn name(self, feed: &daemon::Feed) -> Option<String> {
        let named = match (self.run, self.chat) {
            (Some(_), _) => self.task(feed).map(|task| task.title().to_owned()),
            (None, Some(chat)) => Some(
                chat.build
                    .as_ref()
                    .map_or_else(|| chat.title.clone(), |build| build.name.clone()),
            ),
            (None, None) => None,
        };
        named.filter(|name| !name.trim().is_empty())
    }
    /// What the session is doing: a blocked screen's prompt, the task's progress while the
    /// run works; a chat's or build's waiting question, or how many agents it runs.
    pub fn doing(self, feed: &daemon::Feed) -> Option<String> {
        if let Some(row) = self.run {
            let task = self.task(feed);
            let said = match &row.status {
                SessionStatus::Blocked { .. } => live::excerpt(row)
                    .map(str::to_owned)
                    .or_else(|| task.map(daemon::Task::now)),
                SessionStatus::Working => task.map(daemon::Task::now),
                SessionStatus::Idle if row.paused == Some(true) => {
                    Some("paused · its input waits for Resume".into())
                }
                SessionStatus::Idle => task
                    .and_then(daemon::Task::blocked)
                    .map(|b| b.reason.clone()),
                SessionStatus::Gone => None,
            };
            return said.filter(|text| !text.trim().is_empty());
        }
        let chat = self.chat?;
        if let Some(wait) = chat
            .wait
            .as_ref()
            .filter(|_| chat.state == AssistantState::Waiting)
        {
            return wait.text.lines().next().map(str::to_owned);
        }
        let live = chat.build.as_ref().map_or(0, |build| build.children.live);
        (live > 0).then(|| {
            format!(
                "{live} {} running",
                if live == 1 { "agent" } else { "agents" }
            )
        })
    }
    /// The model and effort: the row's model; a chat without one names its profile.
    pub fn agent(self) -> Option<String> {
        let (model, effort) = match (self.run, self.chat) {
            (Some(run), _) => (run.run.model.clone(), run.run.effort.clone()),
            (None, Some(chat)) => (
                chat.model.clone().or_else(|| chat.profile.clone()),
                chat.effort.clone(),
            ),
            (None, None) => (None, None),
        };
        let model = model?;
        Some(match effort {
            Some(effort) => format!("{model} · {effort}"),
            None => model,
        })
    }
    /// Unix seconds of its newest activity, for the order within a group.
    fn activity(self) -> u64 {
        match (self.run, self.chat) {
            (Some(run), _) => run
                .run
                .quiet_since
                .unwrap_or(run.run.started)
                .as_second()
                .try_into()
                .unwrap_or(0),
            (None, Some(chat)) => chat
                .updated
                .as_deref()
                .or(chat.created.as_deref())
                .and_then(live::unix)
                .unwrap_or(0),
            (None, None) => 0,
        }
    }
}

fn in_scope(source: Source<'_>, feed: &daemon::Feed, filter: &ProjectFilter) -> bool {
    match (source.run, source.chat) {
        (Some(row), _) => feed
            .board
            .task(&row.run.task)
            .map_or(*filter == ProjectFilter::All, |task| {
                filter.matches(task.project())
            }),
        (None, Some(chat)) if chat.kind == AssistantKind::Build => {
            *filter == ProjectFilter::All || filter.matches(source.project(feed))
        }
        // Chats belong to the workspace, not to a project.
        _ => true,
    }
}

fn sources<'a>(feed: &'a daemon::Feed) -> impl Iterator<Item = Source<'a>> {
    feed.agents
        .iter()
        .map(|(id, row)| Source {
            id: id.as_str(),
            run: Some(row),
            chat: None,
        })
        .chain(feed.assistants.iter().map(|(id, row)| Source {
            id: id.as_str(),
            run: None,
            chat: Some(row),
        }))
}

/// The sessions the project scope keeps, in list order.
pub(super) fn scoped<'a>(feed: &'a daemon::Feed, filter: &ProjectFilter) -> Vec<Source<'a>> {
    let filter = filter.resolved(feed);
    let mut rows: Vec<_> = sources(feed)
        .filter(|source| in_scope(*source, feed, &filter))
        .collect();
    let order = |state: SessionState| SessionState::ORDER.iter().position(|s| *s == state);
    rows.sort_by(|a, b| {
        order(a.state())
            .cmp(&order(b.state()))
            .then_with(|| a.ended().cmp(&b.ended()))
            .then_with(|| b.pinned(feed).cmp(&a.pinned(feed)))
            .then_with(|| b.activity().cmp(&a.activity()))
            .then_with(|| a.id.cmp(b.id))
    });
    rows
}
