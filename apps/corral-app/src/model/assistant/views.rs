//! Pure content builders for the shared composer, model list and Agents blocks.

use super::{
    Action, BOTH_Q, FILED_LIST, FiledRow, MODELS, Mode, Model, SESSIONS, Session, State, session,
};
use crate::model::board;
use crate::model::effect::{Conn, Env, Page};
use crate::model::hue::Hue;
use crate::model::nav::{AskTarget, Form};
use crate::model::text::{Rich, span};

/// A state chip, with its content and state colour already resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chip {
    /// Visible wording.
    pub label: String,
    /// Semantic state colour.
    pub hue: Hue,
}

/// A button descriptor shared by cards and surfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Button {
    /// Visible label.
    pub label: &'static str,
    /// Typed action on activation.
    pub action: Action,
    /// Whether activation is allowed.
    pub enabled: bool,
    /// Neutral primary rather than ghost treatment.
    pub primary: bool,
    /// Destructive action treatment.
    pub danger: bool,
}

pub(super) fn button(label: &'static str, action: Action) -> Button {
    Button {
        label,
        action,
        enabled: true,
        primary: false,
        danger: false,
    }
}

/// An entry in the Assistant | PM target switch, whose selected target the app owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetOption {
    /// Target to assign on activation.
    pub target: AskTarget,
    /// Visible target name.
    pub label: &'static str,
    /// Pressed state.
    pub selected: bool,
    /// PM state, absent for Assistant.
    pub state: Option<Chip>,
}

/// A removable bounded-context chip above the draft.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextChip {
    /// Kind, such as task or diff hunk.
    pub kind: &'static str,
    /// Item id and descriptive name.
    pub label: String,
    /// Removal action.
    pub remove: Button,
    /// Scope explanation beside the chip.
    pub note: &'static str,
}

/// A profile row in the accessible Model list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelOption {
    /// Profile, real model id, effort and note.
    pub profile: Model,
    /// Current profile checkmark.
    pub selected: bool,
    /// Picking this row closes the list and shows its toast.
    pub action: Action,
}

/// The model list, independent of popup or sheet placement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelList {
    /// Accessible listbox name.
    pub label: &'static str,
    /// Rows in configured cycle order.
    pub options: Vec<ModelOption>,
    /// Footer with mono configuration key.
    pub footer: Rich,
    /// Disclosure of illustrative profile names.
    pub illustrative: &'static str,
}

/// Builds every list row even when the popup is currently closed.
pub fn model_list(state: &State) -> ModelList {
    ModelList {
        label: "Model",
        options: MODELS
            .iter()
            .map(|model| ModelOption {
                profile: *model,
                selected: model.name == state.model_name(),
                action: Action::PickModel(model.name),
            })
            .collect(),
        footer: vec![
            span("Applies to your next message; the conversation is kept. List: "),
            span("[assistant.quick] models").mono(),
            span(" · Settings › Assistant"),
        ],
        illustrative: "profile names illustrative",
    }
}

/// A keyboard cap and its adjacent explanatory word or phrase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyHint {
    /// Text rendered inside the keyboard cap.
    pub keys: &'static str,
    /// Explanation following the cap.
    pub action: &'static str,
}

/// Composer keyboard hints in source order.
pub const COMPOSER_KEYS: &[KeyHint] = &[
    KeyHint {
        keys: "↵",
        action: "send",
    },
    KeyHint {
        keys: "⇧ ↵",
        action: "new line",
    },
    KeyHint {
        keys: "Ctrl ⇧ P",
        action: "PM",
    },
    KeyHint {
        keys: "Ctrl ⇧ M",
        action: "model",
    },
];

/// Small global-window keyboard hints in source order.
pub const HOTKEY_KEYS: &[KeyHint] = &[
    KeyHint {
        keys: "Ctrl Alt Space",
        action: "open from anywhere",
    },
    KeyHint {
        keys: "↵",
        action: "send",
    },
    KeyHint {
        keys: "Esc",
        action: "hide",
    },
];

/// One field and one Send, with every branch resolved for its target, host and connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Composer {
    /// Target options; empty in a named session's own composer.
    pub targets: Vec<TargetOption>,
    /// Model pill when this host uses the app picker.
    pub model_pill: Option<Button>,
    /// Text inside the model pill, without a disclosure glyph.
    pub model_name: Option<&'static str>,
    /// Open model list, absent when closed or not pickable.
    pub model_menu: Option<ModelList>,
    /// Whether the phone displays the model menu inline.
    pub menu_inline: bool,
    /// PM profile note or named daemon-session note.
    pub profile_note: Option<&'static str>,
    /// Changed-model line above the field, preserving the conversation.
    pub next_message: Option<Rich>,
    /// Removable bounded-context chip.
    pub chip: Option<ContextChip>,
    /// Draft owned by the app.
    pub text: String,
    /// Empty-field prompt.
    pub placeholder: String,
    /// Offline or waiting status above the composer.
    pub status: Option<Rich>,
    /// Attachment affordances and their illustrative outcome.
    pub attachments: Vec<Button>,
    /// Send remains available offline and while waiting, but does not submit blank input.
    pub send: Button,
    /// Desktop key hints, absent on phone and named-session composers.
    pub key_hints: Option<&'static [KeyHint]>,
}

fn pm_chip(conn: Conn) -> Chip {
    let (label, hue) = match conn {
        Conn::Healthy | Conn::Relay => ("live", Hue::Ok),
        Conn::Degraded => ("relaunching", Hue::Held),
        Conn::Down => ("absent", Hue::Fail),
        Conn::Offline => ("live · as of 22:08", Hue::Held),
    };
    Chip {
        label: label.into(),
        hue,
    }
}

fn context(id: &'static str) -> ContextChip {
    let (kind, label) = match id {
        "T-890" => ("task", "T-890 pm-bounded-read-contract".into()),
        "T-868/R1/Q1" => ("question", "T-868/R1/Q1 restore policy".into()),
        "hunk" => ("diff hunk", "85b45e1e · cross-lane.test.ts @@ -212".into()),
        "T-870" => ("plan", "plan T-870".into()),
        "T-854" => ("session", "A-15 · T-854".into()),
        _ => (
            "task",
            board::find(id).map_or_else(|| format!("{id} "), |task| format!("{id} {}", task.title)),
        ),
    };
    ContextChip {
        kind,
        label,
        remove: button("Remove context", Action::Unchip),
        note: "the assistant gets a bounded brief of it",
    }
}

/// Builds Ask or a named session's composer. Pass `None` for the quick thread and drawer.
/// `target` and `text` are app-owned; a named pane uses Pi's picker, never this model list.
pub fn composer(
    state: &State,
    target: AskTarget,
    text: &str,
    named_session: Option<&'static str>,
    env: Env,
) -> Composer {
    let named = named_session.is_some();
    let daemon = named_session
        .and_then(session)
        .is_some_and(|session| session.daemon);
    let pickable = if named {
        daemon
    } else {
        target != AskTarget::Pm
    };
    let status = if named {
        None
    } else if env.conn.refuses() {
        Some(vec![
            span("offline").hue(Hue::Held),
            span(" Messages queue on this device and send once when corrald is reachable."),
        ])
    } else if target != AskTarget::Pm {
        state.mode.wait_reason().map(|reason| {
            vec![
                span(format!("waiting for assistant · {}", reason.label())).hue(Hue::Held),
                span(" Send still stores your message; it waits in the thread."),
            ]
        })
    } else {
        None
    };
    let next_message = if pickable && state.model_name() != MODELS[0].name {
        let model = MODELS
            .iter()
            .find(|model| model.name == state.model_name())
            .unwrap_or(&MODELS[0]);
        Some(vec![
            span(state.model_name()),
            span(format!(
                " Your next message is answered by {}. The conversation is kept.",
                model.model
            )),
        ])
    } else {
        None
    };
    let placeholder = if let Some(id) = named_session {
        format!("Message {id}…")
    } else if let Some(chip) = state.chip {
        format!("Ask about {chip}…")
    } else if target == AskTarget::Pm {
        "Message the PM…".into()
    } else {
        "Ask a question, drop an idea, or give an instruction…".into()
    };
    let mut send = button(
        "Send",
        named_session.map_or(Action::Send, Action::SendToThread),
    );
    send.primary = true;
    send.enabled = !text.trim().is_empty();
    Composer {
        targets: if named {
            Vec::new()
        } else {
            vec![
                TargetOption {
                    target: AskTarget::Assistant,
                    label: "Assistant",
                    selected: target == AskTarget::Assistant,
                    state: None,
                },
                TargetOption {
                    target: AskTarget::Pm,
                    label: "PM",
                    selected: target == AskTarget::Pm,
                    state: Some(pm_chip(env.conn)),
                },
            ]
        },
        model_pill: pickable.then(|| button("Model for the next message", Action::ToggleModelMenu)),
        model_name: pickable.then(|| state.model_name()),
        model_menu: (pickable && state.model_menu).then(|| model_list(state)),
        menu_inline: pickable && env.form == Form::Phone,
        profile_note: if named {
            daemon.then_some("model for this session")
        } else {
            (target == AskTarget::Pm).then_some("the PM runs its own profile")
        },
        next_message,
        chip: state.chip.map(context),
        text: text.into(),
        placeholder,
        status,
        attachments: vec![
            button("Screenshot", Action::Toast("Attach a screenshot")),
            button("Link", Action::Toast("Attach a link")),
            button("File", Action::Toast("Attach a file")),
        ],
        send,
        key_hints: (!named && env.form != Form::Phone).then_some(COMPOSER_KEYS),
    }
}

/// Current quick-thread status chip, also used by the drawer and phone header.
pub fn state_chip(state: &State) -> Chip {
    if let Some(reason) = state.mode.wait_reason() {
        return Chip {
            label: format!("waiting for assistant · {}", reason.label()),
            hue: Hue::Held,
        };
    }
    let (label, hue) = match state.mode {
        Mode::Answering if state.stream < 1.0 => ("answering", Hue::Corral),
        Mode::Answering => ("waiting on you", Hue::You),
        Mode::Starting => ("starting", Hue::Corral),
        Mode::Stopped => ("stopped", Hue::Neutral),
        _ => ("", Hue::Neutral),
    };
    Chip {
        label: label.into(),
        hue,
    }
}

/// Pinned quick-thread tile; render the same content as a compact row on phone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickThread {
    /// Session id.
    pub id: &'static str,
    /// Full tile title.
    pub title: &'static str,
    /// Compact-row title.
    pub row_title: &'static str,
    /// Current answering/waiting chip.
    pub state: Chip,
    /// Pinning description.
    pub pinned: &'static str,
    /// Latest turn outcome.
    pub last_line: String,
    /// Last owner question, clipped at 96 characters.
    pub last_question: String,
    /// Attribution for the last owner message.
    pub owner_at: &'static str,
    /// Thread-wide model selection.
    pub model: &'static str,
    /// Host description.
    pub location: &'static str,
    /// Cross-device scope.
    pub scope: &'static str,
    /// Host's lifetime, unrelated to idle time.
    pub lifetime: &'static str,
    /// Opens the quick thread.
    pub action: Action,
}

/// A session row with both compact and desktop fact lines prebuilt.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionRow {
    /// Typed fixture facts.
    pub session: Session,
    /// State chip, including live-since time.
    pub state: Chip,
    /// Desktop facts with focus placeholder.
    pub desktop_facts: String,
    /// Phone facts omit focus when absent.
    pub phone_facts: String,
    /// Fixed two-decimal session cost.
    pub cost: String,
    /// Opens this session.
    pub action: Action,
}

/// Agents' three assistant blocks; no assistant destination is introduced.
#[derive(Debug, Clone, PartialEq)]
pub struct Agents {
    /// Pinned quick-thread tile or row.
    pub quick_thread: QuickThread,
    /// Assistant sessions section heading.
    pub sessions_title: &'static str,
    /// Named daemon and pane session rows, excluding A-7.
    pub sessions: Vec<SessionRow>,
    /// Desktop count descriptor.
    pub sessions_count: String,
    /// Starts an illustrative named daemon session.
    pub new_session: Button,
    /// Session fixture disclosure.
    pub sessions_illustrative: &'static str,
    /// Filed section heading.
    pub filed_title: &'static str,
    /// Filed rows, each naming its originating session.
    pub filed: &'static [FiledRow],
    /// Filed id disclosure.
    pub filed_illustrative: &'static str,
}

/// Builds quick-thread, named-session and Filed from chat blocks for Agents.
pub fn agents(state: &State, _env: Env) -> Agents {
    let last_line = if let Some(reason) = state.mode.wait_reason() {
        format!("waiting for assistant · {}", reason.label())
    } else {
        match state.mode {
            Mode::Starting => "starting",
            Mode::Stopped => "you stopped the last turn",
            _ if state.stream < 1.0 => "answering…",
            _ => "Your caching idea is filed as T-903. I checked duplicates first…",
        }
        .into()
    };
    let question = state.last_q.as_deref().unwrap_or(BOTH_Q);
    let rows: Vec<SessionRow> = SESSIONS
        .iter()
        .filter(|session| session.id != "A-7")
        .map(|session| {
            let cost = format!("${:.2}", session.cost);
            let focus = session
                .focus
                .map_or(String::new(), |id| format!(" · focus {id}"));
            SessionRow {
                session: *session,
                state: Chip {
                    label: session.state.into(),
                    hue: if session.state.starts_with("live") {
                        Hue::Worker
                    } else {
                        Hue::Neutral
                    },
                },
                desktop_facts: format!(
                    "{} · focus {} · {cost} · scratch {}",
                    session.location,
                    session.focus.unwrap_or("−"),
                    session.scratch
                ),
                phone_facts: format!(
                    "{} · {}{focus} · scratch {}",
                    session.state, session.location, session.scratch
                ),
                cost,
                action: Action::OpenThread(session.id),
            }
        })
        .collect();
    Agents {
        quick_thread: QuickThread {
            id: "A-7",
            title: "Quick thread · A-7",
            row_title: "Quick thread",
            state: state_chip(state),
            pinned: "assistant · pinned",
            last_line,
            last_question: format!("{}…", question.chars().take(96).collect::<String>()),
            owner_at: "you · 22:11",
            model: state.model_name(),
            location: "runs: daemon",
            scope: "shared by your devices",
            lifetime: "live · since 14:02",
            action: Action::OpenQuickThread,
        },
        sessions_title: "Assistant sessions",
        sessions_count: format!("{} · daemon and pane", rows.len()),
        sessions: rows,
        new_session: button(
            "+ New session",
            Action::Toast("New session A-19 started in corrald · separate from the quick thread"),
        ),
        sessions_illustrative: "sessions illustrative",
        filed_title: "Filed from chat",
        filed: FILED_LIST,
        filed_illustrative: "filed ids illustrative",
    }
}

/// One label/value fact in the session sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fact {
    /// Definition label.
    pub label: &'static str,
    /// Styled value, with any record link already attached.
    pub value: Rich,
}

/// Header and sidebar facts for a thread page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadHeader {
    /// Desktop page title.
    pub title: String,
    /// Compact phone title.
    pub phone_title: &'static str,
    /// Back destination label.
    pub back_label: &'static str,
    /// State chip shown only on the quick thread.
    pub state: Option<Chip>,
    /// Host/scope subtitle.
    pub subtitle: &'static str,
    /// Id, State, Where, Profile, Cost and Scratch.
    pub facts: Vec<Fact>,
    /// Session host lifetime explanation.
    pub lifetime_note: &'static str,
    /// Filed from this thread rows, only for A-7 in the mockup.
    pub filed: Vec<FiledRow>,
}

/// Builds known session headers; unknown ids have no fabricated session facts.
pub fn thread_header(state: &State, id: &'static str, _env: Env) -> Option<ThreadHeader> {
    let session = session(id)?;
    let quick = id == "A-7";
    let profile = if session.daemon {
        vec![
            span(state.model_name()),
            span(" · switch in the composer").dim(),
        ]
    } else {
        vec![span("pi-sol xhigh")]
    };
    Some(ThreadHeader {
        title: if quick {
            "Quick thread · A-7".into()
        } else {
            format!("{id} · {}", session.title)
        },
        phone_title: if quick { "Quick thread" } else { id },
        back_label: "Agents",
        state: quick.then(|| state_chip(state)),
        subtitle: if quick {
            "runs: daemon · one per workspace · every device"
        } else {
            "assistant session"
        },
        facts: vec![
            Fact {
                label: "Id",
                value: vec![span(id).mono()],
            },
            Fact {
                label: "State",
                value: vec![span(session.state)],
            },
            Fact {
                label: "Where",
                value: vec![span(session.location)],
            },
            Fact {
                label: "Profile",
                value: profile,
            },
            Fact {
                label: "Cost",
                value: vec![span(format!("${:.2} today", session.cost))],
            },
            Fact {
                label: "Scratch",
                value: vec![span(session.scratch.to_string())],
            },
        ],
        lifetime_note: "The host runs until its work is done or it stops on its own (it exits, crashes, you end the session, or corrald restarts). Rolling at caps starts the next A-<n> with a summary.",
        filed: if quick {
            FILED_LIST
                .iter()
                .filter(|row| row.source.starts_with("A-7"))
                .copied()
                .collect()
        } else {
            Vec::new()
        },
    })
}

/// Styled source command opening an app record, used by compact hotkey results.
pub(super) fn command_link(label: &'static str, page: Page) -> Rich {
    vec![span(label).mono().link(page)]
}
