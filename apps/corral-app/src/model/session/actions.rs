//! The session's action catalog, one list for the ⋯ menu, the palette's session group and the
//! `/` command menu. Each row says its key and its terminal command where it has one; a row the
//! host cannot run now stays listed with its reason and does not run. The run controls (Cancel
//! turn, Pause, Resume, Rewind…, Terminate or Stop, Relaunch or Resume) and the task's Actions
//! menu are rows here; every part of the page adds its own rows through its `actions(cx)`,
//! merged into the group its rows name. This module adds the session-wide rows: the model and
//! effort pickers (`picker`), Compact, Export, the board's refresh, thinking, expand all, jump to
//! the end, and the terminal UI's commands the app does not carry, listed with why. The `/`
//! menu over the composer (`slash`) lists these rows and the host's own commands.

use corral_protocol::detail::DetailRef;
use corral_protocol::session_host::{SessionAction, SessionActionRequest};

use crate::model::Id;
use crate::model::agents::{Control, control_label};
use crate::model::daemon::Feed;
use crate::model::effect::Effect;

use super::transcript::EXPAND_ALL;
use super::{
    Action, Can, Cx, FleetAction, Host, Pending, SessionRole, State, Tab, board, changes, events,
    exit, fleet, plan, proof, questions, sidebar, task, usage,
};

#[path = "picker.rs"]
mod picker;
#[path = "slash.rs"]
mod slash;

pub use picker::{
    PickRow, Picker, PickerAction, PickerKind, PickerUi, ScopeSwitch, picker as model_picker,
};
pub(super) use picker::{change_pending as model_change_pending, changing as model_changing};
pub use slash::{SlashMenu, SlashRow, slash};

/// Each part's rows, in catalog order.
const PARTS: &[fn(&Cx<'_>) -> Option<ActionGroup>] = &[
    questions::actions,
    exit::actions,
    sidebar::actions,
    plan::actions,
    events::actions,
    usage::actions,
    changes::actions,
    board::actions,
    task::actions,
    proof::actions,
    fleet::actions,
];

#[derive(Debug, Clone, PartialEq)]
pub struct SessionActions {
    pub groups: Vec<ActionGroup>,
}

impl SessionActions {
    /// Every row, in group order.
    pub fn rows(&self) -> impl Iterator<Item = &ActionRow> {
        self.groups.iter().flat_map(|group| &group.rows)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ActionGroup {
    /// Views · Message · Display · Run · Task.
    pub title: &'static str,
    pub rows: Vec<ActionRow>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ActionRow {
    /// `act-<slug>`.
    pub id: String,
    pub label: String,
    /// The terminal UI's command (`/finish`), matched by the `/` menu.
    pub command: Option<&'static str>,
    pub key: Option<&'static str>,
    /// Listed muted with ` · unavailable: <reason>`; does not run.
    pub unavailable: Option<String>,
    pub action: Action,
}

/// What the session-wide rows do beyond the page's own actions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandAction {
    /// A host action whose only reply is its notice (`compact`, `export`).
    Host {
        session: Id,
        action: SessionAction,
        /// How the notice names it: `Compact`, `Export`.
        what: &'static str,
    },
    /// `/board`: refresh the build's board, then open its tab.
    Board(Id),
    /// Close the `/` menu and keep the text.
    CloseMenu(Id),
    /// Highlight row `at` of the `/` menu.
    MenuAt { session: Id, at: usize },
    /// The owner has read this notice: it leaves the header.
    DismissNotice { session: Id, notice: String },
    /// End the quick chat `A-n` for good, after this device's one confirmation (as Ask's head).
    EndThread(String),
}

/// Why an operation the host declares as `capability` cannot run now; `absent` when the
/// session's host does not offer it at all, so the row stays listed with why.
fn blocked(cx: &Cx<'_>, capability: &str, absent: &str) -> Option<String> {
    match cx.can(capability) {
        Can::Absent => Some(absent.to_owned()),
        Can::Unavailable(why) => Some(why),
        Can::Yes | Can::Unknown => match cx.host {
            Host::NotRunning | Host::Ended => Some("the host is not running".into()),
            Host::Unreachable => Some("corrald cannot reach the host".into()),
            Host::Running | Host::Unknown => None,
        },
    }
}

/// Whether the page has a live session with a composer: not a child's transcript, not a log.
fn talks(cx: &Cx<'_>) -> bool {
    *cx.role != SessionRole::Child && !cx.log_only()
}

fn with(
    mut found: ActionRow,
    command: Option<&'static str>,
    unavailable: Option<String>,
) -> ActionRow {
    found.command = command;
    found.unavailable = unavailable;
    found
}

/// Choose model, cycle and choose effort, next and previous model.
fn model(cx: &Cx<'_>) -> Option<ActionGroup> {
    if !talks(cx) {
        return None;
    }
    let session = cx.id.clone();
    let blocked = picker::unavailable(cx);
    let no_effort = cx
        .state
        .is_some_and(|state| state.effort_levels.is_empty())
        .then(|| "this model has no effort levels".to_owned());
    let open = |kind| {
        Action::Picker(PickerAction::Open {
            session: session.clone(),
            kind,
        })
    };
    let step = |delta| {
        Action::Picker(PickerAction::NextModel {
            session: session.clone(),
            delta,
        })
    };
    // While an agent's conversation is open the composer shows that agent's model: Choose
    // model opens its choices, and the session's own effort and model order stay listed, muted.
    let agent = super::fleet::opened_agent(cx);
    let (choose, choose_blocked) = match &agent {
        Some(agent) => (
            Action::Fleet(FleetAction::Models {
                session: session.clone(),
                child: Some(agent.child.clone()),
            }),
            agent.model.unavailable.clone(),
        ),
        None => (open(PickerKind::Model), blocked.clone()),
    };
    let blocked = agent
        .map(|agent| {
            format!(
                "agent {}'s conversation is open; the composer shows its model and effort as reported",
                agent.id
            )
        })
        .or(blocked);
    Some(ActionGroup {
        title: "Model",
        rows: vec![
            with(
                row("model", "Choose model…", Some("Ctrl+Shift+M"), choose),
                Some("/model"),
                choose_blocked,
            ),
            with(
                row(
                    "effort",
                    "Cycle effort",
                    Some("Alt+E"),
                    Action::Picker(PickerAction::CycleEffort(session.clone())),
                ),
                None,
                blocked.clone().or_else(|| no_effort.clone()),
            ),
            with(
                row(
                    "effort-list",
                    "Choose effort…",
                    None,
                    open(PickerKind::Effort),
                ),
                None,
                blocked.clone().or(no_effort),
            ),
            with(
                row("model-next", "Next model", Some("Alt+P"), step(1)),
                None,
                blocked.clone(),
            ),
            with(
                row(
                    "model-previous",
                    "Previous model",
                    Some("Alt+Shift+P"),
                    step(-1),
                ),
                None,
                blocked,
            ),
        ],
    })
}

#[allow(clippy::too_many_arguments)]
fn host_row(
    cx: &Cx<'_>,
    slug: &str,
    label: &str,
    command: &'static str,
    capability: &str,
    absent: &str,
    action: SessionAction,
    what: &'static str,
) -> ActionRow {
    with(
        row(
            slug,
            label,
            None,
            Action::Command(CommandAction::Host {
                session: cx.id.clone(),
                action,
                what,
            }),
        ),
        Some(command),
        blocked(cx, capability, absent),
    )
}

/// Plan mode's switch, named for where it leads. It carries no `/` name: the host's own `/plan`
/// takes a request after it and is sent as typed.
fn plan_mode(cx: &Cx<'_>) -> ActionRow {
    let on = cx.state.is_some_and(|state| state.plan_mode.is_some());
    with(
        row(
            "plan-mode",
            if on {
                "Leave plan mode"
            } else {
                "Enter plan mode"
            },
            None,
            Action::Command(CommandAction::Host {
                session: cx.id.clone(),
                action: SessionAction::PlanMode { enabled: !on },
                what: "Plan mode",
            }),
        ),
        None,
        blocked(cx, "plan_mode", "this session's host has no plan mode"),
    )
}

/// Compact context (`/compact`, optional instructions after it in the `/` menu) and plan mode.
fn message(cx: &Cx<'_>) -> Option<ActionGroup> {
    talks(cx).then(|| ActionGroup {
        title: "Message",
        rows: vec![
            host_row(
                cx,
                "compact",
                "Compact context",
                "/compact",
                "compact",
                "this session's host does not compact its context",
                SessionAction::Compact { instructions: None },
                "Compact",
            ),
            plan_mode(cx),
        ],
    })
}

/// Export session (its result is a notice) and the build's board refresh, listed with why where
/// the host does not offer them.
fn workspace(cx: &Cx<'_>) -> Option<ActionGroup> {
    if !talks(cx) {
        return None;
    }
    let mut rows = vec![host_row(
        cx,
        "export",
        "Export session",
        "/corral-export",
        "export",
        "this session's host does not export sessions",
        SessionAction::Export,
        "Export",
    )];
    if *cx.role == SessionRole::Build {
        rows.push(with(
            row(
                "board",
                "Refresh and open the board",
                None,
                Action::Command(CommandAction::Board(cx.id.clone())),
            ),
            Some("/board"),
            blocked(cx, "board", "this build has no board"),
        ));
    }
    Some(ActionGroup {
        title: "Workspace",
        rows,
    })
}

/// The terminal UI's commands the app does not carry, listed with where their job is done.
fn help(cx: &Cx<'_>) -> ActionGroup {
    let nothing = Action::Picker(PickerAction::Close(cx.id.clone()));
    ActionGroup {
        title: "Help",
        rows: vec![
            with(
                row("settings", "Pi settings", None, nothing.clone()),
                Some("/settings"),
                Some(
                    "Pi's settings card is for its own terminal; the app's theme is in Settings › General"
                        .into(),
                ),
            ),
            with(
                row("login", "Sign in to a provider", None, nothing),
                Some("/login"),
                Some("sign in from a terminal with corral-pi login --provider <id>".into()),
            ),
        ],
    }
}

pub(super) fn row(
    slug: &str,
    label: impl Into<String>,
    key: Option<&'static str>,
    action: Action,
) -> ActionRow {
    ActionRow {
        id: format!("act-{slug}"),
        label: label.into(),
        command: None,
        key,
        unavailable: None,
        action,
    }
}

fn views(cx: &Cx<'_>, tabs: &[Tab]) -> ActionGroup {
    ActionGroup {
        title: "Views",
        rows: tabs
            .iter()
            .map(|tab| {
                row(
                    &format!("view-{}", tab.slug()),
                    format!("Open {}", tab.label()),
                    Some(tab.key(&cx.role)),
                    Action::Tab {
                        session: cx.id.clone(),
                        tab: *tab,
                    },
                )
            })
            .collect(),
    }
}

fn display(cx: &Cx<'_>) -> ActionGroup {
    let session = cx.id.clone();
    let mut rows = vec![with(
        row(
            "thinking",
            if cx.ui.hide_thinking {
                "Show thinking"
            } else {
                "Hide thinking"
            },
            Some("Alt+H"),
            Action::Thinking(session.clone()),
        ),
        Some("/thinking"),
        None,
    )];
    if !cx.log_only() {
        rows.push(row(
            "expand-all",
            if cx.ui.expanded.contains(EXPAND_ALL) {
                "Fold all tool rows"
            } else {
                "Expand all tool rows"
            },
            Some("Ctrl+O"),
            Action::Expand {
                session: session.clone(),
                key: EXPAND_ALL.into(),
                fetch: None,
            },
        ));
        rows.push(row(
            "jump-latest",
            "Jump to latest",
            Some("End"),
            Action::JumpEnd(session),
        ));
    }
    ActionGroup {
        title: "Display",
        rows,
    }
}

/// Why Rewind… cannot run: the one reading the reducer also applies before it asks.
fn rewinds_blocked(cx: &Cx<'_>) -> Option<String> {
    crate::model::agents::rewind_blocked(cx.feed, cx.id.as_str())
}

fn run(cx: &Cx<'_>) -> ActionGroup {
    let kind = crate::model::agents::kind(cx.feed, cx.id.as_str());
    let mut rows: Vec<ActionRow> = cx
        .controls()
        .into_iter()
        .map(|control| {
            let mut found = row(
                control.slug(),
                control_label(control, kind),
                (control == Control::Cancel).then_some("esc"),
                Action::Control {
                    session: cx.id.clone(),
                    control,
                },
            );
            if control == Control::Rewinds {
                found.command = Some("/rewind");
                found.unavailable = rewinds_blocked(cx);
            }
            found
        })
        .collect();
    // A quick chat that has not ended can be ended; it stays readable as its `A-n`.
    if let Some(chat) = cx.row.filter(|row| {
        row.kind == corral_protocol::rows::AssistantKind::Quick
            && row.state != corral_protocol::rows::AssistantState::Ended
    }) {
        rows.push(row(
            "end-thread",
            "End thread",
            None,
            Action::Command(CommandAction::EndThread(chat.id.clone())),
        ));
    }
    ActionGroup { title: "Run", rows }
}

fn task(cx: &Cx<'_>) -> Option<ActionGroup> {
    let task = cx.task.filter(|_| cx.role.task_bound())?.clone();
    Some(ActionGroup {
        title: "Task",
        rows: vec![row(
            "task-actions",
            format!("Task {task} actions…"),
            None,
            Action::TaskMenu(task),
        )],
    })
}

/// The catalog of the session `cx` reads, over the tabs its page shows, in the screens' group
/// order: Views · Message · Model · Workspace · Display · Run · Task · Help, each part's rows
/// merged into the group they name.
pub fn actions(cx: &Cx<'_>) -> SessionActions {
    let tabs = super::shown_tabs(cx);
    let mut groups: Vec<ActionGroup> = [
        Some(views(cx, &tabs)),
        message(cx),
        model(cx),
        workspace(cx),
        Some(display(cx)),
        Some(run(cx)),
        task(cx),
    ]
    .into_iter()
    .flatten()
    .collect();
    let parts: Vec<ActionGroup> = PARTS.iter().filter_map(|part| part(cx)).collect();
    // One command, one row: a part's own row wins over the session-wide one.
    let claimed: Vec<&'static str> = parts
        .iter()
        .flat_map(|group| &group.rows)
        .filter_map(|found| found.command)
        .collect();
    for group in &mut groups {
        group.rows.retain(|found| {
            found
                .command
                .is_none_or(|command| !claimed.contains(&command))
        });
    }
    for group in parts {
        match groups.iter_mut().find(|known| known.title == group.title) {
            Some(known) => known.rows.extend(group.rows),
            None => groups.push(group),
        }
    }
    groups.push(help(cx));
    for found in groups.iter_mut().flat_map(|group| group.rows.iter_mut()) {
        // The terminal UI's command for a part's row that does not name its own.
        if found.command.is_none() && found.id == "act-answer" {
            found.command = Some("/questions");
        }
    }
    groups.retain(|group| !group.rows.is_empty());
    SessionActions { groups }
}

/// The reads the pickers and the `/` menu need now.
pub(super) fn fetches(cx: &Cx<'_>) -> Vec<Action> {
    [picker::fetches(cx), slash::fetches(cx)]
        .into_iter()
        .flatten()
        .collect()
}

/// Sends a `session.action` under a new key; its reply text or refusal becomes a notice.
fn host_action(
    state: &mut State,
    session: &Id,
    action: SessionAction,
    what: &'static str,
) -> Effect {
    let key = state.ids.next_id();
    state
        .ui_mut(session)
        .dock
        .pending
        .insert(key.clone(), Pending::Action { what });
    Effect::SessionAction {
        key,
        request: SessionActionRequest {
            id: session.to_string(),
            action,
        },
    }
}

pub(super) fn act(state: &mut State, action: CommandAction, feed: &Feed) -> Vec<Effect> {
    match action {
        CommandAction::Host {
            session,
            action,
            what,
        } => vec![host_action(state, &session, action, what)],
        CommandAction::Board(session) => {
            let mut effects = vec![host_action(
                state,
                &session,
                SessionAction::BoardRefresh,
                "Board refresh",
            )];
            effects.extend(state.act(
                Action::Tab {
                    session: session.clone(),
                    tab: Tab::Board,
                },
                feed,
            ));
            effects.extend(state.act(
                Action::Refresh(DetailRef::BuildBoard {
                    id: session.to_string(),
                }),
                feed,
            ));
            effects
        }
        CommandAction::CloseMenu(session) => {
            let ui = state.ui_mut(&session);
            ui.picker.slash_closed = Some(ui.draft.clone());
            Vec::new()
        }
        CommandAction::MenuAt { session, at } => {
            state.ui_mut(&session).picker.slash_at = Some(at);
            Vec::new()
        }
        CommandAction::DismissNotice { session, notice } => {
            state.ui_mut(&session).notices.dismiss(&notice);
            Vec::new()
        }
        CommandAction::EndThread(chat) => vec![Effect::EndThread(chat)],
    }
}

pub(super) use picker::act as picker_act;
