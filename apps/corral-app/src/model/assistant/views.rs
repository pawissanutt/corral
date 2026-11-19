//! Shared descriptors: chips, buttons, the context chip and what Ask adds to the session
//! composer.

use super::{Action, StagedChip};
use crate::model::hue::Hue;
use crate::model::text::Rich;

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

/// A button descriptor.
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

/// A removable chip above the draft: the record Ask is about, or the default filing project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextChip {
    /// Kind, such as task or project.
    pub kind: String,
    /// Item id and descriptive name.
    pub label: String,
    /// Removal action.
    pub remove: Button,
    /// Scope explanation beside the chip.
    pub note: &'static str,
}

/// Ask's field (its test id), which takes the keyboard focus whenever Ask opens and when a
/// press that removed what it was on hands the focus back.
pub const COMPOSER_FIELD: &str = "ask-input";

/// A keyboard cap and its adjacent explanatory word or phrase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyHint {
    /// Text rendered inside the keyboard cap.
    pub keys: &'static str,
    /// Explanation following the cap.
    pub action: &'static str,
}

/// Ask's keys in source order, as the terminal's key line names them.
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
        keys: "Ctrl ⇧ M",
        action: "model",
    },
];

/// What Ask adds to the session composer it shares with the session page: the chips over the
/// draft, the staged attachments and the controls that stage them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Composer {
    /// The record Ask is about: its words travel with the message.
    pub chip: Option<ContextChip>,
    /// The default filing project, independent of the record chip.
    pub project_chip: Option<ContextChip>,
    /// Upload, waiting or failed attachment line above the composer.
    pub status: Option<Rich>,
    /// Screenshot, Link and File. They attach for real and are dimmed only for a missing
    /// capability, the link (uploads) or a full message.
    pub attachments: Vec<Button>,
    /// What is staged for the next message, in the context chip's anatomy.
    pub staged: Vec<StagedChip>,
    /// Why an attachment button is dimmed.
    pub attach_note: Option<String>,
    /// Why Send waits on the staged attachments, while it does.
    pub holds_send: Option<String>,
}

/// The removable chip's note and remove control.
pub(super) fn context_chip(kind: String, label: String) -> ContextChip {
    ContextChip {
        kind,
        label,
        remove: button("Remove context", Action::Unchip),
        note: "its id goes with your message",
    }
}
