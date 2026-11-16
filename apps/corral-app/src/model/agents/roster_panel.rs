//! The selected roster session's story; board stages never masquerade as its own steps.
use corral_protocol::detail::{DetailRef, SessionItem, SessionPage};
use corral_protocol::rows::AgentStepState;

use crate::model::board::Mark;
use crate::model::daemon;
use crate::model::effect::Env;
use crate::model::glance::Activity;
use crate::model::projects::ProjectFilter;
use crate::model::{Hue, Id, assistant, inbox};

use super::roster::{self, Source};
use super::{Action, Reply, ReplyMode, State, Terminal, live, live_reply, live_session};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RosterPanelTab {
    #[default]
    Summary,
    Terminal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RosterPanelTabItem {
    pub id: &'static str,
    pub label: &'static str,
    pub selected: bool,
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RosterWait {
    pub heading: &'static str,
    pub text: String,
    pub label: &'static str,
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Steps {
    pub title: &'static str,
    pub count: String,
    pub items: Vec<(Mark, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecentState {
    Ready,
    Loading(&'static str),
    Unavailable(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentEntry {
    pub at: String,
    pub text: String,
    pub hint: String,
    pub tool: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recent {
    pub title: &'static str,
    pub order: &'static str,
    pub state: RecentState,
    pub entries: Vec<RecentEntry>,
    pub empty: &'static str,
    pub stale: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RosterPanel {
    pub session: Id,
    pub chip: String,
    pub hue: Hue,
    pub role: String,
    pub shown_id: String,
    pub project: String,
    pub name: String,
    pub facts: String,
    pub close_label: String,
    pub close: Action,
    pub tabs: Vec<RosterPanelTabItem>,
    pub tab: RosterPanelTab,
    pub open_label: &'static str,
    pub open: Action,
    pub waits: Option<RosterWait>,
    pub doing_title: &'static str,
    pub doing: String,
    pub last_output: String,
    pub activity: Option<Activity>,
    pub activity_axis: [String; 3],
    pub unknown_activity: &'static str,
    pub steps: Option<Steps>,
    pub recent: Recent,
    pub terminal: Option<Terminal>,
    /// Keep these details loaded while this panel is visible.
    pub wants: Vec<DetailRef>,
    /// Dispatch all panel controls with `State::act_roster`; their target belongs to
    /// the roster selection rather than the full-session route.
    pub reply: Reply,
    /// Assistant messages use the existing shared thread composer, including the quick scope.
    pub chat: Option<Id>,
}

fn steps(source: Source<'_>) -> Option<Steps> {
    let steps = source
        .row?
        .steps
        .as_ref()
        .filter(|steps| !steps.is_empty())?;
    Some(Steps {
        title: "Steps",
        count: format!(
            "{}/{}",
            steps
                .iter()
                .filter(|step| step.state == AgentStepState::Done)
                .count(),
            steps.len()
        ),
        items: steps
            .iter()
            .map(|step| {
                (
                    match step.state {
                        AgentStepState::Done => Mark::Done,
                        AgentStepState::InProgress => Mark::Active,
                        AgentStepState::Pending => Mark::Next,
                        AgentStepState::Dropped | AgentStepState::Other => Mark::Na,
                    },
                    if step.state == AgentStepState::Other {
                        format!("{} · state unknown", step.title)
                    } else {
                        step.title.clone()
                    },
                )
            })
            .collect(),
    })
}

fn recent_entries(page: &SessionPage, role: &str, feed: &daemon::Feed) -> Vec<RecentEntry> {
    let mut entries = Vec::new();
    for message in page.messages.iter().rev() {
        let at = live::clock(message.at.as_deref(), feed).unwrap_or_else(|| "—".into());
        let author = message.author.as_str();
        let agent_words = author == role
            || matches!(
                author,
                "assistant" | "worker" | "pm" | "planner" | "reviewer" | "plan_reviewer"
            );
        for item in message.items.iter().rev() {
            let (text, hint, tool) = match item {
                SessionItem::Text { text } if agent_words && !text.trim().is_empty() => {
                    (roster::short(text, 220), text.clone(), false)
                }
                SessionItem::ToolCall {
                    name,
                    arguments,
                    summary,
                    ..
                } => {
                    let target = super::replay_tool::target(arguments);
                    let target = if target.is_empty() {
                        summary.clone()
                    } else {
                        target
                    };
                    let text = if target.is_empty() {
                        format!("1 {}", name.to_lowercase())
                    } else {
                        format!(
                            "1 {} · {}",
                            name.to_lowercase(),
                            roster::short(&target, 100)
                        )
                    };
                    (text, format!("{name} · {target}"), true)
                }
                SessionItem::BashExecution { command, .. } => (
                    format!("1 command · {}", roster::short(command, 100)),
                    command.clone(),
                    true,
                ),
                SessionItem::QuestionCard { text, .. } => (
                    format!("Asked: {}", roster::short(text, 200)),
                    text.clone(),
                    false,
                ),
                _ => continue,
            };
            entries.push(RecentEntry {
                at: at.clone(),
                text,
                hint,
                tool,
            });
            if entries.len() == 6 {
                return entries;
            }
        }
    }
    entries
}

fn recent(feed: &daemon::Feed, target: &DetailRef, role: &str) -> Recent {
    let cache = feed.detail(target);
    let page = live_session::page(cache);
    let state = if page.is_some() {
        RecentState::Ready
    } else if let Some(failure) = live_session::failure(cache) {
        RecentState::Unavailable(format!("Transcript unavailable: {failure}"))
    } else if cache.is_none_or(|cache| cache.loading) {
        RecentState::Loading("Loading transcript…")
    } else {
        RecentState::Unavailable("Transcript unavailable".into())
    };
    Recent {
        title: "What it said and did",
        order: "newest first",
        state,
        entries: page
            .map(|page| recent_entries(page, role, feed))
            .unwrap_or_default(),
        empty: "No recorded words or tool runs on this page",
        stale: cache
            .filter(|cache| cache.stale && !cache.loading || cache.error.is_some())
            .filter(|_| page.is_some())
            .map(|_| "Last received transcript; refreshing".into()),
    }
}

/// Build only a selection still visible in the current project and role scopes. `via` is
/// the authenticated device attribution used by the existing session reply model.
pub fn roster_panel(
    state: &State,
    feed: &daemon::Feed,
    env: Env,
    filter: &ProjectFilter,
    via: &str,
) -> Option<RosterPanel> {
    let selected = state.roster_session.as_ref()?;
    let source = roster::selected(feed, selected, filter)
        .filter(|source| roster::visible(*source, &state.roster_role))?;
    let row = roster::row(source, state, feed);
    let target = DetailRef::Session {
        id: selected.to_string(),
    };
    let mut wants = vec![target.clone()];
    let terminal = source.row.and_then(|agent| {
        agent.pane.as_deref().map(|pane| {
            let target = DetailRef::Terminal {
                id: selected.to_string(),
            };
            if state.roster_tab == RosterPanelTab::Terminal {
                wants.push(target.clone());
            }
            live_session::terminal(
                agent,
                pane,
                feed.detail(&target),
                feed,
                super::keys::keys(state, selected, agent, feed, env),
            )
        })
    });
    let tab = if terminal.is_some() {
        state.roster_tab
    } else {
        RosterPanelTab::Summary
    };
    let mut tabs = vec![RosterPanelTabItem {
        id: "seg-agent-panel-summary",
        label: "Summary",
        selected: tab == RosterPanelTab::Summary,
        action: Action::SetRosterTab(RosterPanelTab::Summary),
    }];
    if terminal.is_some() {
        tabs.push(RosterPanelTabItem {
            id: "seg-agent-panel-terminal",
            label: "Terminal",
            selected: tab == RosterPanelTab::Terminal,
            action: Action::SetRosterTab(RosterPanelTab::Terminal),
        });
    }
    let mut facts: Vec<String> = source
        .row
        .and_then(|row| row.profile.clone())
        .or_else(|| source.assistant.and_then(|row| row.profile.clone()))
        .into_iter()
        .collect();
    if let Some(agent) = source.row {
        facts.extend(agent.runtime.clone());
        facts.extend(agent.pane.clone());
        if let Some(since) = live::clock(agent.since.as_deref(), feed) {
            facts.push(format!("since {since}"));
        }
    }
    let waits = source
        .row
        .and_then(|row| row.task.as_deref())
        .and_then(|task| {
            inbox::ordered(feed)
                .into_iter()
                .find(|row| row.task.as_deref() == Some(task))
        })
        .and_then(|waiting| {
            let target = inbox::target(waiting)?;
            if let crate::model::Live::Question(key) = &target {
                wants.push(DetailRef::Question {
                    key: key.to_string(),
                });
            }
            let text = inbox::question_detail(feed, &waiting.id)
                .map(|question| question.text.clone())
                .unwrap_or_else(|| waiting.title.clone());
            Some(RosterWait {
                heading: "Waits on you",
                text,
                label: if inbox::is_question(waiting) {
                    "Answer"
                } else {
                    "Open"
                },
                action: Action::OpenRosterTarget(target),
            })
        });
    let mut reply = live_reply::reply(
        selected,
        source.row,
        source.role(),
        &row.shown_id,
        waits.is_some(),
        state,
        feed,
        env,
        via,
    );
    let chat = (source.role() == "assistant").then(|| {
        Id::from(if source.pinned(feed) {
            assistant::QUICK
        } else {
            source.id()
        })
    });
    if source.row.is_none()
        && let Some(scope) = &chat
    {
        reply.mode = ReplyMode::Closed {
            reason: "Messages use the assistant thread".into(),
            action: Some(("Open thread", Action::OpenThread(scope.clone()))),
        };
    }
    let activity = row.activity;
    Some(RosterPanel {
        session: selected.clone(),
        chip: row.chip,
        hue: row.hue,
        role: row.role,
        shown_id: row.shown_id,
        project: row.project,
        name: row.name,
        facts: facts.join(" · "),
        close_label: format!("Close {}", source.id()),
        close: Action::ClearRoster,
        tabs,
        tab,
        open_label: "Open session",
        open: Action::OpenSession(selected.clone()),
        waits,
        doing_title: "Doing now",
        doing: source.doing(feed),
        last_output: row.last_output,
        activity,
        activity_axis: roster::activity_axis(source.row, feed),
        unknown_activity: "Activity unavailable",
        steps: steps(source),
        recent: recent(feed, &target, source.role()),
        terminal,
        wants,
        reply,
        chat,
    })
}
