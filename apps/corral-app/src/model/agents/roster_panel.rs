//! The selected roster session's story: its run facts, what blocks its task, what it does now,
//! its recorded words (from a corrald that serves transcripts) and its pane's screen. Board
//! stages never masquerade as the session's own steps.
use corral_protocol::app::SESSION_V1;
use corral_protocol::detail::{DetailRef, SessionItem, SessionPage};

use crate::model::daemon;
use crate::model::effect::Env;
use crate::model::projects::ProjectFilter;
use crate::model::{Hue, Id, Live, assistant};

use super::roster::{self, Source};
use super::{Action, Reply, ReplyMode, State, Terminal, live, live_reply, live_session, pane};

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
    /// The run session whose Session menu the panel head carries; none for an assistant.
    pub menu: Option<Id>,
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
    pub quiet: String,
    /// What it said and did, from a corrald that serves transcripts.
    pub recent: Option<Recent>,
    /// How the latest pane request (keys or Focus pane) ended, in its tone.
    pub pane: Option<(Hue, String)>,
    pub terminal: Option<Terminal>,
    /// Keep these details loaded while this panel is visible.
    pub wants: Vec<DetailRef>,
    /// Dispatch all panel presses with `State::act_roster`; their target belongs to the roster
    /// selection rather than the full-session route.
    pub reply: Reply,
    /// Assistant messages use the existing shared thread composer, including the quick scope.
    pub chat: Option<Id>,
}

fn recent_entries(page: &SessionPage, role: &str, feed: &daemon::Feed) -> Vec<RecentEntry> {
    let mut entries = Vec::new();
    for message in page.messages.iter().rev() {
        let at = live::clock(message.at.as_deref(), feed).unwrap_or_else(|| "—".into());
        let author = message.author.as_str();
        let agent_words = author == role || matches!(author, "assistant" | "worker" | "reviewer");
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

/// What the run's task waits on: its Blocked card's reason, opening the task where its next
/// steps are offered.
fn waits(source: Source<'_>, feed: &daemon::Feed) -> Option<RosterWait> {
    let task = feed.board.task(&source.row?.run.task)?;
    let blocked = task.blocked()?;
    Some(RosterWait {
        heading: "Waits on you",
        text: blocked.reason.clone(),
        label: "Open task",
        action: Action::OpenRosterTarget(Live::Task(task.id.clone())),
    })
}

/// Build only a selection still visible in the current project and role scopes. `via` is
/// the authenticated device attribution used by the session reply model.
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
    // An assistant's recorded words come with chat; a run's only from a corrald serving them.
    let transcripts = source.row.is_none() || feed.capable(SESSION_V1);
    let mut wants = Vec::new();
    if transcripts {
        wants.push(target.clone());
    }
    let terminal = source
        .row
        .filter(|agent| agent.run.session.is_some())
        .map(|agent| {
            let target = DetailRef::Terminal {
                id: selected.to_string(),
            };
            if state.roster_tab == RosterPanelTab::Terminal {
                wants.push(target.clone());
            }
            live_session::terminal(selected, agent, feed.detail(&target), feed)
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
    let mut facts: Vec<String> = source.agent().into_iter().collect();
    if let Some(agent) = source.row {
        facts.push(agent.run.harness.clone());
        facts.extend(live::session_name(agent));
        if let Some(since) = live::clock_at(agent.run.started, feed) {
            facts.push(format!("since {since}"));
        }
    }
    let waits = waits(source, feed);
    let mut reply = live_reply::reply(selected, source.row, &row.shown_id, state, feed, env, via);
    let chat = source.row.is_none().then(|| {
        Id::from(if source.pinned(feed) {
            assistant::QUICK
        } else {
            source.id
        })
    });
    if let Some(scope) = &chat {
        reply.mode = ReplyMode::Closed {
            reason: "Messages use the assistant thread".into(),
            action: Some(("Open thread", Action::OpenThread(scope.clone()))),
        };
    }
    Some(RosterPanel {
        session: selected.clone(),
        menu: row.task.as_ref().map(|_| selected.clone()),
        chip: row.chip,
        hue: row.hue,
        role: row.role,
        shown_id: row.shown_id,
        project: row.project,
        name: row.name,
        facts: facts.join(" · "),
        close_label: format!("Close {}", source.id),
        close: Action::ClearRoster,
        tabs,
        tab,
        open_label: "Open session",
        open: Action::OpenSession(selected.clone()),
        waits,
        doing_title: "Doing now",
        doing: source.doing(feed),
        quiet: row.quiet,
        recent: transcripts.then(|| recent(feed, &target, source.role())),
        pane: source
            .row
            .and_then(|agent| pane::outcome(state, agent, feed)),
        terminal,
        wants,
        reply,
        chat,
    })
}
