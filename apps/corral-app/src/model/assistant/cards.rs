//! Cards carried by canonical assistant replies.

use super::{Action, Button, Chip};
use crate::model::hue::Hue;
use crate::model::text::Rich;

/// How a recorded owner-action card was classed when it ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionClass {
    /// Ordinary owner instruction.
    Run,
    /// Confirmation on this device before it runs.
    Confirm,
    /// Input arrived too late to instruct.
    Late,
}

impl ActionClass {
    /// Verbatim class chip text.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Run => "runs on your instruction",
            Self::Confirm => "confirm on this device",
            Self::Late => "late · cannot instruct",
        }
    }

    /// State colour for the class chip.
    pub const fn hue(self) -> Hue {
        if matches!(self, Self::Confirm) {
            Hue::You
        } else {
            Hue::Neutral
        }
    }
}

/// Receipt or explanation below an owner command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionOutcome {
    /// Successful receipt.
    Executed(String),
    /// Command declined by current facts after a fresh confirmation.
    Refused(String),
    /// Owner declined the requested action.
    Declined(String),
    /// Waiting on device confirmation, with explicit consequences.
    Waiting {
        explanation: Rich,
        buttons: Vec<Button>,
    },
    /// Submitted or running; no outcome yet.
    Pending(String),
    /// The outcome was lost; it may have run.
    Unknown(String),
    /// No command ran because this input was delivered over fifteen minutes late.
    Late {
        typed: String,
        delivered: String,
        explanation: String,
    },
}

impl ActionOutcome {
    /// Receipt stamp text; restriction explanations deliberately have no receipt stamp.
    pub const fn stamp(&self) -> Option<&'static str> {
        match self {
            Self::Executed(_) => Some("EXECUTED"),
            Self::Refused(_) => Some("REFUSED"),
            Self::Declined(_) => Some("DECLINED"),
            Self::Waiting { .. } => Some("WAITING ON YOU"),
            Self::Pending(_) => Some("RUNNING"),
            Self::Unknown(_) => Some("UNKNOWN"),
            _ => None,
        }
    }

    /// Semantic state of the receipt.
    pub const fn hue(&self) -> Hue {
        match self {
            Self::Executed(_) => Hue::Ok,
            Self::Refused(_) => Hue::Fail,
            Self::Waiting { .. } | Self::Unknown(_) => Hue::Held,
            Self::Pending(_) => Hue::Corral,
            _ => Hue::Neutral,
        }
    }
}

/// Definition labels shared by filed, edited, saved and shared Idea cards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FiledLabels {
    /// Generated or owner-derived title label.
    pub title: &'static str,
    /// Project field label.
    pub project: &'static str,
    /// Labels field label.
    pub labels: &'static str,
    /// Acceptance criteria field label.
    pub criteria: &'static str,
    /// Duplicate-check field label.
    pub duplicates: &'static str,
}

/// Verbatim field labels, so rendering never supplies domain wording.
pub const FILED_LABELS: FiledLabels = FiledLabels {
    title: "Title",
    project: "Project",
    labels: "Labels",
    criteria: "Criteria",
    duplicates: "Duplicates",
};

/// An owner action always cites the exact journaled owner-input id when one exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionCard {
    /// Heading above the command.
    pub heading: &'static str,
    /// Exact CLI operation proposed or executed.
    pub command: String,
    /// Authority class and its text.
    pub class: ActionClass,
    /// Reason confirmation or restriction applies.
    pub why: Option<String>,
    /// Journal input id, never inferred from the command.
    pub input: Option<String>,
    /// Confirmation device and time, when already given.
    pub confirmed: Option<String>,
    /// Typed receipt or no-run explanation.
    pub outcome: ActionOutcome,
}

/// A choice with its visible consequences in an ask_user prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskOption {
    /// Choice title.
    pub label: String,
    /// What choosing this does to the owner's words.
    pub consequence: String,
    /// The already-recorded answer.
    pub selected: bool,
    /// Whether another answer can still be submitted.
    pub enabled: bool,
    /// What choosing it runs, absent once answered.
    pub action: Option<Action>,
}

/// An agent's question: who asks, the question, its options, the field for the owner's own
/// words where it takes them, and how this device's answer went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskUserCard {
    /// Who asks.
    pub heading: String,
    pub question: Rich,
    /// Each answers at once when chosen.
    pub options: Vec<AskOption>,
    /// The field for the owner's own words, while the question takes them.
    pub words: Option<WordsAnswer>,
    /// How the answer sent from here went.
    pub answer: Option<Rich>,
}

/// What a typed answer to an agent's question names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WordsAnswer {
    pub session: String,
    pub question: String,
    pub placeholder: String,
}
/// An Idea's unchanged request and the fields the assistant wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdeaFields {
    /// Filed task id.
    pub id: String,
    /// Owner's words, never rewritten.
    pub words: String,
    /// Assistant-written title.
    pub title: String,
    /// Assistant-written labels.
    pub labels: Vec<String>,
    /// Assistant-written acceptance criteria.
    pub criteria: Vec<String>,
    /// Duplicate-check outcome.
    pub duplicates: String,
}

/// How the filed card's title is edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Editing {
    /// Not editing.
    No,
    /// The live title, held in `LiveState::editing` and saved with `task edit`. The owner's
    /// words and the criteria in the task body are never rewritten from here.
    Title,
}

/// An assistant-filed Idea, with the unchanged request before editable generated fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FiledCard {
    /// Filed result chip.
    pub chip: Chip,
    /// Idea fields.
    pub idea: IdeaFields,
    /// Project value.
    pub project: String,
    /// Device attribution line.
    pub provenance: Rich,
    /// Assistant attribution above the quotation.
    pub attribution: String,
    /// Quotation caption.
    pub quote_label: &'static str,
    /// Which fields are shown as editable.
    pub editing: Editing,
    /// Optional related record, already linked.
    pub related: Option<Rich>,
    /// Edit/Merge/Discard, or Save/Cancel in edit state.
    pub buttons: Vec<Button>,
    /// Discard's confirmation requirement, absent while editing.
    pub discard_note: Option<&'static str>,
    /// Status of the latest command on this Idea.
    pub status: Option<Chip>,
    /// Whether this card carries a shared attachment.
    pub attached: bool,
}
/// An owner-filed Idea made from pending text with no model invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedCard {
    /// Saved result chip.
    pub chip: Chip,
    /// Explicit no-model attribution.
    pub attribution: &'static str,
    /// Quotation caption.
    pub quote_label: &'static str,
    /// Exact owner words.
    pub words: String,
    /// First line truncated to 120 characters, plus ellipsis when truncated.
    pub title: String,
    /// Title derivation caption.
    pub title_note: &'static str,
    /// What can be added later.
    pub note: &'static str,
}

/// Builds saved-as-written content from the actual submitted text, never another message's request.
pub fn saved_as_written(id: &str, words: &str) -> SavedCard {
    let first = words.split('\n').next().unwrap_or("");
    let mut title: String = first.chars().take(120).collect();
    if first.chars().count() > 120 {
        title.push('…');
    }
    SavedCard {
        chip: Chip::new(format!("saved · {id}"), Hue::Ok),
        attribution: "Idea · filed by you, as written · no model ran",
        quote_label: "your words, unchanged",
        words: words.into(),
        title,
        title_note: "first line of your words, cut at 120 characters",
        note: "No labels or criteria yet. Ask the assistant to refine it when it can run again.",
    }
}

/// Waiting block for a stored message; never expires or files automatically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitingBlock {
    /// State/reason wording.
    pub state: Chip,
    /// Why this host cannot run, absent for compact newly-sent messages.
    pub explanation: Option<String>,
    /// Storage and eventual processing explanation.
    pub note: &'static str,
    /// Optional resume followed by File as idea now and Withdraw.
    pub buttons: Vec<Button>,
    /// Status of a File as idea now or Withdraw already sent.
    pub status: Option<Chip>,
    /// Expiry and auto-filing policy.
    pub lifetime: &'static str,
}

/// Waiting never silently expires or files the owner's words.
pub(super) const WAIT_LIFETIME: &str = "It waits until the assistant can run, you file it, or you withdraw it. It never expires and corrald never files it by itself.";
/// The full waiting block's storage note.
pub(super) const WAIT_NOTE: &str = "Your message is stored and every device shows it; the assistant takes it as soon as it can run.";
