//! Palette rows, their actions and fresh-confirmation previews. Renderers display these values
//! and delegate actions to AppState; filtering and command wording stay here. Which keys each
//! destination answers, and their terminal legend, are in `keys`.

mod keys;
#[cfg(test)]
mod tests;

pub use keys::{key_legend, key_legend_live, list_keys, list_keys_live};

use corral_protocol::rows::PlanState;

use crate::model::daemon::Feed;
use crate::model::{
    Conn, DataMode, DataState, Destination, Env, Form, Live, Page, Rich, Risk, SettingsSection,
    ThemeChoice, assistant, span,
};

/// Search field placeholder shared by every palette form.
pub const PALETTE_PLACEHOLDER: &str = "Go to, open a task id, answer, accept…";

/// Keyboard help beneath the desktop palette and in the terminal palette's head. Escape first
/// clears a typed query (the command field's own behaviour), so the hint says what it does now.
pub const fn palette_hints(query_empty: bool) -> [(&'static str, &'static str); 3] {
    [
        ("↑↓", "move"),
        ("Enter", "run"),
        ("Esc", if query_empty { "close" } else { "clear" }),
    ]
}

/// Live rows of one kind the palette lists without a query, and with one.
const LIVE_IDLE: usize = 8;
const LIVE_MATCHED: usize = 40;

/// Empty-search guidance, preserving the example task id's mono emphasis. The example is the
/// fixture's in the preview and a real task id in daemon mode, or none when there is none.
pub fn palette_empty(query: &str, example: Option<&str>) -> Rich {
    let mut rich = vec![span(format!("No command or task matches “{query}”."))];
    if let Some(example) = example {
        rich.extend([
            span(" Task ids work too: try "),
            span(example).mono(),
            span("."),
        ]);
    }
    rich
}

/// The example task id of the empty-search guidance.
pub fn palette_example(data: DataState, feed: &Feed) -> Option<String> {
    match data {
        DataState::Fixture => Some("T-894".into()),
        DataState::Daemon => feed.board.tasks().first().map(|task| task.id.to_string()),
        DataState::Disconnected => None,
    }
}

/// Desktop explanation beneath a fresh-confirmation title.
pub const CONFIRM_SUB: &str =
    "Confirm on this device. The daemon checks the preview still matches before it applies.";

/// Heading above the three consequences of a risky command.
pub const CONFIRM_EFFECT_HEADING: &str = "Exact effect";

/// Confirmation cancellation button label.
pub const CONFIRM_CANCEL: &str = "Cancel";

/// What a tap on the phone's Confirm is checked against. There is no biometric or PIN step on
/// any client: the tap is the confirmation, and corrald accepts it only from this paired device
/// and only while the preview still matches.
pub const CONFIRM_CHECK: &str = "Tapping Confirm is the check. corrald accepts it only from this \
     paired device, and only while the preview above still matches.";

/// Phone confirmation is generic; other forms name the command's first verb.
pub fn confirm_label(command: Risk, form: Form) -> String {
    if form == Form::Phone {
        "Confirm".into()
    } else {
        format!("Confirm {}", confirm_word(command))
    }
}

/// The exact lower-case word required by the terminal's typed confirmation.
pub fn confirm_word(command: Risk) -> String {
    risk(command)
        .title
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_lowercase()
}

/// A live command's confirming button: generic on phone, else its title's first verb.
pub fn act_confirm_label(title: &str, form: Form) -> String {
    if form == Form::Phone {
        "Confirm".into()
    } else {
        format!("Confirm {}", act_confirm_word(title))
    }
}

/// The lower-case first word of a live command's title, typed to confirm in a terminal.
pub fn act_confirm_word(title: &str) -> String {
    title
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_lowercase()
}

/// A local app command, including demo switches not represented by reducer effects.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Navigate to a destination or a Settings section.
    Go(Destination),
    /// Open a Settings section directly.
    Settings(SettingsSection),
    /// Select or push a fixture page.
    Open(Page),
    /// Select or push a live record.
    OpenLive(Live),
    /// Run the same record action offered in the inspector and row menu.
    Work(crate::model::work::Press),
    /// Open Ask, optionally with a free-text draft.
    Ask(Option<String>),
    /// Switch the session between transcript and terminal.
    ToggleSession,
    /// Change the persisted appearance.
    Theme(ThemeChoice),
    /// Change the original fixture transport switch.
    Data(DataState),
    /// Restore the user's Now dock arrangement.
    ResetNow,
    /// Change the persisted connection demo switch.
    Connection(Conn),
    /// Change the persisted data demo switch.
    Mode(DataMode),
    /// Change the assistant's demo lifecycle state.
    Assistant(assistant::Mode),
    /// Show the global hotkey demo window.
    Hotkey,
    /// Show the phone share-sheet demo.
    Share,
    /// Show the phone's encrypted push demo.
    Lock,
}

/// One palette row in source order. `always` bypasses text matching for the free-text Ask row.
#[derive(Debug, Clone, PartialEq)]
pub struct Command {
    /// Stable selection id, separate from the displayed label.
    pub id: String,
    /// Adjacent rows with this group share a heading.
    pub group: &'static str,
    /// Verbatim command wording.
    pub label: String,
    /// The shortcut cap, if any.
    pub key: Option<&'static str>,
    /// What AppState executes after closing the palette.
    pub action: Action,
    /// Free-text Ask is kept even when the query also matches another command.
    pub always: bool,
}

fn row(
    id: impl Into<String>,
    group: &'static str,
    label: impl Into<String>,
    key: Option<&'static str>,
    action: Action,
) -> Command {
    Command {
        id: id.into(),
        group,
        label: label.into(),
        key,
        action,
        always: false,
    }
}

/// All palette commands, including an always-visible Ask row for non-navigation free text.
/// Daemon mode lists only what corrald actually served; the fixture preview keeps its examples
/// and Demo switches; without a connection there are no records, Ask or Demo switches, only
/// navigation, settings and the explicit data choice.
pub fn commands(
    query: &str,
    socket_configured: bool,
    data: DataState,
    feed: &Feed,
    record: Option<&crate::model::work::Record>,
    form: Form,
) -> Vec<Command> {
    let query = query.trim();
    let live = data == DataState::Daemon;
    let preview = data == DataState::Fixture;
    let ask = preview || (live && feed.capable(corral_protocol::app::ASSISTANT_V1));
    let mut rows = Vec::new();
    if ask {
        rows.extend(ask_with(query));
    }
    if live {
        live_needs_you(&mut rows, feed, query);
        live_verbs(&mut rows, feed, record);
    } else if preview {
        rows.extend(fixture_needs_you());
    }
    rows.extend(Destination::ALL.into_iter().map(|destination| {
        row(
            format!("go:{}", destination.slug()),
            "Go to",
            destination.label(),
            destination_key(destination),
            Action::Go(destination),
        )
    }));
    if live {
        live_open(&mut rows, feed, query);
    } else if preview {
        rows.extend(fixture_open());
    }
    rows.push(pair_row(data));
    if preview {
        rows.push(row(
            "toggle-term",
            "View",
            "Switch transcript / terminal",
            Some("`"),
            Action::ToggleSession,
        ));
    }
    rows.extend(SettingsSection::ALL.into_iter().map(|section| {
        row(
            format!("settings:{}", section.slug()),
            "Settings",
            format!("Settings: {}", section.label()),
            None,
            Action::Settings(section),
        )
    }));
    if ask {
        rows.push(row("ask", "Ask", "Ask", Some("/"), Action::Ask(None)));
    }
    rows.extend(
        [ThemeChoice::System, ThemeChoice::Light, ThemeChoice::Dark]
            .into_iter()
            .map(|theme| {
                row(
                    format!("theme:{}", theme.slug()),
                    "Appearance",
                    format!("Theme: {}", theme.slug()),
                    None,
                    Action::Theme(theme),
                )
            }),
    );
    rows.extend(
        [
            DataState::Fixture,
            DataState::Daemon,
            DataState::Disconnected,
        ]
        .into_iter()
        .filter(|state| *state != DataState::Daemon || socket_configured)
        .map(|data| {
            row(
                format!("data:{}", data.slug()),
                "Data",
                format!("Data: {}", data.slug()),
                None,
                Action::Data(data),
            )
        }),
    );
    rows.push(row(
        "reset-now",
        "Now",
        "Reset Now layout",
        None,
        Action::ResetNow,
    ));
    if preview {
        rows.extend(demo_rows().into_iter().filter(|row| {
            form == Form::Phone || !matches!(row.action, Action::Share | Action::Lock)
        }));
    }
    rows
}

/// The free-text Ask row, unless the text reads as navigation. It is ranked after every
/// command the text also matches (`filtered`), so Enter runs a typed command, not a question.
fn ask_with(query: &str) -> Option<Command> {
    let folded = query.to_lowercase();
    if query.is_empty()
        || ["t-", "go ", "open "]
            .iter()
            .any(|prefix| folded.starts_with(prefix))
    {
        return None;
    }
    let mut ask = row(
        "ask-with",
        "Ask",
        format!("Ask: “{query}”"),
        None,
        Action::Ask(Some(query.into())),
    );
    ask.always = true;
    Some(ask)
}

/// Where pairing happens from here: an enrolled device pairs others from Devices; anything
/// else pairs itself from Connection. The palette only navigates; pairing is that page's.
fn pair_row(data: DataState) -> Command {
    if data == DataState::Daemon {
        row(
            "pair",
            "Device",
            "Pair a new device",
            None,
            Action::Settings(SettingsSection::Devices),
        )
    } else {
        row(
            "pair",
            "Device",
            "Pair this device",
            None,
            Action::Settings(SettingsSection::Connection),
        )
    }
}

/// Adds live rows of one group: those matching the query, a few without one.
fn push_live(
    rows: &mut Vec<Command>,
    group: &'static str,
    query: &str,
    items: impl Iterator<Item = (String, Live)>,
) {
    let folded = query.to_lowercase();
    let limit = if folded.is_empty() {
        LIVE_IDLE
    } else {
        LIVE_MATCHED
    };
    rows.extend(
        items
            .filter(|(label, _)| {
                folded.is_empty() || format!("{group} {label}").to_lowercase().contains(&folded)
            })
            .take(limit)
            .map(|(label, live)| {
                row(
                    format!("live:{}:{}", live.kind().slug(), live.id()),
                    group,
                    label,
                    None,
                    Action::OpenLive(live),
                )
            }),
    );
}

/// Open owner decisions the daemon served: questions, prompts, plans to accept, reviews.
fn live_needs_you(rows: &mut Vec<Command>, feed: &Feed, query: &str) {
    let open = crate::model::inbox::owner_moves(feed).filter_map(|row| {
        let target = crate::model::inbox::target(row)?;
        let label = match &target {
            Live::Question(id) => format!("Answer {id} · {}", row.title),
            Live::Plan(id) => format!("Review plan {id} · {}", row.title),
            Live::Review(id) => format!("Review {id} · {}", row.title),
            Live::Thread(_) | Live::Session(_) => format!("Permission {} · {}", row.id, row.title),
            Live::Task(id) => format!("{id} · {}", row.title),
        };
        Some((label, target))
    });
    push_live(rows, "Needs you", query, open);
}

fn live_verbs(rows: &mut Vec<Command>, feed: &Feed, record: Option<&crate::model::work::Record>) {
    use crate::model::work::{Record, record_verbs};
    let workspace = Record::Workspace;
    let actions = record_verbs(feed, record.unwrap_or(&workspace));
    rows.extend(actions.into_iter().filter(|verb| verb.enabled).map(|verb| {
        row(
            format!("verb:{}", verb.slug),
            "Actions",
            verb.label,
            None,
            Action::Work(verb.press),
        )
    }));
    if record.is_some_and(|record| !matches!(record, Record::Workspace)) {
        rows.extend(
            record_verbs(feed, &workspace)
                .into_iter()
                .filter(|verb| verb.enabled)
                .map(|verb| {
                    row(
                        format!("verb:{}", verb.slug),
                        "Workspace actions",
                        verb.label,
                        None,
                        Action::Work(verb.press),
                    )
                }),
        );
    }
    rows.extend(
        feed.commands
            .keys()
            .filter(|key| {
                crate::model::act::status(feed, key) == Some(crate::model::act::ActStatus::Unknown)
            })
            .map(|key| {
                row(
                    format!("recover:{key}"),
                    "Command results",
                    format!("Check command result {key}"),
                    None,
                    Action::Work(crate::model::work::Press::Recover(key.clone())),
                )
            }),
    );
}

/// Records the daemon served: board tasks, plans, agent sessions, assistant threads.
fn live_open(rows: &mut Vec<Command>, feed: &Feed, query: &str) {
    push_live(
        rows,
        "Open",
        query,
        feed.board.tasks().iter().map(|task| {
            (
                format!("{} {}", task.id, task.title),
                Live::Task(task.id.clone()),
            )
        }),
    );
    push_live(
        rows,
        "Open",
        query,
        feed.plans
            .iter()
            .filter(|(_, plan)| !matches!(plan.state, PlanState::Complete | PlanState::Cancelled))
            .map(|(id, plan)| (format!("Plan {id} {}", plan.title), Live::Plan(id.clone()))),
    );
    push_live(
        rows,
        "Open",
        query,
        feed.agents.iter().map(|(id, agent)| {
            let task = agent.task.as_deref().unwrap_or(id.as_str());
            (
                format!("{task} {} · session", agent.title),
                Live::Session(id.clone()),
            )
        }),
    );
    push_live(
        rows,
        "Open",
        query,
        feed.assistants.iter().map(|(id, thread)| {
            (
                format!("{id} {} · thread", thread.title),
                Live::Thread(id.clone()),
            )
        }),
    );
}

fn fixture_needs_you() -> [Command; 2] {
    [
        row(
            "question",
            "Needs you",
            "Answer T-868/R1/Q1 · restore policy",
            None,
            Action::Open(Page::Question),
        ),
        row(
            "plan:T-885",
            "Needs you",
            "Review plan T-885 P2 · accept or request changes",
            None,
            Action::Open(Page::Plan("T-885")),
        ),
    ]
}

fn fixture_open() -> [Command; 5] {
    [
        row(
            "task:T-890",
            "Open",
            "T-890 pm-bounded-read-contract",
            None,
            Action::Open(Page::Task("T-890")),
        ),
        row(
            "task:T-894",
            "Open",
            "T-894 api-connections",
            None,
            Action::Open(Page::Task("T-894")),
        ),
        row(
            "session:T-871",
            "Open",
            "T-871 annotation-service · session",
            None,
            Action::Open(Page::Session("T-871")),
        ),
        row(
            "session:T-868",
            "Open",
            "T-868 host-queue-ledger · session",
            None,
            Action::Open(Page::Session("T-868")),
        ),
        row(
            "plan:T-870",
            "Open",
            "Plan T-870 Pi input queue lifecycle",
            None,
            Action::Open(Page::Plan("T-870")),
        ),
    ]
}

/// The preview's Demo switches; never offered against a daemon.
fn demo_rows() -> Vec<Command> {
    let mut rows: Vec<Command> = Conn::ALL
        .into_iter()
        .map(|conn| {
            row(
                format!("conn:{}", conn.slug()),
                "Demo",
                format!(
                    "Connection: {}",
                    match conn {
                        Conn::Healthy => "healthy · direct",
                        Conn::Degraded => "degraded",
                        Conn::Down => "daemon down",
                        Conn::Relay => "relay · e2e",
                        Conn::Offline => "offline · last sync",
                    }
                ),
                None,
                Action::Connection(conn),
            )
        })
        .collect();
    rows.extend(DataMode::ALL.into_iter().map(|mode| {
        row(
            format!("mode:{}", mode.slug()),
            "Demo",
            format!(
                "Data: {}",
                if mode == DataMode::Elsewhere {
                    "answered elsewhere"
                } else {
                    mode.slug()
                }
            ),
            None,
            Action::Mode(mode),
        )
    }));
    rows.extend(
        assistant::Mode::ALL
            .into_iter()
            .enumerate()
            .map(|(index, mode)| {
                row(
                    format!("assistant:{index}"),
                    "Demo",
                    format!("Assistant: {}", mode.label()),
                    None,
                    Action::Assistant(mode),
                )
            }),
    );
    rows.extend([
        row(
            "demo-hotkey",
            "Demo",
            "Show hotkey window",
            None,
            Action::Hotkey,
        ),
        row(
            "demo-share",
            "Demo",
            "Show share sheet",
            None,
            Action::Share,
        ),
        row(
            "demo-lock",
            "Demo",
            "Show lock-screen push",
            None,
            Action::Lock,
        ),
    ]);
    rows
}

/// Commands matching group plus label, case-insensitively. The free-text Ask row always
/// survives, after the commands the text matches, so the first match is the default.
pub fn filtered(
    query: &str,
    socket_configured: bool,
    data: DataState,
    feed: &Feed,
    record: Option<&crate::model::work::Record>,
    form: Form,
) -> Vec<Command> {
    let folded = query.trim().to_lowercase();
    let (ask, mut rows): (Vec<Command>, Vec<Command>) =
        commands(query, socket_configured, data, feed, record, form)
            .into_iter()
            .filter(|command| {
                command.always
                    || folded.is_empty()
                    || format!("{} {}", command.group, command.label)
                        .to_lowercase()
                        .contains(&folded)
            })
            .partition(|command| command.always);
    rows.extend(ask);
    rows
}

/// Resolves a selected command without interpreting arbitrary ids as executable commands.
pub fn run(
    id: &str,
    query: &str,
    socket_configured: bool,
    data: DataState,
    feed: &Feed,
    record: Option<&crate::model::work::Record>,
    form: Form,
) -> Option<Action> {
    commands(query, socket_configured, data, feed, record, form)
        .into_iter()
        .find(|command| command.id == id)
        .map(|command| command.action)
}

/// The destination's shortcut as a static key cap for renderers.
pub const fn destination_key(destination: Destination) -> Option<&'static str> {
    Some(match destination {
        Destination::Inbox => "i",
        Destination::Now => "n",
        Destination::Changes => ".",
        Destination::Plans => "p",
        Destination::Agents => "a",
        Destination::Review => "r",
        Destination::Timeline => "t",
        Destination::Usage => "u",
        Destination::Settings => ",",
    })
}

/// Looks up a row's shortcut without allocating the command table during row rendering.
pub fn shortcut(id: &str) -> Option<&'static str> {
    match id {
        "ask" => Some("/"),
        "toggle-term" => Some("`"),
        _ => id
            .strip_prefix("go:")
            .and_then(Destination::from_slug)
            .and_then(destination_key),
    }
}

/// A fresh-confirmation sheet's exact command, effects and success receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RiskInfo {
    /// The dialog title before an optional target name.
    pub title: &'static str,
    /// The daemon command whose effect is previewed.
    pub command: &'static str,
    /// The three exact consequences, in display order.
    pub effects: [&'static str; 3],
    /// The success toast before target and attribution.
    pub ok: &'static str,
}

impl RiskInfo {
    /// Exact command and device attribution under the three effect lines.
    pub fn preview(self, name: Option<&str>, env: Env) -> String {
        let name = name
            .filter(|name| !name.is_empty())
            .map(|name| format!(" {name}"))
            .unwrap_or_default();
        format!(
            "{}{name} · actor=owner · via=app:{} · challenge expires in 60 s",
            self.command,
            env.device()
        )
    }
}

/// The complete RISK table, shared by the confirmation UI and success receipts.
pub const fn risk(risk: Risk) -> RiskInfo {
    match risk {
        Risk::AcceptPlan => RiskInfo {
            title: "Accept plan T-885 P2",
            command: "corral plan accept T-885 --rev P2",
            effects: [
                "Publishes 3 member tasks to main as tracker T-893",
                "Autopilot may admit pm-bounded-read-contract next",
                "Gates, independent review and landing checks unchanged",
            ],
            ok: "Accepted T-885 P2 · 3 members published as T-893",
        },
        Risk::ApproveReview => RiskInfo {
            title: "Approve candidate 85b45e1e",
            command: "corral review approve T-868 --sha 85b45e1e",
            effects: [
                "Owner verdict on T-868 R1 at 85b45e1e only",
                "Landing proceeds to main after the land check",
                "A newer candidate would make this verdict stale",
            ],
            ok: "Approved 85b45e1e · landing proceeds",
        },
        Risk::Revoke => RiskInfo {
            title: "Revoke device",
            command: "corral device revoke",
            effects: [
                "Retires this device's keys now",
                "Closes its open sessions and relay route",
                "Its past actions keep their via= attribution",
            ],
            ok: "Revoked · keys retired, sessions closed",
        },
        Risk::ModePause => RiskInfo {
            title: "Confirm corral mode pause",
            command: "corral mode pause",
            effects: [
                "Autopilot admits no new work after the running gates finish",
                "Running gates, reviews and landings settle normally",
                "Cites owner input A-7/g2/i19: \"Pause autopilot for tonight once the running gates finish.\"",
            ],
            ok: "corral mode pause executed · receipt in the quick thread",
        },
        Risk::Discard => RiskInfo {
            title: "Discard",
            command: "corral discard",
            effects: [
                "Drops the Idea the assistant filed from your message",
                "Your words stay in the quick thread",
                "Nothing else on the board changes",
            ],
            ok: "Discarded · receipt in the quick thread",
        },
    }
}
