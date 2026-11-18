//! Shared composer, assistant-list and thread-header descriptors.

use super::{Action, StagedChip};
use crate::model::hue::Hue;
use crate::model::nav::AskTarget;
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
    /// Optional profile-source disclosure.
    pub illustrative: Option<&'static str>,
}

/// The list's footer: where the profiles are configured.
pub(super) fn model_footer() -> Rich {
    vec![
        span(
            "Applies to your next message; the conversation is kept. API spending needs separate finite approval. List: ",
        ),
        span("profiles").mono(),
        span(" · Settings › Assistant"),
    ]
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
    /// Removable default filing project, independent of the bounded record brief.
    pub project_chip: Option<ContextChip>,
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

/// The assistant sessions block below the Agents roster.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agents {
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
    /// Optional session-source disclosure.
    pub sessions_illustrative: Option<&'static str>,
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
    /// The session whose Session menu (Cancel turn, Pause, Resume, Terminate, Relaunch) the
    /// head carries; none while no control applies.
    pub menu: Option<crate::model::Id>,
}

/// The host lifetime explanation every session's facts carry; there is no idle stop.
pub(super) const LIFETIME_NOTE: &str = "The host runs until its work is done or it stops on its own (it exits, crashes, you end the session, or corrald restarts). Rolling at caps starts the next A-<n> with a summary.";
