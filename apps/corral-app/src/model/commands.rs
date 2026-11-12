//! Palette rows, their actions and fresh-confirmation previews. Renderers display these values
//! and delegate actions to AppState; filtering and command wording stay here.

use crate::model::{
    Conn, DataMode, DataState, Destination, Env, Form, Page, Rich, Risk, SettingsSection,
    ThemeChoice, assistant, span,
};

/// Search field placeholder shared by every palette form.
pub const PALETTE_PLACEHOLDER: &str = "Go to, open a task id, answer, accept…";

/// Keyboard help beneath the desktop palette.
pub const PALETTE_HINTS: [(&str, &str); 3] = [("↑↓", "move"), ("Enter", "run"), ("Esc", "close")];

/// The palette footer's command-system attribution.
pub const PALETTE_FOOTER: &str = "Command · NSH";

/// Empty-search guidance, preserving the example task id's mono emphasis.
pub fn palette_empty(query: &str) -> Rich {
    vec![
        span(format!(
            "No command or task matches “{query}”. Task ids work too: try "
        )),
        span("T-894").mono(),
        span("."),
    ]
}

/// Desktop explanation beneath a fresh-confirmation title.
pub const CONFIRM_SUB: &str =
    "Confirm on this device. The daemon checks the preview still matches before it applies.";

/// Heading above the three consequences of a risky command.
pub const CONFIRM_EFFECT_HEADING: &str = "Exact effect";

/// Confirmation cancellation button label.
pub const CONFIRM_CANCEL: &str = "Cancel";

/// Phone presence-check prompt above the confirmation buttons.
pub const CONFIRM_PRESENCE: &str = "Confirm with Face ID, or enter your PIN";

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

/// Screen-local terminal key hints, using the same page guards as the domain reducers.
/// Destination navigation and global Ask/palette hints are rendered separately by the shell.
pub fn key_legend(
    destination: Destination,
    page: Option<Page>,
) -> Vec<(&'static str, &'static str)> {
    match (destination, page) {
        (_, Some(Page::Question)) => vec![("1 2", "choose"), ("e", "evidence")],
        (Destination::Inbox, None) => vec![
            ("j k", "move"),
            ("Enter", "open"),
            ("f", "focus"),
            ("1 2", "choose"),
        ],
        (Destination::Now, None | Some(Page::Task(_))) => {
            vec![("j k", "move"), ("Enter", "open"), ("Tab", "tab")]
        }
        (Destination::Plans, _) => vec![("j k", "plan"), ("J K", "member"), ("x", "scale")],
        (Destination::Review, None) => vec![("s", "split")],
        _ => Vec::new(),
    }
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
pub fn commands(query: &str, socket_configured: bool, data: DataState) -> Vec<Command> {
    let query = query.trim();
    let folded = query.to_lowercase();
    let mut rows = Vec::new();
    if !query.is_empty()
        && !["t-", "go ", "open "]
            .iter()
            .any(|prefix| folded.starts_with(prefix))
    {
        let mut ask = row(
            "ask-with",
            "Ask",
            format!("Ask: “{query}”"),
            Some("Enter"),
            Action::Ask(Some(query.into())),
        );
        ask.always = true;
        rows.push(ask);
    }
    rows.extend([
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
    ]);
    rows.extend(Destination::ALL.into_iter().map(|destination| {
        row(
            format!("go:{}", destination.slug()),
            "Go to",
            destination.label(),
            destination_key(destination),
            Action::Go(destination),
        )
    }));
    rows.extend([
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
        row(
            "pair",
            "Device",
            "Pair a new device",
            None,
            Action::Go(Destination::Settings),
        ),
        row(
            "toggle-term",
            "View",
            "Switch transcript / terminal",
            Some("`"),
            Action::ToggleSession,
        ),
    ]);
    rows.extend(SettingsSection::ALL.into_iter().map(|section| {
        row(
            format!("settings:{}", section.slug()),
            "Settings",
            format!("Settings: {}", section.label()),
            None,
            Action::Settings(section),
        )
    }));
    rows.push(row("ask", "Ask", "Ask", Some("/"), Action::Ask(None)));
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
    rows.extend(Conn::ALL.into_iter().map(|conn| {
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
    }));
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
    if data == DataState::Daemon {
        rows.retain(|row| {
            !matches!(
                row.action,
                Action::Connection(_)
                    | Action::Ask(_)
                    | Action::Mode(_)
                    | Action::Open(_)
                    | Action::ToggleSession
                    | Action::Assistant(_)
                    | Action::Hotkey
                    | Action::Share
                    | Action::Lock
            )
        });
    }
    rows
}

/// Commands matching group plus label, case-insensitively; the Ask row always survives.
pub fn filtered(query: &str, socket_configured: bool, data: DataState) -> Vec<Command> {
    let folded = query.trim().to_lowercase();
    commands(query, socket_configured, data)
        .into_iter()
        .filter(|command| {
            command.always
                || folded.is_empty()
                || format!("{} {}", command.group, command.label)
                    .to_lowercase()
                    .contains(&folded)
        })
        .collect()
}

/// Resolves a selected command without interpreting arbitrary ids as executable commands.
pub fn run(id: &str, query: &str, socket_configured: bool, data: DataState) -> Option<Action> {
    commands(query, socket_configured, data)
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
        "ask-with" => Some("Enter"),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_text_ask_survives_even_when_a_command_matches() {
        let rows = filtered("plan", false, DataState::Fixture);
        assert!(
            rows.iter()
                .any(|row| row.always && matches!(row.action, Action::Ask(Some(_))))
        );
        assert!(rows.iter().any(|row| !row.always));
        for query in ["t-894", "go now", "open task"] {
            assert!(
                !filtered(query, false, DataState::Fixture)
                    .iter()
                    .any(|row| row.always)
            );
        }
    }

    #[test]
    fn task_id_search_does_not_turn_navigation_into_a_question() {
        let rows = filtered("t-894", false, DataState::Fixture);
        assert!(
            rows.iter()
                .any(|row| row.action == Action::Open(Page::Task("T-894")))
        );
        assert!(!rows.iter().any(|row| matches!(row.action, Action::Ask(_))));
    }
}
