//! The mockup preview of Agents: the fixture wall, and the T-868 session with its illustrative
//! transcript and terminal. Used only while the fixture data state is selected.

use crate::model::board::{self, Mark};
use crate::model::effect::Env;
use crate::model::hue::Hue;
use crate::model::text::span;

use super::{
    AGENTS, Action, AgentStatus, FactValue, Feed, Fill, Line, LineClass, Message, MessageItem,
    Note, QuestionCard, Reply, ReplyMode, Run, RunFact, Session, State, Terminal, Tile, Tool,
    Transcript, View, Wall, line, tail_line,
};

/// The session a fixture id stands for: the PM tile opens T-868's session in the mockup.
pub(super) fn effective_id(id: &'static str) -> &'static str {
    if id == "PM" { "T-868" } else { id }
}

/// Build the wall and phone rows from the fixture snapshot.
pub fn wall(state: &State, _env: Env) -> Wall {
    Wall {
        title: "Agents",
        subtitle: "the quick thread first, then the PM, workers, gates and assistant sessions · what each is doing now, and where",
        machine: "workstation".into(),
        machine_detail: "daemon host · herdr-core 0.1 · 6 agents · Herdr TUI (legacy) · 1 agent"
            .into(),
        machine_note: Some("machine name illustrative"),
        other_machines: "Other machines join over SSH later (Settings › Machines)",
        phone_machine_detail: "daemon host".into(),
        phone_machine_note: Some("name illustrative"),
        count: AGENTS.len(),
        tiles: AGENTS
            .iter()
            .map(|a| {
                let (chip, hue) = a.state.chip();
                Tile {
                    id: a.id.into(),
                    label: a.id.into(),
                    role: a.role.into(),
                    title: a.title.into(),
                    doing: a.doing.into(),
                    profile: a.profile.into(),
                    runtime: a.runtime.into(),
                    pane: a.pane.into(),
                    usage: None,
                    feed: a.feed,
                    feed_label: a.feed.wall_label().into(),
                    feed_short: a.feed.label().into(),
                    since: format!("since {}", a.since),
                    since_short: a.since.into(),
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
        fill: Fill::Ready,
        stale: None,
        footer: Note::Illustrative(
            "Tails are illustrative; agents, panes, profiles and states follow the 22:12 snapshot.",
        ),
        mode: state.mode,
        mode_labels: ["Wall", "List"],
    }
}

fn paragraph(text: &str) -> MessageItem {
    MessageItem::Paragraph(vec![span(text)])
}
fn tool(kind: &str, argument: &str, result: &str, output: &[&str]) -> MessageItem {
    MessageItem::Tool(Tool {
        kind: kind.into(),
        argument: argument.into(),
        result: if result == "fail" { "failed" } else { result }.into(),
        mark: if result == "fail" {
            Mark::Fail
        } else {
            Mark::Done
        },
        output: output.iter().map(|s| tail_line(s)).collect(),
        expanded: None,
        action: Action::Tool,
    })
}
fn message(avatar: &'static str, author: &str, detail: &str, items: Vec<MessageItem>) -> Message {
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
        detail: detail.into(),
        items,
    }
}
fn transcript(id: &'static str) -> Transcript {
    let shell = |messages, disclaimer| Transcript {
        messages,
        older: None,
        newer: None,
        paging: false,
        fill: Fill::Ready,
        stale: None,
        disclaimer: Some(Note::Illustrative(disclaimer)),
    };
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
        return shell(
            vec![message("W", &format!("worker {}", a.id), a.since, items)],
            "transcript illustrative",
        );
    }
    shell(
        vec![
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
                        label: "T-868/R1/Q1 · owner only · blocking".into(),
                        question: "Restore policy: pane-origin input only, or all origins?".into(),
                        button: "Open handover · decide",
                        hint: "2 options · worker recommends 1".into(),
                        open: true,
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
        "Transcript illustrative; the question, its options and the tool targets are real.",
    )
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

/// Build the fixture session header, transcript, reply and terminal for the selected session.
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
        id: id.into(),
        title: task.title.into(),
        sub: format!("worker R1 · {}", task.agent.unwrap_or("")),
        switch_key: "`",
        switch_hint: "switch",
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
        notice: None,
        transcript: transcript(id),
        terminal: Some(Terminal {
            feed,
            heading: format!("{runtime} · {pane} · 140×46"),
            status: if feed == Feed::Snapshot {
                "snapshot · refreshed 20s ago · no live stream on the legacy runtime"
            } else {
                "live stream · scrollback 1,000"
            }
            .into(),
            input: if feed == Feed::Snapshot {
                "read-only snapshot"
            } else {
                "keys go to the agent's pane as owner input · actor=owner"
            }
            .into(),
            lines: terminal_lines(id),
            cursor: true,
            verbatim: false,
            empty: None,
            phone_keys: vec!["Esc", "Tab", "Ctrl", "↑", "↓", "⏎"],
            refresh: None,
        }),
        reply: Reply {
            placeholder: format!("Message worker {id}…"),
            accessibility_label: "Reply to agent",
            button: "Send",
            hint: vec![
                span("Sends with "),
                span(format!("corral send {id}")).mono(),
                span(" · arrives as owner input"),
            ],
            question_hint: "Answering the open question happens on its handover sheet".into(),
            mode: ReplyMode::Open,
            status: None,
            recover: None,
            delivered: None,
        },
        run_details: Run {
            title: "Run",
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
            steps: task
                .steps
                .iter()
                .map(|&(mark, words)| (mark, words.to_owned()))
                .collect(),
        },
    }
}
