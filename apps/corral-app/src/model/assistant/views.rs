//! Content shared by the composer, model list and Agents blocks, and the fixture preview's
//! builders for them. Every field the daemon can supply is owned, so the live builders in
//! `live` fill the same records the renderers already draw.

use super::{
    Action, BOTH_Q, FILED_LIST, FiledRow, MODELS, Mode, SESSIONS, Session, StagedChip, State,
    session,
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

impl Chip {
    pub fn new(label: impl Into<String>, hue: Hue) -> Chip {
        Chip {
            label: label.into(),
            hue,
        }
    }
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
    pub kind: String,
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
    /// Profile name.
    pub name: String,
    /// Model id and effort under the name.
    pub model: String,
    /// Short selection advice, empty when the profile has none.
    pub note: String,
    /// Current profile checkmark.
    pub selected: bool,
    /// Picking this row closes the list and applies the profile to the next message.
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
    /// Disclosure of illustrative profile names (fixture only).
    pub illustrative: Option<&'static str>,
}

/// The list's footer: where the profiles are configured.
pub(super) fn model_footer() -> Rich {
    vec![
        span("Applies to your next message; the conversation is kept. List: "),
        span("[assistant.quick] models").mono(),
        span(" · Settings › Assistant"),
    ]
}

/// Builds every list row even when the popup is currently closed.
pub fn model_list(state: &State) -> ModelList {
    ModelList {
        label: "Model",
        options: MODELS
            .iter()
            .map(|model| ModelOption {
                name: model.name.into(),
                model: model.model.into(),
                note: model.note.into(),
                selected: model.name == state.model_name(),
                action: Action::PickModel(model.name),
            })
            .collect(),
        footer: model_footer(),
        illustrative: Some("profile names illustrative"),
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
    pub model_name: Option<String>,
    /// Open model list, absent when closed or not pickable.
    pub model_menu: Option<ModelList>,
    /// Whether the phone displays the model menu inline.
    pub menu_inline: bool,
    /// PM profile note, named-session note or the host's own profile.
    pub profile_note: Option<String>,
    /// Changed-model line above the field, preserving the conversation.
    pub next_message: Option<Rich>,
    /// Removable bounded-context chip.
    pub chip: Option<ContextChip>,
    /// Draft owned by the app.
    pub text: String,
    /// Empty-field prompt.
    pub placeholder: String,
    /// Offline, waiting, refusal or failure status above the composer.
    pub status: Option<Rich>,
    /// Screenshot, Link and File. With the daemon they attach for real and are dimmed only for
    /// a missing capability, the link (uploads), authority, the PM target or a full message.
    pub attachments: Vec<Button>,
    /// What is staged for the next message, in the context chip's anatomy.
    pub staged: Vec<StagedChip>,
    /// Why an attachment button is dimmed.
    pub attach_note: Option<String>,
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
    Chip::new(label, hue)
}

fn context(id: &'static str) -> ContextChip {
    let (kind, label) = match id {
        "T-890" => ("task", "T-890 pm-bounded-read-contract".into()),
        "T-868/R1/Q1" => ("question", "T-868/R1/Q1 restore policy".into()),
        "hunk" => ("diff hunk", "85b45e1e · cross-lane.test.ts @@ -212".into()),
        "T-870" => ("plan", "T-870".into()),
        "T-854" => ("session", "A-15 · T-854".into()),
        _ => (
            "task",
            board::find(id).map_or_else(|| format!("{id} "), |task| format!("{id} {}", task.title)),
        ),
    };
    ContextChip {
        kind: kind.into(),
        label,
        remove: button("Remove context", Action::Unchip),
        note: "the assistant gets a bounded brief of it",
    }
}

/// The removable chip's note and remove control, shared with the live composer.
pub(super) fn context_chip(kind: String, label: String) -> ContextChip {
    ContextChip {
        kind,
        label,
        remove: button("Remove context", Action::Unchip),
        note: "the assistant gets a bounded brief of it",
    }
}

/// The composer's target switch; the PM entry carries its state.
pub(super) fn targets(target: AskTarget, pm: Chip) -> Vec<TargetOption> {
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
            state: Some(pm),
        },
    ]
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
    let next_message = if pickable && state.model_note {
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
            targets(target, pm_chip(env.conn))
        },
        model_pill: pickable.then(|| button("Model for the next message", Action::ToggleModelMenu)),
        model_name: pickable.then(|| state.model_name().to_owned()),
        model_menu: (pickable && state.model_menu).then(|| model_list(state)),
        menu_inline: pickable && env.form == Form::Phone,
        profile_note: if named {
            daemon.then(|| "model for this session".to_owned())
        } else {
            (target == AskTarget::Pm).then(|| "the PM runs its own profile".to_owned())
        },
        next_message,
        chip: state.chip.map(context),
        text: text.into(),
        placeholder,
        status,
        attachments: vec![
            button("Screenshot", Action::Toast(FIXTURE_ATTACH)),
            button("Link", Action::Toast(FIXTURE_ATTACH)),
            button("File", Action::Toast(FIXTURE_ATTACH)),
        ],
        staged: Vec::new(),
        attach_note: None,
        send,
        key_hints: (!named && env.form != Form::Phone).then_some(COMPOSER_KEYS),
    }
}

/// The fixture preview's attachment buttons say what they would need.
const FIXTURE_ATTACH: &str = "The fixture preview attaches nothing; connect to corrald to attach screenshots, links and files.";

/// Current quick-thread status chip, also used by the drawer and phone header.
pub fn state_chip(state: &State) -> Chip {
    if let Some(reason) = state.mode.wait_reason() {
        return Chip::new(
            format!("waiting for assistant · {}", reason.label()),
            Hue::Held,
        );
    }
    let (label, hue) = match state.mode {
        Mode::Answering if state.stream < 1.0 => ("answering", Hue::Corral),
        Mode::Answering => ("waiting on you", Hue::You),
        Mode::Starting => ("starting", Hue::Corral),
        Mode::Stopped => ("stopped", Hue::Neutral),
        _ => ("", Hue::Neutral),
    };
    Chip::new(label, hue)
}

/// Pinned quick-thread tile; render the same content as a compact row on phone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickThread {
    /// Session id.
    pub id: String,
    /// Full tile title.
    pub title: String,
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
    pub owner_at: String,
    /// Thread-wide model selection.
    pub model: String,
    /// Host description.
    pub location: String,
    /// Cross-device scope.
    pub scope: &'static str,
    /// Host's lifetime, unrelated to idle time.
    pub lifetime: String,
    /// Opens the quick thread.
    pub action: Action,
}

/// A session row with both compact and desktop fact lines prebuilt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRow {
    /// Session identity.
    pub id: String,
    /// Session title.
    pub title: String,
    /// State chip, including live-since time.
    pub state: Chip,
    /// Desktop facts with focus placeholder.
    pub desktop_facts: String,
    /// Phone facts omit focus when absent.
    pub phone_facts: String,
    /// Fixed two-decimal session cost, or "cost unknown".
    pub cost: String,
    /// Opens this session.
    pub action: Action,
}

/// One Filed from chat row, always naming its source session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FiledLine {
    /// Task to open.
    pub id: String,
    /// Filed title or merge outcome.
    pub title: String,
    /// Filing time.
    pub at: String,
    /// Session and host that supplied the words.
    pub source: String,
    /// Opens the task.
    pub action: Action,
}

impl FiledLine {
    fn fixture(row: &FiledRow) -> FiledLine {
        FiledLine {
            id: row.id.into(),
            title: row.title.into(),
            at: row.at.into(),
            // The mockup's rows were all filed by the assistant.
            source: format!("{} {}", super::LABELS.filed_by, row.source),
            action: Action::Open(Page::Task(row.id)),
        }
    }
}

/// Agents' three assistant blocks; no assistant destination is introduced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agents {
    /// Pinned quick-thread tile or row.
    pub quick_thread: QuickThread,
    /// Assistant sessions section heading.
    pub sessions_title: &'static str,
    /// Named daemon and pane session rows, excluding the quick thread.
    pub sessions: Vec<SessionRow>,
    /// Desktop count descriptor.
    pub sessions_count: String,
    /// Starts a named daemon session.
    pub new_session: Button,
    /// Loading, stale, empty or refusal line under the sessions.
    pub sessions_note: Option<String>,
    /// Session fixture disclosure.
    pub sessions_illustrative: Option<&'static str>,
    /// Filed section heading.
    pub filed_title: &'static str,
    /// Filed rows, each naming its originating session.
    pub filed: Vec<FiledLine>,
    /// The source's total of filings beside the heading; none while the source is unknown,
    /// loading or unavailable.
    pub filed_total: Option<u64>,
    /// Loading, freshness, error, capability or empty.
    pub filed_note: Option<String>,
    /// Newest, Newer, Older or Retry for the served page.
    pub filed_pager: Vec<Button>,
    /// Filed id disclosure.
    pub filed_illustrative: Option<&'static str>,
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
        .map(fixture_row)
        .collect();
    Agents {
        quick_thread: QuickThread {
            id: "A-7".into(),
            title: "Quick thread · A-7".into(),
            row_title: "Quick thread",
            state: state_chip(state),
            pinned: "assistant · pinned",
            last_line,
            last_question: format!("{}…", question.chars().take(96).collect::<String>()),
            owner_at: "you · 22:11".into(),
            model: state.model_name().into(),
            location: "runs: daemon".into(),
            scope: "shared by your devices",
            lifetime: "live · since 14:02".into(),
            action: Action::OpenQuickThread,
        },
        sessions_title: "Assistant sessions",
        sessions_count: format!("{} · daemon and pane", rows.len()),
        sessions: rows,
        new_session: button(
            "+ New session",
            Action::Toast("New session A-19 started in corrald · separate from the quick thread"),
        ),
        sessions_note: None,
        sessions_illustrative: Some("sessions illustrative"),
        filed_title: "Filed from chat",
        filed: FILED_LIST.iter().map(FiledLine::fixture).collect(),
        filed_total: Some(FILED_LIST.len() as u64),
        filed_note: None,
        filed_pager: Vec::new(),
        filed_illustrative: Some("filed ids illustrative"),
    }
}

fn fixture_row(session: &Session) -> SessionRow {
    let cost = format!("${:.2}", session.cost);
    let focus = session
        .focus
        .map_or(String::new(), |id| format!(" · focus {id}"));
    SessionRow {
        id: session.id.into(),
        title: session.title.into(),
        state: Chip::new(
            session.state,
            if session.state.starts_with("live") {
                Hue::Worker
            } else {
                Hue::Neutral
            },
        ),
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
    pub phone_title: String,
    /// Back destination label.
    pub back_label: &'static str,
    /// State chip shown on the quick thread and on a live session.
    pub state: Option<Chip>,
    /// Host/scope subtitle.
    pub subtitle: String,
    /// Id, State, Where, Profile, Cost and Scratch.
    pub facts: Vec<Fact>,
    /// Session host lifetime explanation.
    pub lifetime_note: &'static str,
    /// Filed from this thread rows.
    pub filed: Vec<FiledLine>,
    /// End session, on a served named session corrald has not ended; it asks this device to
    /// confirm before the canonical `assistant.session` End goes out.
    pub end: Option<Button>,
}

/// The host lifetime explanation every session's facts carry; there is no idle stop.
pub(super) const LIFETIME_NOTE: &str = "The host runs until its work is done or it stops on its own (it exits, crashes, you end the session, or corrald restarts). Rolling at caps starts the next A-<n> with a summary.";

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
        phone_title: if quick { "Quick thread" } else { id }.into(),
        back_label: "Agents",
        state: quick.then(|| state_chip(state)),
        subtitle: if quick {
            "runs: daemon · one per workspace · every device"
        } else {
            "assistant session"
        }
        .into(),
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
        lifetime_note: LIFETIME_NOTE,
        filed: if quick {
            FILED_LIST
                .iter()
                .filter(|row| row.source.starts_with("A-7"))
                .map(FiledLine::fixture)
                .collect()
        } else {
            Vec::new()
        },
        end: None,
    })
}

/// Styled source command opening an app record, used by compact hotkey results.
pub(super) fn command_link(label: &'static str, page: Page) -> Rich {
    vec![span(label).mono().link(page)]
}
