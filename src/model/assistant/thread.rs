//! Walk `ThreadItem` in order; an assistant reply can contain several different typed cards.

use super::views::button;
use super::{
    ANSWER, Action, ActionCard, ActionClass, ActionOutcome, AnswerBlock, AnswerRun, AskUserCard,
    BOTH_Q, Button, Chip, FiledCard, MergedCard, MessageState, Mode, PermissionCard, SOURCES,
    SavedCard, Source, State, TOOLS, Tool, WaitingBlock, duplicate_card, filed_card, merged_card,
    mode_pause_card, permission_card, saved_as_written, session, waiting_block,
};
use crate::model::effect::{Env, Page};
use crate::model::hue::Hue;
use crate::model::text::{Rich, span, task_link};

/// An owner message, with origin and delivery state independently available.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnerMessage {
    /// Name shown in the message header.
    pub author: &'static str,
    /// Device name, or `pane` for a Pi-authored message.
    pub device: &'static str,
    /// Header provenance including the `app:` prefix when appropriate.
    pub provenance: String,
    /// Delivery/display time.
    pub at: &'static str,
    /// Owner's exact text.
    pub text: String,
    /// Delivery, waiting or terminal state.
    pub state: MessageState,
    /// Additional attribution such as the offline typing time.
    pub extra: Option<&'static str>,
    /// Shared attachment carried by the input.
    pub attachment: Option<&'static str>,
}

/// An owner-authored PM inbox item and optional folded reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PmMessage {
    /// Owner input and its state.
    pub owner: OwnerMessage,
    /// `PMI-<n>`, or PMI on arrival for an on-device queue.
    pub id: &'static str,
    /// Folded PM outcome in this same thread.
    pub reply: Option<PmReply>,
    /// Non-interruption explanation for queued-for-PM items.
    pub note: Option<&'static str>,
}

/// The PM's outcome of one inbox item, not a separate conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PmReply {
    /// Reply time.
    pub at: &'static str,
    /// Outcome text.
    pub text: &'static str,
}

/// Content inside an assistant reply, in its original order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    /// Prose with every inline reference already linked.
    Paragraph(Rich),
    /// Numbered paragraphs.
    Ordered(Vec<Rich>),
    /// Owner-command class and receipt.
    Action(ActionCard),
    /// Answered or answerable ask_user card.
    AskUser(AskUserCard),
    /// Comment appended instead of a new filing.
    Merged(MergedCard),
    /// Deferred file-write permission prompt.
    Permission(PermissionCard),
    /// Assistant-filed Idea with editable generated fields.
    Filed(FiledCard),
    /// Host startup explanation.
    Starting { state: Chip, text: &'static str },
    /// Illustrative-content disclosure.
    Illustrative(&'static str),
}

/// An assistant message with the identity of the model that actually wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssistantMessage {
    /// Session id in the author header.
    pub id: &'static str,
    /// Reply time.
    pub at: &'static str,
    /// Historical model name, unaffected by selecting the next message's model.
    pub model: &'static str,
    /// Answering, stopped, answered-and-filed or needs-you state.
    pub state: Option<Chip>,
    /// Ordered reply body, including any cards.
    pub body: Vec<Body>,
    /// Expandable tool summary, absent outside the streamed fixture turn.
    pub tool_summary: Option<&'static str>,
    /// Expandable tool rows.
    pub tools: &'static [Tool],
    /// Whether to display the streaming caret after the visible paragraphs.
    pub streaming: bool,
    /// Optional Stop button.
    pub stop: Option<Button>,
    /// Cost and token count for this ask.
    pub cost: Option<&'static str>,
    /// Completed-turn supporting records.
    pub sources: &'static [Source],
    /// Partial-answer retention explanation after Stop.
    pub stopped_note: Option<&'static str>,
    /// Snapshot/wording disclosure.
    pub illustrative: Option<&'static str>,
}

impl AssistantMessage {
    /// Heading for supporting record links, shown only after a completed turn.
    pub fn sources_label(&self) -> Option<&'static str> {
        (!self.sources.is_empty()).then_some("Rests on")
    }
}

/// A roll divider that preserves access to the previous read-only transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RollDivider {
    /// Summary of the previous generation and counts.
    pub text: &'static str,
    /// Opens the illustrative earlier transcript.
    pub transcript: Button,
}

/// A queue note and its non-expiring-message escape hatches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueNote {
    /// Delivery, late-authority and context-change explanation.
    pub text: &'static str,
    /// File and/or withdraw actions.
    pub buttons: Vec<Button>,
}

/// Top-level transcript item; render in sequence, without deriving state from prose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThreadItem {
    /// Generation boundary and summary.
    Roll(RollDivider),
    /// Owner input.
    Owner(OwnerMessage),
    /// Assistant output and cards.
    Assistant(AssistantMessage),
    /// PM input plus its optional outcome.
    Pm(PmMessage),
    /// Host/model waiting block.
    Waiting(WaitingBlock),
    /// Locally queued delivery note.
    Queued(QueueNote),
    /// Owner-filed as-written result.
    Saved(SavedCard),
    /// Retained text's withdrawal explanation.
    Note(&'static str),
}

fn owner(
    at: &'static str,
    device: &'static str,
    text: impl Into<String>,
    state: MessageState,
) -> OwnerMessage {
    OwnerMessage {
        author: "you",
        device,
        provenance: if device == "pane" {
            "pane".into()
        } else {
            format!("app:{device}")
        },
        at,
        text: text.into(),
        state,
        extra: None,
        attachment: None,
    }
}

fn assistant(id: &'static str, at: &'static str, body: Vec<Body>) -> AssistantMessage {
    AssistantMessage {
        id,
        at,
        model: "pi-sol xhigh",
        state: None,
        body,
        tool_summary: None,
        tools: &[],
        streaming: false,
        stop: None,
        cost: None,
        sources: &[],
        stopped_note: None,
        illustrative: None,
    }
}

fn pm(
    at: &'static str,
    device: &'static str,
    text: impl Into<String>,
    state: MessageState,
    id: &'static str,
    reply: Option<PmReply>,
) -> ThreadItem {
    ThreadItem::Pm(PmMessage {
        owner: owner(at, device, text, state),
        id,
        reply,
        note: (state == MessageState::QueuedForPm)
            .then_some("Delivered when the PM next takes input; supervision is not interrupted."),
    })
}

fn action(
    command: &'static str,
    class: ActionClass,
    input: &'static str,
    outcome: ActionOutcome,
) -> Body {
    Body::Action(ActionCard {
        heading: "Owner action",
        command,
        class,
        why: None,
        input,
        confirmed: None,
        outcome,
    })
}

fn rich(runs: &[AnswerRun]) -> Rich {
    runs.iter()
        .map(|run| match run {
            AnswerRun::Text(text) => span(*text),
            AnswerRun::Link(label, page) => span(*label).mono().link(*page),
        })
        .collect()
}

fn answer(state: &State, words: &str, env: Env) -> ThreadItem {
    if let Some(reason) = state.mode.wait_reason() {
        if state.withdrawn {
            return ThreadItem::Note(
                "withdrawn by you · nothing was filed or run · the text stays here for reference",
            );
        }
        return if state.saved_now {
            ThreadItem::Saved(saved_as_written("T-903", words))
        } else {
            ThreadItem::Waiting(waiting_block(reason, true))
        };
    }
    if state.mode == Mode::Starting {
        let mut message = assistant(
            "A-7",
            "22:11",
            vec![Body::Starting {
                state: Chip {
                    label: "starting".into(),
                    hue: Hue::Corral,
                },
                text: "A-7's host is starting (first use, or corrald restarted, or the host exited), usually 4–8 s. Your message is stored; keep typing.",
            }],
        );
        message.state = Some(Chip {
            label: "starting".into(),
            hue: Hue::Corral,
        });
        return ThreadItem::Assistant(message);
    }
    let position = if state.stream.is_finite() {
        state.stream.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let shown = if state.mode == Mode::Stopped {
        2
    } else {
        ((position * (ANSWER.len() + 1) as f32).ceil() as usize).max(1)
    };
    let streaming = state.mode == Mode::Answering && position < 1.0;
    let done = !streaming && state.mode != Mode::Stopped;
    let mut body: Vec<Body> = ANSWER
        .iter()
        .take(shown)
        .map(|block| match block {
            AnswerBlock::Paragraph(runs) => Body::Paragraph(rich(runs)),
            AnswerBlock::Ordered(items) => {
                Body::Ordered(items.iter().map(|runs| rich(runs)).collect())
            }
        })
        .collect();
    if done {
        if let Some(card) = filed_card(state, "T-903", false) {
            body.push(Body::Filed(card));
        }
    }
    let mut message = assistant("A-7", "22:11", body);
    message.state = Some(Chip {
        label: if streaming {
            "answering"
        } else if state.mode == Mode::Stopped {
            "stopped"
        } else {
            "answered and filed"
        }
        .into(),
        hue: if streaming { Hue::Corral } else { Hue::Neutral },
    });
    message.tool_summary =
        Some("Read 4 records · searched duplicates · filed 1 Idea · 2.1 s · tool activity");
    message.tools = TOOLS;
    message.streaming = streaming;
    message.stop = streaming.then(|| button("Stop", Action::SetMode(Mode::Stopped)));
    message.cost = Some("this ask $0.04 · 7.3k tokens");
    message.sources = if done { SOURCES } else { &[] };
    message.stopped_note = (state.mode == Mode::Stopped)
        .then_some("you stopped this turn · partial answer kept · host still live");
    message.illustrative = Some("wording illustrative; facts from the 22:12 snapshot");
    let _ = env;
    ThreadItem::Assistant(message)
}

fn append_sent(items: &mut Vec<ThreadItem>, state: &State, id: &str) {
    for message in state.sent.iter().filter(|message| message.thread == id) {
        if message.pm {
            items.push(pm(
                "now",
                message.device,
                message.text.clone(),
                message.state,
                if message.state == MessageState::QueuedOnDevice {
                    "PMI on arrival"
                } else {
                    "PMI-121"
                },
                None,
            ));
            if message.state == MessageState::QueuedOnDevice {
                items.push(ThreadItem::Queued(QueueNote {
                    text: "Posted to the PM on arrival, with the time you typed it (22:13).",
                    buttons: vec![button("Withdraw", Action::Withdraw)],
                }));
            }
        } else {
            items.push(ThreadItem::Owner(owner(
                "now",
                message.device,
                message.text.clone(),
                message.state,
            )));
            match message.state {
                MessageState::Sent => items.push(ThreadItem::Assistant(assistant(message.thread, "now", vec![Body::Illustrative("mock: the assistant's reply to new messages is not simulated")]))),
                MessageState::Waiting(reason) => items.push(ThreadItem::Waiting(waiting_block(reason, false))),
                MessageState::Saved(id) => items.push(ThreadItem::Saved(saved_as_written(id, &message.text))),
                MessageState::Withdrawn => {},
                _ => items.push(ThreadItem::Queued(QueueNote { text: "Delivered once when corrald is reachable; it never expires. If that is more than 15 min from now it can be answered and filed, not instruct. A context chip that changes meanwhile arrives marked \"context changed since you typed\".", buttons: vec![button("File as idea now", Action::FileNow), button("Withdraw", Action::Withdraw)] })),
            }
        }
    }
}

/// Builds any known thread. A-17 is pane-authored, A-18 app-authored; named transcripts
/// intentionally have only the two illustrative messages present in the source, not invented history.
pub fn thread(state: &State, id: &'static str, env: Env) -> Vec<ThreadItem> {
    if id != "A-7" {
        let Some(session) = session(id) else {
            return Vec::new();
        };
        let device = if session.daemon {
            "pawis-desktop"
        } else {
            "pane"
        };
        let mut items = vec![
            ThreadItem::Owner(owner(
                "20:58",
                device,
                format!("{}: summarise where we are.", session.title),
                MessageState::Sent,
            )),
            ThreadItem::Assistant(assistant(
                id,
                "20:59",
                vec![
                    Body::Paragraph(vec![span(if session.state == "ended" {
                        "Session ended; the transcript stays readable."
                    } else {
                        "Working through it; I'll post what I find here."
                    })]),
                    Body::Illustrative("transcript illustrative"),
                ],
            )),
        ];
        append_sent(&mut items, state, id);
        return items;
    }
    let mut items = vec![ThreadItem::Roll(RollDivider {
        text: "earlier conversation summarized · A-6 → A-7 · 14 messages, 3 filed, 2 owner actions",
        transcript: button(
            "A-6 transcript",
            Action::Toast("A-6 transcript opened (read-only)"),
        ),
    })];
    items.push(ThreadItem::Owner(owner(
        "21:31",
        "pawis-desktop",
        "queue T-873",
        MessageState::Sent,
    )));
    items.push(ThreadItem::Assistant(assistant(
        id,
        "21:31",
        vec![action(
            "corral queue add T-873",
            ActionClass::Run,
            "A-7/g2/i14",
            ActionOutcome::Executed("T-873 annotation-pi-host-api queued · after T-871".into()),
        )],
    )));
    items.push(pm("21:44", "pawis-iphone", "Hold T-852's re-gate until the external op finishes.", MessageState::PmReplied, "PMI-118", Some(PmReply { at: "21:46", text: "Already held (held · external op). I re-gate when the op ends and tell you if it fails again." })));
    let mut late = owner(
        "21:47",
        "pawis-iphone",
        "why is T-852 held? park it till the op is done",
        MessageState::Late,
    );
    late.extra = Some("typed 21:20 offline");
    items.push(ThreadItem::Owner(late));
    items.push(ThreadItem::Assistant(assistant(id, "21:47", vec![Body::Paragraph(vec![task_link("T-852"), span(" is "), span("held · external op").bold(), span(": the op started 19:28 and Corral re-gates when it ends. It has spent $43 over 2 rounds.")]), action("corral attention park T-852", ActionClass::Late, "A-7/g2/i15", ActionOutcome::Late { typed: "21:20", delivered: "21:47", explanation: "Nothing ran. You typed this at 21:20; it reached corrald at 21:47, more than 15 min later, so it cannot instruct. Say it again to act.".into() })])));
    items.push(pm(
        "21:50",
        "pawis-desktop",
        "After T-890 lands, admit T-891 before T-895.",
        MessageState::Delivered,
        "PMI-119",
        None,
    ));
    items.push(ThreadItem::Owner(owner(
        "21:52",
        "pawis-laptop",
        "queue T-872",
        MessageState::Sent,
    )));
    items.push(ThreadItem::Assistant(assistant(id, "21:52", vec![action("corral queue add T-872", ActionClass::AskAndFileOnly, "A-7/g2/i16", ActionOutcome::AskAndFileOnly(vec![span("Nothing ran. "), span("pawis-laptop").mono(), span(" is a web client: it may ask and file, not instruct. Say it from the desktop, phone or terminal, or turn instructing on for it in Settings › Devices.")]))])));
    items.push(ThreadItem::Owner(owner(
        "21:58",
        "pawis-desktop",
        "land T-890",
        MessageState::Sent,
    )));
    items.push(ThreadItem::Assistant(assistant(id, "21:58", vec![Body::Action(ActionCard { heading: "Owner action", command: "corral approve T-890", class: ActionClass::Confirm, why: Some("approve (land) waits for a confirmation"), input: "A-7/g2/i17", confirmed: Some("pawis-desktop 21:58"), outcome: ActionOutcome::Refused("approve: T-890 has no passing gate on candidate 1751bd2 (quick gate running)".into()) }), Body::Paragraph(vec![span("The quick gate on "), span("1751bd2").mono().link(Page::Task("T-890")), span(" is still running (412/980). I will not retry; say \"land T-890\" again once it passes.")]), Body::Illustrative("refusal text illustrative")])));
    items.push(ThreadItem::Owner(owner(
        "22:02",
        "pawis-iphone",
        "Pause autopilot for tonight once the running gates finish.",
        MessageState::Sent,
    )));
    let mut pause = assistant(id, "22:02", vec![Body::Action(mode_pause_card())]);
    pause.state = Some(Chip {
        label: "waiting on you".into(),
        hue: Hue::You,
    });
    items.push(ThreadItem::Assistant(pause));
    items.push(ThreadItem::Owner(owner(
        "22:04",
        "pawis-desktop",
        "review diffs need an inline comment box tied to the candidate SHA",
        MessageState::Sent,
    )));
    items.push(ThreadItem::Assistant(assistant(
        id,
        "22:04",
        vec![Body::AskUser(duplicate_card()), Body::Merged(merged_card())],
    )));
    items.push(ThreadItem::Owner(owner(
        "22:05",
        "pawis-desktop",
        "keep tonight's notes in ~/miti-workspace/notes/corral-log.md",
        MessageState::Sent,
    )));
    let mut permission = assistant(id, "22:05", vec![Body::Permission(permission_card(env))]);
    permission.state = Some(Chip {
        label: "waiting on you".into(),
        hue: Hue::You,
    });
    items.push(ThreadItem::Assistant(permission));
    items.push(pm(
        "22:06",
        "pawis-desktop",
        "Summarise tonight's stalls for me at 07:00.",
        MessageState::QueuedForPm,
        "PMI-120",
        None,
    ));
    let mut shared = owner(
        "22:08",
        "pawis-iphone",
        "read this and dispatch T-891 if it fits",
        MessageState::Sent,
    );
    shared.attachment = Some("link nsh.dev/elements/diff-view");
    items.push(ThreadItem::Owner(shared));
    items.push(ThreadItem::Assistant(assistant(
        id,
        "22:08",
        vec![Body::Action(ActionCard {
            heading: "Owner action",
            command: "corral dispatch T-891",
            class: ActionClass::Confirm,
            why: Some("this turn read a shared link"),
            input: "A-7/g2/i21",
            confirmed: None,
            outcome: ActionOutcome::Declined(
                "declined on pawis-iphone 22:09 · nothing changed".into(),
            ),
        })],
    )));
    items.push(ThreadItem::Owner(owner(
        "22:10",
        "pawis-iphone",
        "board: show the PM inbox count in the title bar",
        MessageState::Sent,
    )));
    if let Some(card) = filed_card(state, "T-902", false) {
        items.push(ThreadItem::Assistant(assistant(
            id,
            "22:10",
            vec![Body::Filed(card)],
        )));
    }
    let words = state.last_q.as_deref().unwrap_or(BOTH_Q);
    let message_state = if let Some(reason) = state.mode.wait_reason() {
        if state.withdrawn {
            MessageState::Withdrawn
        } else if state.saved_now {
            MessageState::Saved("T-903")
        } else {
            MessageState::Waiting(reason)
        }
    } else {
        MessageState::Sent
    };
    items.push(ThreadItem::Owner(owner(
        "22:11",
        env.device(),
        words,
        message_state,
    )));
    items.push(answer(state, words, env));
    append_sent(&mut items, state, id);
    items
}
