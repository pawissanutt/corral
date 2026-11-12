//! Agents wall and session fixtures, state and presentation data.
//!
//! The assistant module supplies the quick thread before the agent tiles, and the
//! sessions and Filed from chat blocks after them, on both desktop and phone.

use crate::model::board::{self, Mark, Rail};
use crate::model::effect::{Effect, Env, Page};
use crate::model::hue::Hue;
use crate::model::nav::{Destination, Form};
use crate::model::text::{Rich, span};

mod data;

pub use data::AGENTS;

/// The agent's current activity, used for the wall chip and phone dot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    Live,
    Working,
    Asking,
    Gating,
    Idle,
}

impl AgentStatus {
    /// Chip text and state colour.
    pub fn chip(self) -> (&'static str, Hue) {
        match self {
            Self::Live => ("live", Hue::Corral),
            Self::Working => ("working", Hue::Worker),
            Self::Asking => ("needs you", Hue::You),
            Self::Gating => ("gating", Hue::Corral),
            Self::Idle => ("idle", Hue::Neutral),
        }
    }
}

/// A live stream or a periodically refreshed, read-only legacy snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feed {
    Live,
    Snapshot,
}

impl Feed {
    /// Short phone label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Snapshot => "snapshot",
        }
    }
    /// Wall tile label.
    pub fn wall_label(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Snapshot => "snapshot · 20s ago",
        }
    }
}

/// One agent on the workstation, including its complete illustrative tail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Agent {
    pub id: &'static str,
    pub role: &'static str,
    pub task: Option<&'static str>,
    pub title: &'static str,
    pub profile: &'static str,
    pub runtime: &'static str,
    pub pane: &'static str,
    pub state: AgentStatus,
    pub doing: &'static str,
    pub since: &'static str,
    pub machine: &'static str,
    pub tail: &'static [&'static str],
    pub feed: Feed,
}

/// The selected session surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum View {
    #[default]
    Transcript,
    Terminal,
}
/// The wall/list setting; the mockup's list button has no action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Wall,
    List,
}
/// Session and wall selection, retained across navigation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    pub session: &'static str,
    pub view: View,
    pub mode: Mode,
}
impl Default for State {
    fn default() -> Self {
        Self {
            session: "T-868",
            view: View::Transcript,
            mode: Mode::Wall,
        }
    }
}
impl State {
    /// Select a session without resetting its transcript/terminal choice.
    pub fn open(&mut self, id: &'static str) {
        self.session = id;
    }
    /// Toggle transcript and terminal.
    pub fn toggle_view(&mut self) {
        self.view = match self.view {
            View::Transcript => View::Terminal,
            View::Terminal => View::Transcript,
        };
    }
    /// Handle an action from either surface.
    pub fn act(&mut self, action: Action, env: Env) -> Vec<Effect> {
        match action {
            Action::Open(id) => {
                self.open(id);
                vec![Effect::Open(Page::Session(id))]
            }
            Action::Question => vec![Effect::Open(Page::Question)],
            Action::Back => vec![Effect::Go(Destination::Agents)],
            Action::SetView(view) => {
                self.view = view;
                vec![]
            }
            Action::SetMode(mode) => {
                self.mode = mode;
                vec![]
            }
            Action::Key(_) => vec![],
            Action::ToggleView => {
                self.toggle_view();
                vec![]
            }
            Action::Tool => vec![Effect::Toast("Tool call expanded".into())],
            Action::Send => {
                let id = effective_id(self.session);
                let text = if env.form == Form::Phone {
                    "Sent · via=app:pawis-iphone".to_string()
                } else {
                    format!("Sent to {id} via corral send · actor=owner via=app:pawis-desktop")
                };
                vec![Effect::Toast(text)]
            }
        }
    }
}
/// Actions attached to tiles, controls, tool calls and the reply composer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Open(&'static str),
    Question,
    Back,
    SetView(View),
    SetMode(Mode),
    Key(&'static str),
    ToggleView,
    Tool,
    Send,
}

/// Terminal line classification, including prompt and success lines in the wall tails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineClass {
    Prompt,
    Ok,
    Dim,
    Bold,
    Plain,
}
/// One terminal line, with inline styling where the mockup mixes styles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub text: Rich,
    pub class: LineClass,
}
fn line(text: &str, class: LineClass) -> Line {
    Line {
        text: vec![span(text)],
        class,
    }
}
fn tail_line(text: &str) -> Line {
    let class = if text.contains("PASS") || text.contains("ok ") || text.contains("pass") {
        LineClass::Ok
    } else if ["$", "Edit", "Bash", "Read", "wake", "read", "Ask"]
        .iter()
        .any(|p| text.starts_with(p))
    {
        LineClass::Prompt
    } else {
        LineClass::Plain
    };
    line(text, class)
}
/// One wall tile or phone row, with its actionable session and last four tail lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tile {
    pub agent: Agent,
    pub since: String,
    pub chip: &'static str,
    pub hue: Hue,
    pub doing_hue: Hue,
    pub tail: Vec<Line>,
    pub action: Action,
}
/// Agents page content; quick thread and assistant blocks are provided by the assistant module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wall {
    pub title: &'static str,
    pub subtitle: &'static str,
    pub machine: &'static str,
    pub machine_detail: &'static str,
    pub machine_note: &'static str,
    pub other_machines: &'static str,
    pub phone_machine_detail: &'static str,
    pub phone_machine_note: &'static str,
    pub count: usize,
    pub tiles: Vec<Tile>,
    pub footer: &'static str,
    pub mode: Mode,
    pub mode_labels: [&'static str; 2],
}
/// Build the wall and phone rows from the snapshot.
pub fn wall(state: &State, _env: Env) -> Wall {
    Wall {
        title: "Agents",
        subtitle: "the quick thread first, then the PM, workers, gates and assistant sessions · what each is doing now, and where",
        machine: "workstation",
        machine_detail: "daemon host · herdr-core 0.1 · 6 agents · Herdr TUI (legacy) · 1 agent",
        machine_note: "machine name illustrative",
        other_machines: "Other machines join over SSH later (Settings › Machines)",
        phone_machine_detail: "daemon host",
        phone_machine_note: "name illustrative",
        count: AGENTS.len(),
        tiles: AGENTS
            .iter()
            .map(|a| {
                let (chip, hue) = a.state.chip();
                Tile {
                    agent: *a,
                    since: format!("since {}", a.since),
                    chip,
                    hue,
                    doing_hue: if a.state == AgentStatus::Asking {
                        Hue::You
                    } else {
                        Hue::Neutral
                    },
                    tail: a.tail[a.tail.len().saturating_sub(4)..]
                        .iter()
                        .map(|s| tail_line(s))
                        .collect(),
                    action: Action::Open(a.task.unwrap_or("PM")),
                }
            })
            .collect(),
        footer: "Tails are illustrative; agents, panes, profiles and states follow the 22:12 snapshot.",
        mode: state.mode,
        mode_labels: ["Wall", "List"],
    }
}

/// A tool call row; expanded output is fixed in the fixture, and pressing it shows a toast.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tool {
    pub kind: &'static str,
    pub argument: &'static str,
    pub result: &'static str,
    pub mark: Mark,
    pub output: Vec<Line>,
    pub action: Action,
}
/// A paragraph, tool call or inline handover within a transcript message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageItem {
    Paragraph(Rich),
    Tool(Tool),
    Question(QuestionCard),
}
/// Inline owner-only blocking handover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionCard {
    pub label: &'static str,
    pub question: &'static str,
    pub button: &'static str,
    pub hint: &'static str,
    pub action: Action,
}
/// One transcript message, with avatar and ordered content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub avatar: &'static str,
    pub hue: Hue,
    pub head: Rich,
    pub author: String,
    pub detail: &'static str,
    pub items: Vec<MessageItem>,
}
/// Session transcript, including the illustrative disclaimer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transcript {
    pub messages: Vec<Message>,
    pub disclaimer: &'static str,
}
fn paragraph(text: &str) -> MessageItem {
    MessageItem::Paragraph(vec![span(text)])
}
fn tool(
    kind: &'static str,
    argument: &'static str,
    result: &'static str,
    output: &[&str],
) -> MessageItem {
    MessageItem::Tool(Tool {
        kind,
        argument,
        result: if result == "fail" { "failed" } else { result },
        mark: if result == "fail" {
            Mark::Fail
        } else {
            Mark::Done
        },
        output: output.iter().map(|s| tail_line(s)).collect(),
        action: Action::Tool,
    })
}
fn message(
    avatar: &'static str,
    author: &str,
    detail: &'static str,
    items: Vec<MessageItem>,
) -> Message {
    let hue = match avatar {
        "C" => Hue::Corral,
        "Y" => Hue::You,
        _ => Hue::Worker,
    };
    let mut head = vec![span(author).bold()];
    if let Some(via) = detail.strip_prefix("21:58 · ") {
        head.push(span(" · 21:58 · ").dim());
        head.push(span(via).mono().dim());
    } else if detail.starts_with("asked you") {
        head.push(span(format!(" {detail}")).dim());
    } else {
        head.push(span(format!(" · {detail}")).dim());
    }
    Message {
        avatar,
        hue,
        head,
        author: author.into(),
        detail,
        items,
    }
}
fn transcript(id: &'static str) -> Transcript {
    if id != "T-868" {
        let a = AGENTS
            .iter()
            .find(|a| a.task == Some(id))
            .unwrap_or(&AGENTS[0]);
        let mut items = vec![paragraph(a.doing)];
        for entry in a.tail {
            if let Some((kind, arg)) = entry.split_once(' ') {
                if matches!(kind, "Edit" | "Bash" | "Read") {
                    items.push(tool(kind, arg.trim_start(), "ok", &[]));
                }
            }
        }
        return Transcript {
            messages: vec![message("W", &format!("worker {}", a.id), a.since, items)],
            disclaimer: "transcript illustrative",
        };
    }
    Transcript {
        messages: vec![
            message(
                "C",
                "Corral",
                "18:58",
                vec![MessageItem::Paragraph(vec![
                    span("Run R1 admitted on pi-sol xhigh · governed by ").dim(),
                    span("T-701@P2").mono().dim(),
                    span(" · write scope 20 paths · herdr-core pane ").dim(),
                    span("w12C:p2").mono().dim(),
                ])],
            ),
            message(
                "W",
                "worker T-868",
                "18:59",
                vec![
                    paragraph(
                        "Verifying installed Pi package metadata is 1.5.0 and refreshing its frozen dependencies in this worktree.",
                    ),
                    tool("Bash", "bun install --frozen-lockfile", "ok 6.2s", &[]),
                    tool(
                        "Read",
                        "packages/corral-pi/patches/@earendil-works%2Fpi-coding-agent@1.5.0.patch",
                        "612 lines",
                        &[],
                    ),
                ],
            ),
            message(
                "W",
                "worker T-868",
                "19:08",
                vec![
                    MessageItem::Paragraph(vec![
                        span("Stable identities are in: "),
                        span("q-g<generation>-<bind>-<counter>").mono(),
                        span(". Lifecycle entries persist before any queue mutation."),
                    ]),
                    tool(
                        "Edit",
                        "packages/corral-pi/src/host/queue.ts",
                        "+41 −6",
                        &[],
                    ),
                    tool(
                        "Bash",
                        "bun test test/queue-host.test.ts",
                        "16 pass",
                        &[
                            "$ bun test test/queue-host.test.ts",
                            " ✓ withdraw targets one identity",
                            " ✓ start race: started persists before drain",
                            " ✓ replay returns stored outcome",
                            " 16 pass · 0 fail · 1.64s",
                        ],
                    ),
                ],
            ),
            message(
                "W",
                "worker T-868",
                "asked you · 19:12 · escalated by the PM 19:14",
                vec![
                    MessageItem::Question(QuestionCard {
                        label: "T-868/R1/Q1 · owner only · blocking",
                        question: "Restore policy: pane-origin input only, or all origins?",
                        button: "Open handover · decide",
                        hint: "2 options · worker recommends 1",
                        action: Action::Question,
                    }),
                    paragraph(
                        "I can implement the ledger and other controls independently while awaiting this decision.",
                    ),
                ],
            ),
            message(
                "W",
                "worker T-868",
                "19:40",
                vec![
                    tool(
                        "Edit",
                        "packages/corral-pi/src/bridge/queue.ts",
                        "+88 −3",
                        &[],
                    ),
                    tool("Bash", "bun run --bun typecheck", "ok 11.0s", &[]),
                ],
            ),
            message(
                "Y",
                "you",
                "21:58 · via=app:pawis-iphone",
                vec![paragraph("Does option 1 change anything in rich mode?")],
            ),
            message(
                "W",
                "worker T-868",
                "21:59",
                vec![
                    paragraph(
                        "No. Rich mode stays read-only/count-only; neither option restores there.",
                    ),
                    MessageItem::Paragraph(vec![
                        span("idle since 21:40 · waiting on T-868/R1/Q1").dim(),
                    ]),
                ],
            ),
        ],
        disclaimer: "Transcript illustrative; the question, its options and the tool targets are real.",
    }
}

/// Reply composer copy and send action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub placeholder: String,
    pub accessibility_label: &'static str,
    pub button: &'static str,
    pub hint: Rich,
    pub question_hint: &'static str,
    pub action: Action,
}
/// One labeled fact in the desktop run sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunFact {
    pub label: &'static str,
    pub value: FactValue,
}
/// A fact is either styled text or the task's rail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FactValue {
    Text(Rich),
    Rail(Rail),
}
/// Desktop run sidebar, including the task's rail and ordered steps.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub title: &'static str,
    pub task_id: &'static str,
    pub task_title: &'static str,
    pub rail: Rail,
    pub agent: &'static str,
    pub runtime: &'static str,
    pub pane: &'static str,
    pub machine: &'static str,
    pub started: &'static str,
    pub facts: Vec<RunFact>,
    pub steps_title: &'static str,
    pub steps: &'static [(Mark, &'static str)],
}
/// Terminal toolbar and complete lines. Snapshot data must never be presented as a stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Terminal {
    pub feed: Feed,
    pub heading: String,
    pub status: &'static str,
    pub input: &'static str,
    pub lines: Vec<Line>,
    pub cursor: bool,
    pub phone_keys: [&'static str; 6],
}
/// Session header and selected surface; both surfaces are supplied for switching.
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub id: &'static str,
    pub title: &'static str,
    pub agent: &'static str,
    pub run: &'static str,
    pub sub: String,
    pub switch_key: &'static str,
    pub switch_hint: &'static str,
    pub machine: &'static str,
    pub phone_title: String,
    pub back_label: &'static str,
    pub desktop_back_label: &'static str,
    pub back: Action,
    pub view: View,
    pub switch_labels: [&'static str; 2],
    pub switch_actions: [Action; 2],
    pub toggle: Action,
    pub transcript: Transcript,
    pub terminal: Terminal,
    pub reply: Reply,
    pub run_details: Run,
}
fn effective_id(id: &'static str) -> &'static str {
    if id == "PM" { "T-868" } else { id }
}
fn terminal_lines(id: &'static str) -> Vec<Line> {
    if id != "T-868" {
        let a = AGENTS
            .iter()
            .find(|a| a.task == Some(id))
            .unwrap_or(&AGENTS[0]);
        return a.tail.iter().map(|s| tail_line(s)).collect();
    }
    let mut lines = vec![
        line(
            "pi 1.5.0 · corral bound session · T-868 R1 · pi-sol xhigh",
            LineClass::Dim,
        ),
        line(
            "───────────────────────────────────────────────────────────",
            LineClass::Dim,
        ),
        line(
            "● Edit packages/corral-pi/src/bridge/queue.ts",
            LineClass::Bold,
        ),
    ];
    lines.push(Line {
        text: vec![
            span("  "),
            span("+88").hue(Hue::Ok),
            span(" "),
            span("-3").hue(Hue::Fail),
        ],
        class: LineClass::Plain,
    });
    lines.push(line("● Bash bun run --bun typecheck", LineClass::Bold));
    lines.push(Line {
        text: vec![span("  "), span("ok").hue(Hue::Ok), span(" 11.0s")],
        class: LineClass::Plain,
    });
    lines.extend([
        line("", LineClass::Plain),
        Line {
            text: vec![span("? T-868/R1/Q1 · waiting on the owner").hue(Hue::You)],
            class: LineClass::Plain,
        },
        line(
            "  Restore policy: pane-origin input only, or all origins?",
            LineClass::Plain,
        ),
        line(
            "  1. Restore only pane-origin input; leave Corral/extension items queued",
            LineClass::Plain,
        ),
        line(
            "  2. Retain restore-all; journal removals, resubmit as new pane input",
            LineClass::Plain,
        ),
        line("", LineClass::Plain),
        Line {
            text: vec![
                span("owner (app:pawis-iphone):").dim(),
                span(" Does option 1 change anything in rich mode?"),
            ],
            class: LineClass::Plain,
        },
        line(
            "● No. Rich mode stays read-only/count-only; neither option restores there.",
            LineClass::Bold,
        ),
        line("", LineClass::Plain),
        line(
            "queued  0 steering · 0 follow-ups · queued, not execution",
            LineClass::Dim,
        ),
        line("›", LineClass::Prompt),
    ]);
    lines
}
/// Build the session header, transcript, reply and terminal for the selected session.
pub fn session(state: &State, _env: Env) -> Session {
    let id = effective_id(state.session);
    let task = board::task(id);
    let feed = if id == "T-854" {
        Feed::Snapshot
    } else {
        Feed::Live
    };
    let runtime = if feed == Feed::Snapshot {
        "Herdr TUI (legacy)"
    } else {
        "herdr-core"
    };
    let pane = task.pane.unwrap_or("w12C:p2");
    Session {
        id,
        title: task.title,
        agent: task.agent.unwrap_or(""),
        run: "worker R1",
        sub: format!("worker R1 · {}", task.agent.unwrap_or("")),
        switch_key: "`",
        switch_hint: "switch",
        machine: "workstation",
        phone_title: format!("{id} session"),
        back_label: "Back",
        desktop_back_label: "Agents",
        back: Action::Back,
        view: state.view,
        switch_labels: ["Transcript", "Terminal"],
        switch_actions: [
            Action::SetView(View::Transcript),
            Action::SetView(View::Terminal),
        ],
        toggle: Action::ToggleView,
        transcript: transcript(id),
        terminal: Terminal {
            feed,
            heading: format!("{runtime} · {pane} · 140×46"),
            status: if feed == Feed::Snapshot {
                "snapshot · refreshed 20s ago · no live stream on the legacy runtime"
            } else {
                "live stream · scrollback 1,000"
            },
            input: if feed == Feed::Snapshot {
                "read-only snapshot"
            } else {
                "keys go to the agent's pane as owner input · actor=owner"
            },
            lines: terminal_lines(id),
            cursor: true,
            phone_keys: ["Esc", "Tab", "Ctrl", "↑", "↓", "⏎"],
        },
        reply: Reply {
            placeholder: format!("Message worker {id}…"),
            accessibility_label: "Reply to agent",
            button: "Send",
            hint: vec![
                span("Sends with "),
                span(format!("corral send {id}")).mono(),
                span(" · arrives as owner input"),
            ],
            question_hint: "Answering the open question happens on its handover sheet",
            action: Action::Send,
        },
        run_details: Run {
            title: "Run",
            task_id: id,
            task_title: task.title,
            rail: task.rail,
            agent: task.agent.unwrap_or("pi-sol xhigh"),
            runtime: "herdr-core",
            pane: task.pane.unwrap_or("—"),
            machine: "workstation",
            started: task.started.unwrap_or("—"),
            facts: vec![
                RunFact {
                    label: "Task",
                    value: FactValue::Text(vec![span(id).mono(), span(" "), span(task.title)]),
                },
                RunFact {
                    label: "Rail",
                    value: FactValue::Rail(task.rail),
                },
                RunFact {
                    label: "Agent",
                    value: FactValue::Text(vec![span(task.agent.unwrap_or("pi-sol xhigh"))]),
                },
                RunFact {
                    label: "Runtime",
                    value: FactValue::Text(vec![
                        span("herdr-core · "),
                        span(task.pane.unwrap_or("—")).mono(),
                    ]),
                },
                RunFact {
                    label: "Machine",
                    value: FactValue::Text(vec![span("workstation")]),
                },
                RunFact {
                    label: "Started",
                    value: FactValue::Text(vec![span(task.started.unwrap_or("—"))]),
                },
            ],
            steps_title: "Steps",
            steps: task.steps,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{State, View};
    #[test]
    fn toggling_twice_does_not_leave_terminal_selected() {
        let mut state = State::default();
        state.toggle_view();
        state.toggle_view();
        assert_eq!(state.view, View::Transcript);
    }
    #[test]
    fn opening_another_session_does_not_reset_view() {
        let mut state = State::default();
        state.toggle_view();
        state.open("T-854");
        assert_eq!(state.view, View::Terminal);
    }
}
