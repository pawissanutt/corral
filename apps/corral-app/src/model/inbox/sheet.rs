//! Pure handover descriptors and the fixture preview's builders. Band order and wording are
//! resolved here or in `live_sheet`, never in a renderer.

pub use super::sheet_types::*;
use super::{Action, Evidence, Q, QuestionState, State, assessment, phone};
use crate::model::board;
use crate::model::effect::{Conn, DataMode, Env, Page};
use crate::model::hue::Hue;
use crate::model::nav::Destination;
use crate::model::text::{Rich, span};

pub(crate) fn button(label: impl Into<String>, action: Action, kind: ButtonKind) -> Button {
    Button {
        label: label.into(),
        action,
        enabled: true,
        kind,
        shortcut: None,
    }
}

/// Resolve header chips and attribution for the active form.
pub fn qhead(_state: &State, env: Env) -> Header {
    let mut chips = vec![Chip {
        label: "needs you",
        hue: Hue::You,
    }];
    if !phone(env) {
        chips.push(Chip {
            label: Q.kind,
            hue: Hue::Neutral,
        });
    }
    chips.push(Chip {
        label: "blocking",
        hue: Hue::Fail,
    });
    if phone(env) {
        chips.push(Chip {
            label: "owner only",
            hue: Hue::Neutral,
        });
    }
    Header {
        chips,
        record: format!("{} · rev {}", Q.key, Q.rev),
        attribution: format!("from {} · {} · {} ago", Q.from, Q.via, Q.age),
        title: Q.short.into(),
    }
}

/// Build S with the queued/answered/answered-elsewhere notice stamped beneath the question.
pub fn situation(state: &State, env: Env) -> Situation {
    let notice = match state.question_state(env) {
        QuestionState::Elsewhere => Some(Notice {
            stamp: Some("ANSWERED ELSEWHERE"),
            hue: Hue::Ok,
            body: vec![
                span("Answered on the "),
                span(Q.answered_elsewhere.where_).bold(),
                span(format!(" at {} with option 1 · ", Q.answered_elsewhere.at)),
                span("on the PM pane"),
                span(". This copy is read-only; one answer wins across devices and the PM pane."),
            ],
            retry: None,
        }),
        QuestionState::Mine => Some(Notice {
            stamp: Some("ANSWERED"),
            hue: Hue::Ok,
            body: vec![
                span(format!(
                    "By you at 22:12 with option {} · ",
                    state.choice + 1
                )),
                span(format!("you on {}", env.device())),
                span(". Other devices and the PM pane now show it answered."),
            ],
            retry: None,
        }),
        QuestionState::Queued => Some(Notice {
            stamp: Some("QUEUED"),
            hue: Hue::Held,
            body: vec![span(format!(
                "Option {} waits on this device. It sends when corrald is reachable, only if rev 2 is still current; otherwise it comes back to you.",
                state.choice + 1
            ))],
            retry: None,
        }),
        _ => None,
    };
    Situation {
        attention: None,
        text: Q.text.into(),
        deadline: None,
        waits_title: "Waiting on this answer",
        waits: Q
            .waits
            .iter()
            .map(|wait| WaitLine {
                id: wait.id.into(),
                text: wait.text.into(),
                kind: wait.kind,
            })
            .collect(),
        waits_empty: None,
        notice,
    }
}

/// Build the miniature plan graph using the mockup's source geometry, not UI-derived layout.
pub fn mini_graph(compact: bool) -> MiniGraph {
    let cw = if compact { 117.0 } else { 106.0 };
    let width = if compact { 98.0 } else { 86.0 };
    let node = |x, y, id, subtitle, hue, dim| MiniNode {
        id,
        subtitle,
        position: (x, y),
        size: (width, 34.0),
        hue,
        dim,
    };
    MiniGraph {
        size: (if compact { 340.0 } else { 300.0 }, 118.0),
        label: "Position in plan T-870: T-701 done, T-867 stale gate, T-868 needs you, T-869 queued",
        nodes: vec![
            node(0.0, 8.0, "T-701", "plan P2 ✓", Hue::Neutral, false),
            node(cw, 8.0, "T-867", "gate ◌ stale", Hue::Neutral, false),
            node(2.0 * cw, 8.0, "T-868", "needs you", Hue::You, false),
            node(2.0 * cw, 65.0, "T-869", "after T-867", Hue::Neutral, true),
        ],
        edges: vec![
            MiniEdge::Line {
                from: (width, 25.0),
                to: (cw, 25.0),
            },
            MiniEdge::Cubic {
                from: (cw + width, 25.0),
                control1: (cw + 104.0, 25.0),
                control2: (cw + 104.0, 25.0),
                to: (2.0 * cw, 25.0),
            },
            MiniEdge::Cubic {
                from: (cw + width, 25.0),
                control1: (2.0 * cw - 8.0, 25.0),
                control2: (2.0 * cw - 14.0, 82.0),
                to: (2.0 * cw, 82.0),
            },
        ],
        caption: "T-870 · layer 1 of 2 · member 2 of 3 · final accept needs all 3",
    }
}

/// Build B; focus desktop deliberately uses the compact graph in its context column.
pub fn background(_state: &State, env: Env, compact: bool) -> Background {
    if env.mode == DataMode::Loading {
        return Background::Loading {
            skeleton_count: 3,
            note: "Task detail loads lazily from corrald; the question itself came with the board projection.",
        };
    }
    let task = board::task(Q.task);
    Background::Ready {
        goal: vec![
            span("T-868 host-queue-ledger").bold(),
            span(format!(" · {}", task.goal.unwrap_or(""))),
        ],
        plan_line: vec![
            span("Plan "),
            span("T-870").mono(),
            span(" Pi input: stable queue item lifecycle · governed by "),
            span("T-701@P2").mono(),
            span(" · run R1 on pi-sol xhigh, step 3 of 5"),
        ],
        graph: Some(mini_graph(compact)),
    }
}

/// Build R; read-only elsewhere uses the winning option rather than this device's choice.
pub fn recommendation(state: &State, env: Env) -> Recommendation {
    let status = state.question_state(env);
    let locked = !matches!(status, QuestionState::Open | QuestionState::Queued);
    let chosen = if status == QuestionState::Elsewhere {
        Q.answered_elsewhere.option
    } else {
        state.choice
    };
    let reason = vec![
        span("Why the worker recommends 1:").bold(),
        span(" "),
        span(Q.recommendation).dim(),
    ];
    Recommendation {
        label: "Options",
        options: Q
            .options
            .iter()
            .enumerate()
            .map(|(index, option)| AnswerOption {
                number: index + 1,
                label: option.label.into(),
                recommendation_chip: option.rec.then(|| "worker recommends".into()),
                selected: chosen == index,
                enabled: !locked,
                action: Action::Choose(index),
                consequences: vec![
                    ("Then", option.next.into()),
                    ("Unblocks", option.unblocks.into()),
                    ("Costs", option.costs.into()),
                ],
                proof: Some(button(
                    format!("see {}", option.proof),
                    Action::Evidence(if index == 0 {
                        Evidence::Diff
                    } else {
                        Evidence::File
                    }),
                    ButtonKind::Quiet,
                )),
            })
            .collect(),
        locked,
        draft_mark: "✎",
        draft_placeholder: "Write a different answer…",
        draft_key: None,
        reason: Some(reason),
    }
}

/// Build the complete thread band/sheet, including the illustrative send control.
pub fn thread_view() -> ThreadView {
    ThreadView {
        title: "Thread · worker T-868".into(),
        messages: Q
            .thread
            .iter()
            .map(|message| ThreadLine {
                who: message.who.into(),
                via: message.via.map(Into::into),
                at: message.at.into(),
                text: message.text.into(),
            })
            .collect(),
        empty: None,
        reply_placeholder: "Ask worker T-868 before answering…".into(),
        reply_label: "Follow-up to the asker",
        reply_key: None,
        reply_enabled: true,
        send: button("Ask", Action::SendThread, ButtonKind::Ghost),
        attribution: Some("thread messages illustrative"),
        status: None,
        done: button("Done", Action::CloseThread, ButtonKind::Quiet),
    }
}

/// Build the history labels without requiring renderer knowledge of “who”.
pub fn history() -> Vec<HistoryRow> {
    Q.history
        .iter()
        .map(|entry| HistoryRow {
            label: format!(
                "{} · {}",
                entry.at,
                if entry.who == "you" {
                    "your decision"
                } else {
                    "tried"
                }
            ),
            owner_decision: entry.who == "you",
            text: entry.text.into(),
        })
        .collect()
}

/// Build the pinned controls exactly as the source: queued remains labelled as an answer,
/// while its reducer ignores repeat submissions; offline and down both say “Queue answer”.
pub fn decide_bar(state: &State, env: Env) -> DecideBar {
    let status = state.question_state(env);
    if matches!(status, QuestionState::Elsewhere | QuestionState::Mine) {
        if phone(env) {
            return DecideBar {
                buttons: vec![
                    button(
                        "Back to Inbox",
                        Action::Go(Destination::Inbox),
                        ButtonKind::Ghost,
                    ),
                    button("Next · T-885 plan", Action::NextQ, ButtonKind::Primary),
                ],
                stamp: None,
                note: vec![],
                submit: None,
            };
        }
        return DecideBar {
            buttons: vec![button(
                "Watch T-868 resume",
                Action::Open(Page::Session(Q.task)),
                ButtonKind::Ghost,
            )],
            stamp: Some((
                if status == QuestionState::Elsewhere {
                    "ANSWERED ON PM PANE 22:14".into()
                } else {
                    "ANSWERED 22:12".into()
                },
                Hue::Ok,
            )),
            note: vec![span(
                "One answer wins: the PM pane, this app and every paired device read the same question record.",
            )],
            submit: None,
        };
    }
    let off = matches!(env.conn, Conn::Offline | Conn::Down);
    let target = if state.draft.is_empty() {
        format!("option {}", state.choice + 1)
    } else {
        "your text".into()
    };
    let mut answer = button(
        format!("{} · {target}", if off { "Queue answer" } else { "Answer" }),
        Action::Answer,
        ButtonKind::Primary,
    );
    if phone(env) {
        return DecideBar {
            buttons: vec![
                button("Ask worker", Action::Thread, ButtonKind::Ghost),
                answer,
            ],
            stamp: None,
            note: vec![],
            submit: Some(Action::Answer),
        };
    }
    answer.shortcut = Some("Ctrl ↵");
    let mut note = if off {
        vec![span(
            "Offline: answers queue with their revision; plan accepts and grants refuse.",
        )]
    } else {
        vec![
            span("Runs "),
            span(format!("corral question answer --owner {} --rev 2", Q.key)).mono(),
        ]
    };
    note.push(span("\n"));
    note.push(span(format!("you on {}", env.device())));
    DecideBar {
        buttons: vec![
            answer,
            button(
                "Ask the worker",
                Action::Toast("Focus moved to the thread reply box"),
                ButtonKind::Ghost,
            ),
            button(
                "Discuss with PM",
                Action::Toast("Opened in the PM conversation"),
                ButtonKind::Quiet,
            ),
        ],
        stamp: None,
        note,
        submit: Some(Action::Answer),
    }
}

pub(crate) fn band(
    kind: BandKind,
    letter: &'static str,
    title: &'static str,
    detail: Option<String>,
) -> Band {
    Band {
        kind,
        letter,
        title,
        detail,
    }
}

/// Desktop inline order is S, B, A, R, Thread, History. Focus mode moves B, Thread,
/// History to the side and leaves S, A, R in the centre so the choices stay visible.
pub(crate) fn desk_bands(sheet: &mut Sheet, with_side: bool, plan: Option<String>) {
    let threads = sheet.thread.messages.len().to_string();
    sheet.bands = vec![band(BandKind::Situation, "S", "Situation", None)];
    if !with_side {
        sheet
            .bands
            .push(band(BandKind::Background, "B", "Background", None));
    }
    sheet.bands.extend([
        band(BandKind::Assessment, "A", "Assessment", None),
        band(BandKind::Recommendation, "R", "Recommendation", None),
    ]);
    if with_side {
        sheet.side_bands = vec![
            band(BandKind::Background, "B", "Background · place", plan),
            band(
                BandKind::Thread,
                "T",
                "Thread with the asker",
                Some(threads),
            ),
            band(
                BandKind::History,
                "H",
                "History",
                Some("tried · your decisions".into()),
            ),
        ];
    } else {
        sheet.bands.extend([
            band(BandKind::Thread, "T", "Thread", None),
            band(BandKind::History, "H", "History", None),
        ]);
    }
}

/// Phone order is S, R, A, B, Thread, History; its thread is a collapsed preview opening
/// the shared sheet.
pub(crate) fn phone_bands(sheet: &mut Sheet) {
    let threads = sheet.thread.messages.len();
    sheet.bands = vec![
        band(BandKind::Situation, "S", "Situation", None),
        band(
            BandKind::Recommendation,
            "R",
            "Options",
            Some("consequence of each".into()),
        ),
        band(BandKind::Assessment, "A", "Evidence", None),
        band(BandKind::Background, "B", "Place", None),
        band(
            BandKind::Thread,
            "T",
            "Thread with the asker",
            Some(format!("{threads} · Ask ›")),
        ),
        band(BandKind::History, "H", "History", None),
    ];
}

/// The fixture's inline or focus desktop handover.
pub fn fixture_desk(state: &State, env: Env, with_side: bool) -> Sheet {
    let mut sheet = build_sheet(state, env, with_side);
    desk_bands(&mut sheet, with_side, Some("T-870".into()));
    sheet
}

/// The fixture's focus page in every form.
pub fn fixture_question(state: &State, env: Env) -> Sheet {
    if !phone(env) {
        return fixture_desk(state, env, true);
    }
    let mut sheet = build_sheet(state, env, true);
    phone_bands(&mut sheet);
    sheet
}

/// The keys line on a desktop focus page.
pub(crate) fn key_help(choose: &str) -> Rich {
    vec![
        span("Keys "),
        span(choose.to_owned()).mono(),
        span(" choose · "),
        span("Ctrl ↵").mono(),
        span(" answer · "),
        span("Esc").mono(),
        span(" back"),
    ]
}

fn build_sheet(state: &State, env: Env, focus: bool) -> Sheet {
    let status = state.question_state(env);
    let preview = Q
        .thread
        .get(1)
        .map(|message| {
            vec![
                span(format!("{}:", message.who)).bold(),
                span(format!(" {}", message.text)),
            ]
        })
        .unwrap_or_default();
    Sheet {
        title: "Question",
        header: qhead(state, env),
        state: status,
        answered: !matches!(status, QuestionState::Open | QuestionState::Queued),
        bands: Vec::new(),
        side_bands: Vec::new(),
        situation: situation(state, env),
        background: background(state, env, phone(env) || focus),
        assessment: assessment(state, env),
        recommendation: recommendation(state, env),
        thread: thread_view(),
        thread_preview: preview,
        thread_action: Action::Thread,
        thread_open: state.thread_open,
        history: history(),
        history_empty: None,
        decide: decide_bar(state, env),
        back: focus.then(|| button("Inbox", Action::Go(Destination::Inbox), ButtonKind::Quiet)),
        pager: focus.then(|| "1 of 5 needing you".into()),
        next: focus.then(|| {
            button(
                if phone(env) {
                    "Next"
                } else {
                    "Next · T-885 plan"
                },
                Action::NextQ,
                ButtonKind::Ghost,
            )
        }),
        key_help: (focus && !phone(env)).then(|| key_help("1 2")),
        ask: focus.then(|| {
            button(
                if phone(env) {
                    "Ask the assistant about this"
                } else {
                    "Ask about this"
                },
                Action::Ask,
                ButtonKind::Ghost,
            )
        }),
    }
}
