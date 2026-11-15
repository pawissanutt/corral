//! Walk `ThreadItem` in order; an assistant reply can contain several different typed cards.
//! The records are owned so the fixture preview below and the live thread in `live_thread` fill
//! the same shapes.

use super::views::button;
use super::{
    ANSWER, Action, ActionCard, ActionClass, ActionOutcome, AnswerBlock, AnswerRun, AskUserCard,
    AttachmentView, BOTH_Q, Button, Chip, FileChip, FiledCard, MergedCard, MessageState, Mode,
    PermissionCard, SOURCES, SavedCard, State, TOOLS, WaitingBlock, duplicate_card, filed_card,
    merged_card, mode_pause_card, permission_card, saved_as_written, session, waiting_block,
};
use crate::model::effect::{Env, Page};
use crate::model::hue::Hue;
use crate::model::text::{Rich, span, task_link};

/// An owner message, with origin and delivery state independently available.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnerMessage {
    /// Name shown in the message header.
    pub author: &'static str,
    /// Header provenance including the `app:` prefix when appropriate.
    pub provenance: String,
    /// Delivery/display time.
    pub at: String,
    /// Owner's exact text.
    pub text: String,
    /// Delivery, waiting or terminal state, as its outlined tag.
    pub tag: Chip,
    /// Additional attribution such as the offline typing time.
    pub extra: Option<String>,
    /// Shared attachment or bounded context carried by the input.
    pub attachment: Option<String>,
    /// The images, files and links the message carries, as served (or as this device sent).
    pub files: Vec<FileChip>,
    /// The attachment whose canonical readback is open under the message.
    pub opened: Option<AttachmentView>,
}

/// An owner-authored PM inbox item and optional folded reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PmMessage {
    /// Owner input and its state.
    pub owner: OwnerMessage,
    /// `PMI-<n>`, or "PMI on arrival" while it has none yet.
    pub id: String,
    /// Folded PM outcome in this same thread.
    pub reply: Option<PmReply>,
    /// Non-interruption explanation for queued-for-PM items.
    pub note: Option<&'static str>,
    /// Withdraw while the PM has not replied.
    pub buttons: Vec<Button>,
    /// Status of a withdrawal this device sent.
    pub status: Option<Chip>,
}

/// The PM's outcome of one inbox item, not a separate conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PmReply {
    /// Reply time.
    pub at: String,
    /// Outcome text.
    pub text: String,
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
    /// Deferred permission prompt.
    Permission(PermissionCard),
    /// Assistant-filed Idea with editable generated fields.
    Filed(FiledCard),
    /// Host startup explanation.
    Starting { state: Chip, text: String },
    /// Illustrative-content disclosure.
    Illustrative(&'static str),
}

/// One expandable tool-activity row: tool, command and elapsed time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolRow {
    /// Tool name.
    pub kind: String,
    /// Command sent to the tool.
    pub command: String,
    /// Duration of this call, empty when unreported.
    pub elapsed: String,
    /// The call failed.
    pub failed: bool,
}

/// A record cited under the completed reply, and what pressing it does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceLink {
    /// Link text.
    pub label: String,
    /// Opens the record (or shows the fixture search outcome).
    pub action: Action,
}

/// An assistant message with the identity of the model that actually wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssistantMessage {
    /// Stable identity of the reply in its thread, for per-reply view state.
    pub key: String,
    /// One-shot reveal and highlight of the decision opened from Inbox.
    pub anchored: bool,
    /// Session id in the author header.
    pub id: String,
    /// Reply time.
    pub at: String,
    /// Historical model name, unaffected by selecting the next message's model.
    pub model: String,
    /// Answering, stopped, answered-and-filed or needs-you state.
    pub state: Option<Chip>,
    /// Ordered reply body, including any cards.
    pub body: Vec<Body>,
    /// Expandable tool summary, absent when the turn used no tools.
    pub tool_summary: Option<String>,
    /// Expandable tool rows.
    pub tools: Vec<ToolRow>,
    /// Whether to display the streaming caret after the visible paragraphs.
    pub streaming: bool,
    /// Optional Stop button.
    pub stop: Option<Button>,
    /// Cost and token count for this ask.
    pub cost: Option<String>,
    /// Completed-turn supporting records.
    pub sources: Vec<SourceLink>,
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
    pub text: String,
    /// Opens the earlier transcript, when it is known.
    pub transcript: Option<Button>,
}

/// A queue note and its non-expiring-message escape hatches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueNote {
    /// Delivery, late-authority and context-change explanation.
    pub text: String,
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
    /// A PM outcome whose inbox item is not in this page of the thread.
    PmReply { id: String, reply: PmReply },
    /// Host/model waiting block.
    Waiting(WaitingBlock),
    /// Locally queued delivery note.
    Queued(QueueNote),
    /// Owner-filed as-written result.
    Saved(SavedCard),
    /// Retained text's withdrawal explanation, or a thread notice.
    Note(String),
}

/// An owner message's outlined tag from the fixture's delivery state.
fn tag(state: MessageState, pm: bool) -> Chip {
    Chip::new(state.label(), if pm { state.pm_hue() } else { state.hue() })
}

fn owner(
    at: &str,
    device: &'static str,
    text: impl Into<String>,
    state: MessageState,
) -> OwnerMessage {
    OwnerMessage {
        author: "you",
        provenance: if device == "pane" {
            "pane".into()
        } else {
            format!("app:{device}")
        },
        at: at.into(),
        text: text.into(),
        tag: tag(state, false),
        extra: None,
        attachment: None,
        files: Vec::new(),
        opened: None,
    }
}

fn assistant(id: &str, at: &str, body: Vec<Body>) -> AssistantMessage {
    AssistantMessage {
        key: format!("{id}@{at}"),
        anchored: false,
        id: id.into(),
        at: at.into(),
        model: "pi-sol xhigh".into(),
        state: None,
        body,
        tool_summary: None,
        tools: Vec::new(),
        streaming: false,
        stop: None,
        cost: None,
        sources: Vec::new(),
        stopped_note: None,
        illustrative: None,
    }
}

fn pm(
    at: &str,
    device: &'static str,
    text: impl Into<String>,
    state: MessageState,
    id: &str,
    reply: Option<PmReply>,
) -> ThreadItem {
    let mut message = owner(at, device, text, state);
    message.tag = tag(state, true);
    ThreadItem::Pm(PmMessage {
        owner: message,
        id: id.into(),
        reply,
        note: (state == MessageState::QueuedForPm)
            .then_some("Delivered when the PM next takes input; supervision is not interrupted."),
        buttons: Vec::new(),
        status: None,
    })
}

fn action(command: &str, class: ActionClass, input: &str, outcome: ActionOutcome) -> Body {
    Body::Action(ActionCard {
        heading: "Owner action",
        command: command.into(),
        class,
        why: None,
        input: Some(input.into()),
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
                "withdrawn by you · nothing was filed or run · the text stays here for reference"
                    .into(),
            );
        }
        return if state.saved_now {
            ThreadItem::Saved(saved_as_written("T-903", words))
        } else {
            ThreadItem::Waiting(waiting_block(reason, true))
        };
    }
    if state.mode == Mode::Starting {
        let starting = Chip::new("starting", Hue::Corral);
        let mut message = assistant(
            "A-7",
            "22:11",
            vec![Body::Starting {
                state: starting.clone(),
                text: "A-7's host is starting (first use, or corrald restarted, or the host exited), usually 4–8 s. Your message is stored; keep typing.".into(),
            }],
        );
        message.state = Some(starting);
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
    if done && let Some(card) = filed_card(state, "T-903", false) {
        body.push(Body::Filed(card));
    }
    let mut message = assistant("A-7", "22:11", body);
    message.state = Some(Chip::new(
        if streaming {
            "answering"
        } else if state.mode == Mode::Stopped {
            "stopped"
        } else {
            "answered and filed"
        },
        if streaming { Hue::Corral } else { Hue::Neutral },
    ));
    message.tool_summary =
        Some("Read 4 records · searched duplicates · filed 1 Idea · 2.1 s · tool activity".into());
    message.tools = TOOLS
        .iter()
        .map(|tool| ToolRow {
            kind: tool.kind.into(),
            command: tool.command.into(),
            elapsed: tool.elapsed.into(),
            failed: false,
        })
        .collect();
    message.streaming = streaming;
    message.stop = streaming.then(|| button("Stop", Action::SetMode(Mode::Stopped)));
    message.cost = Some("this ask $0.04 · 7.3k tokens".into());
    if done {
        message.sources = SOURCES
            .iter()
            .map(|source| SourceLink {
                label: source.label.into(),
                action: match (source.page, source.toast) {
                    (Some(page), _) => Action::Open(page),
                    (None, Some(toast)) => Action::Toast(toast),
                    (None, None) => Action::Toast(source.label),
                },
            })
            .collect();
    }
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
                    text: "Posted to the PM on arrival, with the time you typed it (22:13).".into(),
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
                MessageState::Sent => {
                    let mut reply = assistant(message.thread, "now", vec![Body::Illustrative("mock: the assistant's reply to new messages is not simulated")]);
                    reply.model = message.model.into();
                    items.push(ThreadItem::Assistant(reply));
                }
                MessageState::Waiting(reason) => items.push(ThreadItem::Waiting(waiting_block(reason, false))),
                MessageState::Saved(id) => items.push(ThreadItem::Saved(saved_as_written(id, &message.text))),
                MessageState::Withdrawn => {},
                _ => items.push(ThreadItem::Queued(QueueNote { text: "Delivered once when corrald is reachable; it never expires. If that is more than 15 min from now it can be answered and filed, not instruct. A context chip that changes meanwhile arrives marked \"context changed since you typed\".".into(), buttons: vec![button("File as idea now", Action::FileNow), button("Withdraw", Action::Withdraw)] })),
            }
        }
    }
}

/// Builds any known fixture thread. A-17 is pane-authored, A-18 app-authored; named transcripts
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
        text: "earlier conversation summarized · A-6 → A-7 · 14 messages, 3 filed, 2 owner actions"
            .into(),
        transcript: Some(button(
            "A-6 transcript",
            Action::Toast("A-6 transcript opened (read-only)"),
        )),
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
    items.push(pm("21:44", "pawis-iphone", "Hold T-852's re-gate until the external op finishes.", MessageState::PmReplied, "PMI-118", Some(PmReply { at: "21:46".into(), text: "Already held (held · external op). I re-gate when the op ends and tell you if it fails again.".into() })));
    let mut late = owner(
        "21:47",
        "pawis-iphone",
        "why is T-852 held? park it till the op is done",
        MessageState::Late,
    );
    late.extra = Some("typed 21:20 offline".into());
    items.push(ThreadItem::Owner(late));
    items.push(ThreadItem::Assistant(assistant(id, "21:47", vec![Body::Paragraph(vec![task_link("T-852"), span(" is "), span("held · external op").bold(), span(": the op started 19:28 and Corral re-gates when it ends. It has spent $43 over 2 rounds.")]), action("corral attention park T-852", ActionClass::Late, "A-7/g2/i15", ActionOutcome::Late { typed: "21:20".into(), delivered: "21:47".into(), explanation: "Nothing ran. You typed this at 21:20; it reached corrald at 21:47, more than 15 min later, so it cannot instruct. Say it again to act.".into() })])));
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
    items.push(ThreadItem::Assistant(assistant(id, "21:58", vec![Body::Action(ActionCard { heading: "Owner action", command: "corral approve T-890".into(), class: ActionClass::Confirm, why: Some("approve (land) waits for a confirmation".into()), input: Some("A-7/g2/i17".into()), confirmed: Some("pawis-desktop 21:58".into()), outcome: ActionOutcome::Refused("approve: T-890 has no passing gate on candidate 1751bd2 (quick gate running)".into()) }), Body::Paragraph(vec![span("The quick gate on "), span("1751bd2").mono().link(Page::Task("T-890")), span(" is still running (412/980). I will not retry; say \"land T-890\" again once it passes.")]), Body::Illustrative("refusal text illustrative")])));
    items.push(ThreadItem::Owner(owner(
        "22:02",
        "pawis-iphone",
        "Pause autopilot for tonight once the running gates finish.",
        MessageState::Sent,
    )));
    let mut pause = assistant(id, "22:02", vec![Body::Action(mode_pause_card())]);
    pause.state = Some(Chip::new("waiting on you", Hue::You));
    pause.anchored = state.anchor.as_ref().is_some_and(|(thread, key)| {
        *thread == super::ThreadRef::Fixture(id) && key == "confirmation"
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
    permission.anchored = state.anchor.as_ref().is_some_and(|(thread, key)| {
        *thread == super::ThreadRef::Fixture(id) && key == "permission"
    });
    permission.state = Some(Chip::new("waiting on you", Hue::You));
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
    shared.attachment = Some("link nsh.dev/elements/diff-view".into());
    items.push(ThreadItem::Owner(shared));
    items.push(ThreadItem::Assistant(assistant(
        id,
        "22:08",
        vec![Body::Action(ActionCard {
            heading: "Owner action",
            command: "corral dispatch T-891".into(),
            class: ActionClass::Confirm,
            why: Some("this turn read a shared link".into()),
            input: Some("A-7/g2/i21".into()),
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
