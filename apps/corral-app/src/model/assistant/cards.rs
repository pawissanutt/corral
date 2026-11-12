//! Typed cards carried inside assistant replies and reused by Inbox and share content.

use super::views::button;
use super::{Action, Button, Chip, Filed, State, WaitReason, filed};
use crate::model::effect::{Env, Page, Risk};
use crate::model::hue::Hue;
use crate::model::nav::Form;
use crate::model::text::{Rich, span, task_link};

/// Authority class displayed on an owner-action card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionClass {
    /// Ordinary owner instruction.
    Run,
    /// Fresh confirmation required on this device.
    Confirm,
    /// Web client is paired for asking and filing only.
    AskAndFileOnly,
    /// Input arrived too late to instruct.
    Late,
}

impl ActionClass {
    /// Verbatim class chip text.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Run => "runs on your instruction",
            Self::Confirm => "confirm on this device",
            Self::AskAndFileOnly => "this device may ask and file only",
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
    /// No command ran because this client lacks instruct permission.
    AskAndFileOnly(Rich),
    /// No command ran because this input was delivered over fifteen minutes late.
    Late {
        typed: &'static str,
        delivered: &'static str,
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
            _ => None,
        }
    }

    /// Semantic state of the receipt.
    pub const fn hue(&self) -> Hue {
        match self {
            Self::Executed(_) => Hue::Ok,
            Self::Refused(_) => Hue::Fail,
            Self::Waiting { .. } => Hue::Held,
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

/// An owner action always cites the exact journaled owner-input id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionCard {
    /// Heading above the command.
    pub heading: &'static str,
    /// Exact CLI operation proposed or executed.
    pub command: &'static str,
    /// Authority class and its text.
    pub class: ActionClass,
    /// Reason confirmation or restriction applies.
    pub why: Option<&'static str>,
    /// Journal input id, never inferred from the command.
    pub input: &'static str,
    /// Confirmation device and time, when already given.
    pub confirmed: Option<&'static str>,
    /// Typed receipt or no-run explanation.
    pub outcome: ActionOutcome,
}

/// The mode-pause action shared by the thread and pending Inbox prompt.
pub fn mode_pause_card() -> ActionCard {
    let mut confirm = button(
        "Confirm on this device",
        Action::Confirm(Risk::ModePause, None),
    );
    confirm.primary = true;
    ActionCard {
        heading: "Owner action",
        command: "corral mode pause",
        class: ActionClass::Confirm,
        why: Some("mode is confirm-class"),
        input: "A-7/g2/i19",
        confirmed: None,
        outcome: ActionOutcome::Waiting {
            explanation: vec![
                span("Nothing runs until you confirm here; Decline records a "),
                span("declined").bold(),
                span(" receipt."),
            ],
            buttons: vec![
                confirm,
                button(
                    "Decline",
                    Action::Toast("Declined · receipt recorded · nothing changed"),
                ),
            ],
        },
    }
}

/// A choice with its visible consequences in an ask_user prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskOption {
    /// Choice title.
    pub label: &'static str,
    /// What choosing this does to the owner's words.
    pub consequence: &'static str,
    /// The already-recorded answer.
    pub selected: bool,
    /// Whether another answer can still be submitted.
    pub enabled: bool,
}

/// The already-answered duplicate check, before the subsequent merged card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskUserCard {
    /// Prompt provenance and time.
    pub heading: &'static str,
    /// Linked likely duplicate and question.
    pub question: Rich,
    /// Merge and file-separately choices, with consequences.
    pub options: Vec<AskOption>,
    /// Answer time, input id and response surface.
    pub answer: Rich,
}

/// Builds the fixture's duplicate-check prompt with its recorded merge choice.
pub fn duplicate_card() -> AskUserCard {
    AskUserCard {
        heading: "assistant asks · ask_user · before filing · 22:04",
        question: vec![
            span("This looks close to "),
            task_link("T-874"),
            span(" Pi review annotations: durable candidate-bound threads").dim(),
            span(". Merge, or file separately?"),
        ],
        options: vec![
            AskOption {
                label: "Merge into T-874",
                consequence: "Your words become a comment on T-874. Nothing new is filed.",
                selected: true,
                enabled: false,
            },
            AskOption {
                label: "File separately",
                consequence: "A new Idea, linked to T-874 as related. It can be merged later.",
                selected: false,
                enabled: false,
            },
        ],
        answer: vec![
            span("Answered inline by you after 40 s · "),
            span("A-7/g2/i18").mono(),
            span(" · dialog"),
        ],
    }
}

/// A merge appends unchanged words to the existing task instead of filing anything new.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergedCard {
    /// Result chip.
    pub chip: Chip,
    /// Linked task and title.
    pub destination: Rich,
    /// Quotation caption.
    pub quote_label: &'static str,
    /// Owner words, unmodified.
    pub words: &'static str,
    /// Attribution and explicit no-new-task outcome.
    pub attribution: &'static str,
}

/// Builds the merged outcome of the T-874 duplicate check.
pub fn merged_card() -> MergedCard {
    MergedCard {
        chip: Chip {
            label: "merged · T-874".into(),
            hue: Hue::Neutral,
        },
        destination: vec![
            span("comment added to "),
            task_link("T-874"),
            span(" Pi review annotations: durable candidate-bound threads"),
        ],
        quote_label: "appended as a comment, your words unchanged",
        words: "review diffs need an inline comment box tied to the candidate SHA",
        attribution: "added by assistant A-7 from your message · nothing new filed",
    }
}

/// A permission grant's duration, or explicit denial.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grant {
    /// Only this operation, no user-presence prompt.
    Once,
    /// Standing grant for one hour.
    Hour,
    /// Standing grant for the current session.
    Session,
    /// Record denial for this turn.
    Deny,
}

impl Grant {
    /// Standing grants on phone require Face ID or PIN before the action is dispatched.
    pub const fn requires_presence(self, env: Env) -> bool {
        matches!(env.form, Form::Phone) && matches!(self, Self::Hour | Self::Session)
    }

    /// Illustrative outcome after any required presence check has succeeded.
    pub const fn outcome(self, env: Env) -> &'static str {
        match self {
            Self::Once => "Allowed once · permission input recorded",
            Self::Hour if matches!(env.form, Form::Phone) => {
                "Face ID confirmed · allowed for 1 hour on ~/miti-workspace/notes"
            }
            Self::Hour => "Allowed for 1 hour on ~/miti-workspace/notes",
            Self::Session if matches!(env.form, Form::Phone) => {
                "Face ID confirmed · allowed for this session on ~/miti-workspace/notes"
            }
            Self::Session => "Allowed for this session on ~/miti-workspace/notes",
            Self::Deny => "Denied · recorded · not asked again this turn",
        }
    }
}

/// One added line of the write-permission preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    /// Destination line number.
    pub line: u32,
    /// Addition marker.
    pub marker: &'static str,
    /// Exact preview text.
    pub text: &'static str,
}

/// A permission button plus its phone presence requirement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionChoice {
    /// Grant or deny button.
    pub button: Button,
    /// Gate activation behind Face ID or PIN when true.
    pub requires_presence: bool,
    /// Presence prompt wording, absent for one-shot approval and denial.
    pub presence_prompt: Option<&'static str>,
}

/// File-write permission prompt, reused in the thread and Inbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionCard {
    /// Request heading and timestamp.
    pub heading: &'static str,
    /// Unanswered state.
    pub state: Chip,
    /// File and replacement summary.
    pub path: &'static str,
    /// Illustrative preview lines, not a complete six-line patch.
    pub lines: Vec<DiffLine>,
    /// Three grants and a denial, already resolved for phone presence rules.
    pub choices: Vec<PermissionChoice>,
    /// Explanation of standing grants.
    pub presence_note: &'static str,
    /// Prompt deadline and first-answer-wins semantics.
    pub timer_note: &'static str,
    /// Preview disclosure.
    pub illustrative: &'static str,
}

/// Builds the write prompt; a renderer must complete `requires_presence` before dispatching a standing grant.
pub fn permission_card(env: Env) -> PermissionCard {
    let choices = [
        ("Allow once", Grant::Once),
        ("Allow for 1 hour", Grant::Hour),
        ("Allow for this session", Grant::Session),
        ("Deny", Grant::Deny),
    ]
    .into_iter()
    .map(|(label, grant)| {
        let mut button = button(label, Action::Grant(grant));
        button.primary = grant == Grant::Once;
        button.danger = grant == Grant::Deny;
        let requires_presence = grant.requires_presence(env);
        PermissionChoice {
            button,
            requires_presence,
            presence_prompt: requires_presence.then_some("Face ID or PIN"),
        }
    })
    .collect();
    PermissionCard {
        heading: "assistant wants to write · edit · 22:05",
        state: Chip {
            label: "waiting on you".into(),
            hue: Hue::You,
        },
        path: "~/miti-workspace/notes/corral-log.md · replacement 1 · +6 lines",
        lines: vec![
            DiffLine {
                line: 41,
                marker: "+",
                text: "## 2026-11-07 night",
            },
            DiffLine {
                line: 42,
                marker: "+",
                text: "- T-867 stale on 5949a4b (base moved), re-gate queued",
            },
            DiffLine {
                line: 43,
                marker: "+",
                text: "- T-870 waits on T-868/R1/Q1",
            },
        ],
        choices,
        presence_note: "On a phone, the two standing grants ask for Face ID or PIN first; Allow once and Deny do not.",
        timer_note: "The tool waited 120 s, then deferred; this prompt stays answerable here and in Inbox until 22:05 tomorrow. One answer wins.",
        illustrative: "path and preview illustrative",
    }
}

/// An assistant-filed Idea, with the unchanged request before editable generated fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FiledCard {
    /// Filed result chip.
    pub chip: Chip,
    /// Full immutable fixture fields.
    pub idea: Filed,
    /// Project value.
    pub project: &'static str,
    /// Device attribution line.
    pub provenance: Rich,
    /// Assistant attribution above the quotation.
    pub attribution: &'static str,
    /// Quotation caption.
    pub quote_label: &'static str,
    /// Whether title, project and criteria are shown as editable fields.
    pub editing: bool,
    /// Optional related record, already linked.
    pub related: Option<Rich>,
    /// Edit/Merge/Discard, or Save/Cancel in edit state.
    pub buttons: Vec<Button>,
    /// Discard's confirmation requirement, absent while editing.
    pub discard_note: Option<&'static str>,
    /// Whether this card carries a shared attachment.
    pub attached: bool,
}

/// Builds a known filed card; unknown ids never borrow another Idea's content.
pub fn filed_card(state: &State, id: &str, attached: bool) -> Option<FiledCard> {
    let idea = *filed(id)?;
    let editing = state.edit == Some(idea.id);
    let buttons = if editing {
        let mut save = button("Save", Action::Edit(None));
        save.primary = true;
        vec![save, button("Cancel", Action::Edit(None))]
    } else {
        let mut discard = button("Discard", Action::Confirm(Risk::Discard, Some(idea.id)));
        discard.danger = true;
        vec![
            button("Edit", Action::Edit(Some(idea.id))),
            button("Merge into T-…", Action::Merge(idea.id)),
            discard,
        ]
    };
    Some(FiledCard {
        chip: Chip {
            label: format!("filed · {}", idea.id),
            hue: Hue::Ok,
        },
        idea,
        project: "corral",
        provenance: vec![
            span("Idea · project "),
            span("corral").bold(),
            span(" · "),
            span(format!("via=app:{}", idea.device)).mono(),
        ],
        attribution: "filed by assistant A-7 from your message · your words kept as the request",
        quote_label: "your words, unchanged",
        editing,
        related: idea
            .related
            .map(|id| vec![span("· related ").dim(), task_link(id)]),
        buttons,
        discard_note: (!editing).then_some("Discard asks on this device"),
        attached,
    })
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
pub fn saved_as_written(id: &'static str, words: &str) -> SavedCard {
    let first = words.split('\n').next().unwrap_or("");
    let mut title: String = first.chars().take(120).collect();
    if first.chars().count() > 120 {
        title.push('…');
    }
    SavedCard {
        chip: Chip {
            label: format!("saved · {id}"),
            hue: Hue::Ok,
        },
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
    pub explanation: Option<&'static str>,
    /// Storage and eventual processing explanation.
    pub note: &'static str,
    /// Optional resume followed by File as idea now and Withdraw.
    pub buttons: Vec<Button>,
    /// Expiry and auto-filing policy.
    pub lifetime: &'static str,
}

/// Builds a full host wait or a compact wait for a just-submitted message.
pub fn waiting_block(reason: WaitReason, full: bool) -> WaitingBlock {
    let mut buttons = Vec::new();
    if full && reason == WaitReason::HostDown {
        buttons.push(button(
            "Resume A-7",
            Action::SetMode(super::Mode::Answering),
        ));
    }
    buttons.push(button("File as idea now", Action::FileNow));
    buttons.push(button("Withdraw", Action::Withdraw));
    WaitingBlock {
        state: Chip {
            label: format!("waiting for assistant · {}", reason.label()),
            hue: Hue::Held,
        },
        explanation: full.then(|| reason.explanation()),
        note: if full {
            "Your message is stored and every device shows it; the assistant takes it as soon as it can run."
        } else {
            ""
        },
        buttons,
        lifetime: "It waits until the assistant can run, you file it, or you withdraw it. It never expires and corrald never files it by itself.",
    }
}

/// The shared link's filed outcome after delivery from the phone outbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedFiledCard {
    /// Filed result chip.
    pub chip: Chip,
    /// Card's project/type facts.
    pub description: &'static str,
    /// Shared-link chip.
    pub attachment: &'static str,
    /// Assistant/source attribution.
    pub attribution: &'static str,
    /// Generated title.
    pub title: &'static str,
    /// Generated labels and related task.
    pub labels: Rich,
    /// Edit, merge and device-confirmed discard.
    pub buttons: Vec<Button>,
}

/// Builds the T-905 outcome of the captionless fixture share.
pub fn shared_filed_card() -> SharedFiledCard {
    let mut discard = button("Discard", Action::Confirm(Risk::Discard, Some("T-905")));
    discard.danger = true;
    SharedFiledCard {
        chip: Chip {
            label: "filed · T-905".into(),
            hue: Hue::Ok,
        },
        description: "Idea · project corral",
        attachment: "link attached",
        attribution: "filed by assistant A-7 from your share",
        title: "Review: evaluate NSH DiffView annotations for inline comments",
        labels: vec![
            span("review"),
            span(" "),
            span("nsh"),
            span(" · related ").dim(),
            span("T-874").mono().link(Page::Task("T-874")),
        ],
        buttons: vec![
            button("Edit", Action::Edit(Some("T-905"))),
            button("Merge into T-…", Action::Merge("T-905")),
            discard,
        ],
    }
}
