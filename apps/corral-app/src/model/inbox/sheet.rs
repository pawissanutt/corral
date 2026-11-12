//! Pure handover descriptors. Band order and wording are resolved here, not in a renderer.

use super::{
    Action, Evidence, HistoryEntry, Message, Q, QuestionState, State, Wait, assessment, phone,
};
use crate::model::board::{self, Hunk, LineKind};
use crate::model::effect::{Conn, DataMode, Env, Page};
use crate::model::hue::Hue;
use crate::model::nav::Destination;
use crate::model::text::{Rich, span};

/// A labelled state chip in a header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chip {
    /// Verbatim chip text.
    pub label: &'static str,
    /// State colour, never selection colour.
    pub hue: Hue,
}

/// The source's three button emphasis levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    /// Main action.
    Primary,
    /// Secondary outlined action.
    Ghost,
    /// Low-emphasis text action.
    Quiet,
}

/// A fully labelled control; dispatch its action rather than interpreting its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Button {
    /// Visible text, already resolved for form and state.
    pub label: String,
    /// Reducer action.
    pub action: Action,
    /// Whether the renderer accepts activation.
    pub enabled: bool,
    /// Source emphasis.
    pub kind: ButtonKind,
    /// Display-only shortcut hint, when present.
    pub shortcut: Option<&'static str>,
}

pub(crate) fn button(label: impl Into<String>, action: Action, kind: ButtonKind) -> Button {
    Button {
        label: label.into(),
        action,
        enabled: true,
        kind,
        shortcut: None,
    }
}

/// A stamped answer or a recoverable error with all inline attribution preserved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// Optional uppercase stamp.
    pub stamp: Option<&'static str>,
    /// State colour of the notice.
    pub hue: Hue,
    /// Complete notice body, including bold/mono runs.
    pub body: Rich,
    /// Retry control, when the source offers one.
    pub retry: Option<Button>,
}

/// Question title and its complete metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    /// Ordered chips; phone uses owner-only where desktop has the kind chip.
    pub chips: Vec<Chip>,
    /// Mono question key and revision.
    pub record: String,
    /// Worker, escalation and age line.
    pub attribution: String,
    /// Full title.
    pub title: &'static str,
}

/// S: the decision, affected work and optional immutable answer stamp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Situation {
    /// The complete question.
    pub text: &'static str,
    /// Dependency-list heading.
    pub waits_title: &'static str,
    /// Waiting tasks, including the explicitly independent task.
    pub waits: &'static [Wait],
    /// Answered, answered-elsewhere or queued stamp, absent when open.
    pub notice: Option<Notice>,
}

/// A card in the miniature plan graph, in source SVG units.
#[derive(Debug, Clone, PartialEq)]
pub struct MiniNode {
    /// Task id displayed in the card.
    pub id: &'static str,
    /// Card subtitle.
    pub subtitle: &'static str,
    /// Upper-left position in source units.
    pub position: (f32, f32),
    /// Card dimensions in source units.
    pub size: (f32, f32),
    /// State colour of the node.
    pub hue: Hue,
    /// Whether the source dims the queued node.
    pub dim: bool,
}

/// A miniature dependency edge, preserving the source's line or cubic geometry.
#[derive(Debug, Clone, PartialEq)]
pub enum MiniEdge {
    /// Horizontal segment with its endpoints.
    Line { from: (f32, f32), to: (f32, f32) },
    /// Cubic segment with two controls and endpoints.
    Cubic {
        from: (f32, f32),
        control1: (f32, f32),
        control2: (f32, f32),
        to: (f32, f32),
    },
}

/// The position-in-plan graph, with its accessibility name and caption.
#[derive(Debug, Clone, PartialEq)]
pub struct MiniGraph {
    /// Source view-box width and height.
    pub size: (f32, f32),
    /// Full textual alternative.
    pub label: &'static str,
    /// Nodes in source order.
    pub nodes: Vec<MiniNode>,
    /// Dependencies in source order.
    pub edges: Vec<MiniEdge>,
    /// Plan, layer, member and final-accept line.
    pub caption: &'static str,
}

/// B: lazy task detail or the task's goal and position in the plan.
#[derive(Debug, Clone, PartialEq)]
pub enum Background {
    /// Task detail is independent of the board-projected question.
    Loading {
        /// Number of skeleton blocks.
        skeleton_count: u8,
        /// Explanation below the placeholders.
        note: &'static str,
    },
    /// Loaded task and plan context.
    Ready {
        /// Bold task title followed by the task goal.
        goal: Rich,
        /// Plan, governing revision, run and step metadata.
        plan_line: Rich,
        /// Source miniature graph, never inferred by the UI.
        graph: MiniGraph,
    },
}

/// A labelled evidence-tab control.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceTab {
    /// The tab's label.
    pub label: &'static str,
    /// Its typed evidence identity.
    pub evidence: Evidence,
    /// Its pressed state.
    pub selected: bool,
    /// Reducer action selecting the tab.
    pub action: Action,
}

/// A numbered patch line; absent numbers are intentionally blank cells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    /// Old line number, absent for additions.
    pub old: Option<u32>,
    /// New line number, absent for deletions.
    pub new: Option<u32>,
    /// Addition, deletion or context mark.
    pub kind: LineKind,
    /// Prefix cell; context uses an empty prefix.
    pub prefix: &'static str,
    /// Source code text.
    pub text: &'static str,
}

/// The selected evidence contents, including file titles and real/illustrative labels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvidenceBody {
    /// Real patch hunk and option-1 explanation.
    Diff {
        /// Canonical board hunk, without duplicating its data.
        hunk: &'static Hunk,
        /// Display-ready line numbering.
        lines: Vec<DiffLine>,
        /// Explanation below the hunk.
        note: Rich,
    },
    /// Interface excerpt or log tail.
    Text {
        /// File title.
        file: &'static str,
        /// Illustrative attribution, if present.
        attribution: Option<&'static str>,
        /// Styled content lines; PASS lines are marked successful.
        lines: Vec<Rich>,
    },
    /// Rendered illustrative terminal screenshot.
    Shot {
        /// Real artifact title and rendered suffix.
        file: &'static str,
        /// Explicit illustrative label.
        attribution: &'static str,
        /// Styled terminal lines, preserving blank lines.
        lines: Vec<Rich>,
        /// Whether the terminal ends with a cursor after the final prompt.
        cursor: bool,
        /// Caption beneath the screenshot.
        note: &'static str,
    },
}

/// A: evidence is independently loading, failed, or ready with four tabs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Assessment {
    /// No evidence tabs are shown while the content streams in.
    Loading {
        /// Number of evidence skeleton blocks.
        skeleton_count: u8,
        /// Explanatory copy.
        note: &'static str,
    },
    /// The board-projected options remain current despite an artifact error.
    Error(Notice),
    /// Loaded evidence toolbar and selected body.
    Ready {
        /// Toolbar accessibility name.
        label: &'static str,
        /// Four ordered evidence tabs.
        tabs: Vec<EvidenceTab>,
        /// The selected body.
        body: EvidenceBody,
    },
}

/// One rendered answer radio, its consequences and its evidence link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnswerOption {
    /// One-based number printed on the option.
    pub number: usize,
    /// Complete option label.
    pub label: &'static str,
    /// Worker-recommendation chip, absent on the other option.
    pub recommendation_chip: Option<&'static str>,
    /// Whether this option is selected, including the elsewhere winning answer.
    pub selected: bool,
    /// Whether the radio is interactive.
    pub enabled: bool,
    /// Choose this option.
    pub action: Action,
    /// Then, Unblocks, Costs values in the label order on Recommendation.
    pub consequences: [&'static str; 3],
    /// “see” evidence link with the correct tab action.
    pub proof: Button,
}

/// R: options, free-answer field and recommendation explanation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recommendation {
    /// Radiogroup accessibility name.
    pub label: &'static str,
    /// Labels for each option's three consequence rows.
    pub consequence_labels: [&'static str; 3],
    /// Both numbered options.
    pub options: Vec<AnswerOption>,
    /// Whether this answer is read-only; queued answers remain editable in the source.
    pub locked: bool,
    /// Pencil glyph on the alternate-answer row.
    pub draft_mark: &'static str,
    /// Alternate-answer placeholder.
    pub draft_placeholder: &'static str,
    /// Current alternate answer, including whitespace as entered.
    pub draft: String,
    /// Bold recommendation lead-in.
    pub reason_label: &'static str,
    /// Complete recommendation reason.
    pub reason: &'static str,
}

/// Thread content shared by the inline band and phone sheet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadView {
    /// Phone sheet title.
    pub title: &'static str,
    /// The source's two illustrative messages.
    pub messages: &'static [Message],
    /// Reply textarea placeholder.
    pub reply_placeholder: &'static str,
    /// Textarea accessibility name.
    pub reply_label: &'static str,
    /// Ask button and source send toast.
    pub send: Button,
    /// Illustrative attribution beneath the reply.
    pub attribution: &'static str,
    /// Done control for dismissing the sheet.
    pub done: Button,
}

/// One history row with its owner-decision versus worker-attempt label resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryRow {
    /// Complete date and kind label.
    pub label: String,
    /// Whether this row uses the owner-decision mark.
    pub owner_decision: bool,
    /// Canonical history data.
    pub entry: HistoryEntry,
}

/// The source's SBAR, Thread and History band identities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BandKind {
    /// S.
    Situation,
    /// B.
    Background,
    /// A.
    Assessment,
    /// R.
    Recommendation,
    /// T.
    Thread,
    /// H.
    History,
}

/// Ordered band metadata, sufficient to render the correct headings in each form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Band {
    /// Which typed sheet payload to render.
    pub kind: BandKind,
    /// Single-letter left-margin label.
    pub letter: &'static str,
    /// Complete heading.
    pub title: &'static str,
    /// Right-side context or count, when the source has one.
    pub detail: Option<String>,
}

/// Sticky answer controls in all states; answered stamps replace the answer controls in place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecideBar {
    /// Ordered controls for the current form and answer state.
    pub buttons: Vec<Button>,
    /// Desktop's answered timestamp stamp.
    pub stamp: Option<&'static str>,
    /// Command attribution, offline queue rule, or one-answer-wins explanation.
    pub note: Rich,
}

/// A complete handover with payloads separate from source-defined band order.
#[derive(Debug, Clone, PartialEq)]
pub struct Sheet {
    /// Screen title used by the phone navigation bar.
    pub title: &'static str,
    /// Header metadata and question title.
    pub header: Header,
    /// Effective answer state, including the elsewhere override.
    pub state: QuestionState,
    /// Whether the sheet carries answered/read-only presentation.
    pub answered: bool,
    /// Main bands, in actual render order.
    pub bands: Vec<Band>,
    /// Focus-mode context bands, empty for inline desktop and phone.
    pub side_bands: Vec<Band>,
    /// S payload.
    pub situation: Situation,
    /// B payload.
    pub background: Background,
    /// A payload.
    pub assessment: Assessment,
    /// R payload.
    pub recommendation: Recommendation,
    /// T payload, full inline on desktop or in the phone sheet.
    pub thread: ThreadView,
    /// Phone's collapsed thread preview.
    pub thread_preview: Rich,
    /// Open the phone thread preview.
    pub thread_action: Action,
    /// Whether to show the phone thread sheet.
    pub thread_open: bool,
    /// H payload.
    pub history: Vec<HistoryRow>,
    /// Pinned decision controls.
    pub decide: DecideBar,
    /// Focus-page back control, absent inline.
    pub back: Option<Button>,
    /// Pager wording, absent inline.
    pub pager: Option<&'static str>,
    /// Next-plan control, absent inline.
    pub next: Option<Button>,
    /// Focus desktop shortcut help, absent inline and on phone.
    pub key_help: Option<Rich>,
    /// Ask about this question, absent inline.
    pub ask: Option<Button>,
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
        title: Q.short,
    }
}

/// Build S with the queued/answered/answered-elsewhere notice stamped beneath the question.
pub fn situation(state: &State, env: Env) -> Situation {
    let notice = match state.question_state(env) {
        QuestionState::Open => None,
        QuestionState::Elsewhere => Some(Notice {
            stamp: Some("ANSWERED ELSEWHERE"),
            hue: Hue::Ok,
            body: vec![
                span("Answered on the "),
                span(Q.answered_elsewhere.where_).bold(),
                span(format!(" at {} with option 1 · ", Q.answered_elsewhere.at)),
                span("via=pm-pane").mono(),
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
                span(format!("actor=owner {}", env.via())).mono(),
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
    };
    Situation {
        text: Q.text,
        waits_title: "Waiting on this answer",
        waits: Q.waits,
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
        graph: mini_graph(compact),
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
    Recommendation {
        label: "Options",
        consequence_labels: ["Then", "Unblocks", "Costs"],
        options: Q
            .options
            .iter()
            .enumerate()
            .map(|(index, option)| AnswerOption {
                number: index + 1,
                label: option.label,
                recommendation_chip: option.rec.then_some("worker recommends"),
                selected: chosen == index,
                enabled: !locked,
                action: Action::Choose(index),
                consequences: [option.next, option.unblocks, option.costs],
                proof: button(
                    format!("see {}", option.proof),
                    Action::Evidence(if index == 0 {
                        Evidence::Diff
                    } else {
                        Evidence::File
                    }),
                    ButtonKind::Quiet,
                ),
            })
            .collect(),
        locked,
        draft_mark: "✎",
        draft_placeholder: "Write a different answer…",
        draft: state.draft.clone(),
        reason_label: "Why the worker recommends 1:",
        reason: Q.recommendation,
    }
}

/// Build the complete thread band/sheet, including the illustrative send control.
pub fn thread_view() -> ThreadView {
    ThreadView {
        title: "Thread · worker T-868",
        messages: Q.thread,
        reply_placeholder: "Ask worker T-868 before answering…",
        reply_label: "Follow-up to the asker",
        send: button("Ask", Action::SendThread, ButtonKind::Ghost),
        attribution: "thread messages illustrative",
        done: button("Done", Action::CloseThread, ButtonKind::Quiet),
    }
}

/// Build the history labels without requiring renderer knowledge of “who”.
pub fn history() -> Vec<HistoryRow> {
    Q.history
        .iter()
        .map(|&entry| HistoryRow {
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
            entry,
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
            };
        }
        return DecideBar {
            buttons: vec![button(
                "Watch T-868 resume",
                Action::Open(Page::Session(Q.task)),
                ButtonKind::Ghost,
            )],
            stamp: Some(if status == QuestionState::Elsewhere {
                "ANSWERED ON PM PANE 22:14"
            } else {
                "ANSWERED 22:12"
            }),
            note: vec![span(
                "One answer wins: the PM pane, this app and every paired device read the same question record.",
            )],
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
    note.push(span("actor=owner · via=app:pawis-desktop").mono());
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
    }
}

fn band(kind: BandKind, letter: &'static str, title: &'static str, detail: Option<String>) -> Band {
    Band {
        kind,
        letter,
        title,
        detail,
    }
}

/// Desktop inline order is S, B, A, R, Thread, History. Focus mode moves B, Thread,
/// History to the side and leaves S, A, R in the centre so the choices stay visible.
pub fn sheet_desk(state: &State, env: Env, with_side: bool) -> Sheet {
    let mut sheet = build_sheet(state, env, with_side);
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
            band(
                BandKind::Background,
                "B",
                "Background · place",
                Some("T-870".into()),
            ),
            band(
                BandKind::Thread,
                "T",
                "Thread with the asker",
                Some(Q.thread.len().to_string()),
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
    sheet
}

/// Focus-page builder for every form. Phone order is S, R, A, B, Thread, History;
/// its thread is a collapsed preview opening the shared sheet, and its pager follows JS.
pub fn question(state: &State, env: Env) -> Sheet {
    if !phone(env) {
        return sheet_desk(state, env, true);
    }
    let mut sheet = build_sheet(state, env, true);
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
            Some(format!("{} · Ask ›", Q.thread.len())),
        ),
        band(BandKind::History, "H", "History", None),
    ];
    sheet
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
        decide: decide_bar(state, env),
        back: focus.then(|| button("Inbox", Action::Go(Destination::Inbox), ButtonKind::Quiet)),
        pager: focus.then_some("1 of 5 needing you"),
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
        key_help: (focus && !phone(env)).then(|| {
            vec![
                span("Keys "),
                span("1").mono(),
                span(" "),
                span("2").mono(),
                span(" choose · "),
                span("Ctrl ↵").mono(),
                span(" answer · "),
                span("Esc").mono(),
                span(" back"),
            ]
        }),
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
