//! Content for Ask's drawer/sheet, the small global window and the phone share extension.

use super::views::{button, command_link};
use super::{
    Action, ActionCard, Button, Chip, Composer, HOTKEY_KEYS, HotkeyState, KeyHint, PermissionCard,
    ShareStep, SharedFiledCard, State, ThreadItem, composer, mode_pause_card, permission_card,
    shared_filed_card, state_chip, thread,
};
use crate::model::effect::{Conn, Env, Page};
use crate::model::hue::Hue;
use crate::model::nav::{AskTarget, Form};
use crate::model::text::{Rich, span};

/// Content in the desktop Ask drawer or the phone composer sheet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drawer {
    /// Dialog/accessibility name.
    pub label: &'static str,
    /// Drawer title, or Ask on phone.
    pub title: &'static str,
    /// Assistant state, hidden by the source phone sheet.
    pub state: Option<Chip>,
    /// Cross-device host scope, desktop only.
    pub subtitle: Option<&'static str>,
    /// Open in Agents or quick-thread sheet link, then Close.
    pub buttons: Vec<Button>,
    /// Quick transcript on desktop/terminal; the phone sheet contains only the composer.
    pub items: Vec<ThreadItem>,
    /// All target, model, chip, input and send content.
    pub composer: Composer,
    /// Phone sheet requests the system keyboard.
    pub keyboard_requested: bool,
    /// Label used by Escape and close accessibility.
    pub close_label: &'static str,
}

/// Builds Ask, taking the app-owned target and draft explicitly.
pub fn drawer(state: &State, target: AskTarget, text: &str, env: Env) -> Drawer {
    let phone = env.form == Form::Phone;
    Drawer {
        label: "Ask",
        title: if phone { "Ask" } else { "Quick thread · A-7" },
        state: (!phone).then(|| state_chip(state)),
        subtitle: (!phone).then_some("shared by your devices · runs in corrald"),
        buttons: vec![
            button(
                if phone {
                    "Quick thread · A-7 ›"
                } else {
                    "Open in Agents"
                },
                Action::OpenQuickThread,
            ),
            button("Close", Action::CloseAsk),
        ],
        items: if phone {
            Vec::new()
        } else {
            thread(state, "A-7", env)
        },
        composer: composer(state, target, text, None, env),
        keyboard_requested: phone,
        close_label: "Close (Esc)",
    }
}

/// A compact filed result in the small global window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactFiled {
    /// Filed outcome chip.
    pub chip: Chip,
    /// Idea title.
    pub title: &'static str,
    /// Unchanged-words/duplicate-check explanation.
    pub note: &'static str,
}

/// Result area of the global-hotkey window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotkeyResult {
    /// Typing has no result area yet.
    Typing,
    /// Answer and filing in one result.
    AnswerAndFiled {
        /// Answer grounded in the board snapshot.
        answer: Rich,
        /// Linked supporting commands.
        sources: Vec<Rich>,
        /// Idea filed by the same turn.
        filed: CompactFiled,
    },
    /// A message that was only an idea.
    Filed(CompactFiled),
}

impl HotkeyResult {
    /// Heading beside the compact answer's supporting command links.
    pub const fn sources_label(&self) -> Option<&'static str> {
        match self {
            Self::AnswerAndFiled { .. } => Some("Rests on"),
            _ => None,
        }
    }
}

/// The small Ask window content, independent of the illustrated Windows desktop behind it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeyWindow {
    /// Application and window names.
    pub title: &'static str,
    /// Brand word in the window header.
    pub brand: &'static str,
    /// Dim surface name beside the brand.
    pub name: &'static str,
    /// Accessible label of the window's hide control.
    pub hide: &'static str,
    /// Delivery state of the last submission from this window.
    pub sent: Option<Chip>,
    /// Per-state input value in the source fixture.
    pub input: &'static str,
    /// Fixed Assistant target and model picker content.
    pub composer: Composer,
    /// Per-state answer or filed result.
    pub result: HotkeyResult,
    /// Result author/state/cost attribution, absent while typing.
    pub attribution: Option<&'static str>,
    /// Reply scope, absent while typing.
    pub reply_note: Option<&'static str>,
    /// Open the main Corral quick thread, absent while typing.
    pub open: Option<Button>,
    /// Keyboard hints in their source order.
    pub keys: &'static [KeyHint],
    /// Why this window remains available without the main window.
    pub lifetime: &'static str,
    /// Current reachability description.
    pub reach: &'static str,
    /// The illustrated terminal transcript behind the window.
    pub terminal: Rich,
    /// Illustration disclosure.
    pub illustrative: &'static str,
    /// Notification-area interaction explanation.
    pub tray_label: &'static str,
    /// Snapshot clock.
    pub clock: &'static str,
    /// Snapshot date.
    pub date: &'static str,
}

/// Source terminal examples, including the deliberate CLI/no-daemon distinction from queued app chat.
pub fn terminal_examples() -> Rich {
    vec![
        span("$ corral ask \"why is T-867 stale?\"\n").mono(),
        span("quick thread A-7 · assistant · answering\n").dim(),
        span("T-867 is stale because the base moved: its gate ran on\ncandidate 5949a4b, then main moved at 22:05, so that verdict\nbelongs to an older candidate. Corral queued a re-gate.\n"),
        span("  rests on: ").dim(), span("corral show T-867").mono().link(Page::Task("T-867")), span(" · "), span("corral log T-867 --since 20:40").mono().link(Page::Session("T-867")), span(" · this ask $0.02\n").dim(),
        span("$ corral ask \"board: show the PM inbox count in the title bar\"\n").mono(),
        span("filed · T-902").hue(Hue::Ok), span(" Board: PM inbox count in the title bar · corral · your words kept\n"),
        span("# assistant cannot run (daemon up): the message is kept, corral ask returns\n").dim(),
        span("$ corral ask \"summarise tonight's stalls\"\n").mono(),
        span("waiting for assistant · provider quota · kept in the quick thread\n").hue(Hue::Held),
        span("# with corrald down, corral ask refuses and names the no-daemon path\n").dim(),
        span("$ corral ask \"what landed today?\"\n").mono(),
        span("corral ask: corrald is not running; nothing was sent. To keep an idea anyway: corral new --idea \"<text>\"\n").hue(Hue::Fail), span("$ ").mono(),
    ]
}

/// Builds all three hotkey states; the model picker remains present in every state.
pub fn hotkey_window(state: &State, env: Env) -> HotkeyWindow {
    let input = match state.hotkey {
        HotkeyState::Typing => "why is T-867 sta",
        HotkeyState::Filed => "board: show the PM inbox count in the title bar",
        HotkeyState::Answer => {
            "why is T-867 stale? we should cache gate receipts so this stops happening"
        }
    };
    let result = match state.hotkey {
        HotkeyState::Typing => HotkeyResult::Typing,
        HotkeyState::Filed => HotkeyResult::Filed(CompactFiled {
            chip: Chip {
                label: "filed · T-902".into(),
                hue: Hue::Ok,
            },
            title: "Board: PM inbox count in the title bar",
            note: "your words kept as written · duplicates checked: nothing close",
        }),
        HotkeyState::Answer => HotkeyResult::AnswerAndFiled {
            answer: vec![
                span("T-867").mono().link(Page::Task("T-867")),
                span(" is stale because the base moved: its gate ran on "),
                span("5949a4b").mono().link(Page::Task("T-867")),
                span(
                    " and main moved at 22:05. Corral queued a re-gate; nothing is needed from you.",
                ),
            ],
            sources: vec![
                command_link("corral show T-867", Page::Task("T-867")),
                command_link("corral log T-867", Page::Session("T-867")),
            ],
            filed: CompactFiled {
                chip: Chip {
                    label: "filed · T-903".into(),
                    hue: Hue::Ok,
                },
                title: "Gate: reuse receipts for unchanged crates after a base move",
                note: "“we should cache gate receipts…” kept as written",
            },
        },
    };
    let result_visible = state.hotkey != HotkeyState::Typing;
    HotkeyWindow {
        title: "Corral · Ask",
        brand: "Corral",
        name: "Ask",
        hide: "Hide",
        sent: state.hotkey_sent.map(|state| Chip {
            label: state.label(),
            hue: state.hue(),
        }),
        input,
        composer: composer(state, AskTarget::Assistant, input, None, env),
        result,
        attribution: if !result_visible {
            None
        } else if state.hotkey == HotkeyState::Filed {
            Some("assistant A-7 · filed · this ask $0.02")
        } else {
            Some("assistant A-7 · answered and filed · this ask $0.02")
        },
        reply_note: result_visible.then_some("Reply continues the quick thread"),
        open: result_visible.then(|| button("Open in Corral", Action::HotkeyOpen)),
        keys: HOTKEY_KEYS,
        lifetime: "main window closed · runs from the notification area",
        reach: match env.conn {
            Conn::Relay => "relay · e2e",
            Conn::Offline => "offline · last sync 22:08",
            _ => "direct · loopback",
        },
        terminal: terminal_examples(),
        illustrative: "Windows desktop, terminal and taskbar drawn for illustration; the Corral main window is closed, the daemon runs in WSL2",
        tray_label: "Corral: health dot; click opens Ask",
        clock: "22:12",
        date: "07/11/2026",
    }
}

/// Shared browser page shown behind the system sheet and Corral extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SharedPage {
    /// Page title.
    pub title: &'static str,
    /// Browser URL.
    pub url: &'static str,
    /// Share preview title.
    pub attachment_title: &'static str,
    /// Absolute attachment URL.
    pub attachment_url: &'static str,
    /// Page excerpt.
    pub excerpt: &'static str,
    /// Clarifies that the browser is outside Corral.
    pub note: &'static str,
}

/// The illustrative NSH DiffView page being shared.
pub const SHARED_PAGE: SharedPage = SharedPage {
    title: "DiffView",
    url: "nsh.dev/elements/diff-view",
    attachment_title: "DiffView · Native Shell",
    attachment_url: "https://nsh.dev/elements/diff-view",
    excerpt: "The DiffView component: the changes between two texts or of a patch, side by side or unified, with the words changed within lines highlighted, unchanged lines collapsed into separators that expand…",
    note: "Another app (browser), not Corral.",
};

/// One system-share app; only Corral has a simulated activation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareApp {
    /// App label.
    pub label: &'static str,
    /// Corral opens its caption extension; other apps have no mock action.
    pub action: Option<Action>,
}

/// Share extension body with every per-step branch already resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShareContent {
    /// System share sheet app chooser.
    Sheet {
        /// Apps shown by the source mock.
        apps: Vec<ShareApp>,
        /// Additional system actions.
        actions: &'static [&'static str],
        /// System illustration disclosure.
        illustrative: &'static str,
    },
    /// Optional caption followed by Send; no network work in the extension.
    Compose {
        /// App-owned caption draft.
        caption: String,
        /// Caption placeholder.
        placeholder: &'static str,
        /// Scope, captionless behavior and provenance.
        note: Rich,
    },
    /// On-device outbox item, not delivered yet.
    Saved {
        /// Locally saved state.
        state: Chip,
        /// Connection-aware eventual-delivery explanation.
        note: &'static str,
        /// Opens Corral and advances the illustrative delivery step.
        open: Button,
    },
    /// Captionless share filed after the app delivered it.
    Filed {
        /// Explains the assistant's classification.
        note: &'static str,
        /// T-905 with the attached link.
        card: SharedFiledCard,
        /// Closes the share extension.
        done: Button,
    },
}

/// Phone share sheet content; presence of `None` means no overlay is shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareSheet {
    /// Underlying browser page and attachment metadata.
    pub page: SharedPage,
    /// Accessible dialog name.
    pub title: &'static str,
    /// Cancel affordance for the extension, absent for the OS chooser.
    pub cancel: Option<Button>,
    /// Send affordance only in caption composition.
    pub send: Option<Button>,
    /// Whether the shared link attachment is shown.
    pub attachment_visible: bool,
    /// Per-step body.
    pub content: ShareContent,
    /// System keyboard is requested only for the optional caption.
    pub keyboard_requested: bool,
}

/// Builds all phone share stages. Captionless delivery is the fixture's T-905 case;
/// the extension saves locally even when the daemon is healthy.
pub fn share_sheet(state: &State, caption: &str, env: Env) -> Option<ShareSheet> {
    let step = state.share?;
    let content = match step {
        ShareStep::Sheet => ShareContent::Sheet {
            apps: ["AirDrop", "Messages", "Mail", "Corral", "Notes"]
                .into_iter()
                .map(|label| ShareApp {
                    label,
                    action: (label == "Corral").then_some(Action::Share(Some(ShareStep::Compose))),
                })
                .collect(),
            actions: &["Copy", "Add to Reading List", "Add Bookmark"],
            illustrative: "system share sheet, drawn for illustration",
        },
        ShareStep::Compose => ShareContent::Compose {
            caption: caption.into(),
            placeholder: "Add a caption (optional)",
            note: vec![
                span("Lands in the quick thread · with no caption the assistant files it · "),
                span("via=app:pawis-iphone").mono(),
            ],
        },
        ShareStep::Saved => ShareContent::Saved {
            state: Chip {
                label: "saved on this phone".into(),
                hue: Hue::Held,
            },
            note: if env.conn.refuses() {
                "sends when Corral connects (offline now)."
            } else {
                "sends when Corral connects."
            },
            open: button("Open Corral", Action::Share(Some(ShareStep::Delivered))),
        },
        ShareStep::Delivered => ShareContent::Filed {
            note: "In the quick thread · no caption, so the assistant filed it:",
            card: shared_filed_card(),
            done: button("Done", Action::Share(None)),
        },
    };
    Some(ShareSheet {
        page: SHARED_PAGE,
        title: if step == ShareStep::Sheet {
            "Share"
        } else {
            "Share to Corral"
        },
        cancel: (step != ShareStep::Sheet).then(|| button("Cancel", Action::Share(None))),
        send: (step == ShareStep::Compose)
            .then(|| button("Send", Action::Share(Some(ShareStep::Saved)))),
        attachment_visible: step != ShareStep::Delivered,
        content,
        keyboard_requested: step == ShareStep::Compose,
    })
}

/// Pending assistant prompt kinds that screens may combine with Inbox-owned selection state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prompt {
    /// Deferred file write.
    Permission,
    /// Confirm-class mode change.
    Confirmation,
}

/// One assistant-owned Inbox row; filing ideas never creates an Inbox row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingPrompt {
    /// Prompt identity.
    pub prompt: Prompt,
    /// Leading attribution.
    pub lead: &'static str,
    /// Record/command id.
    pub id: &'static str,
    /// Pending question.
    pub title: &'static str,
    /// Choice/class summary.
    pub meta: &'static str,
    /// Age in the snapshot.
    pub age: &'static str,
}

/// Assistant's two pending prompts counted by Needs you.
pub const PENDING_PROMPTS: &[PendingPrompt] = &[
    PendingPrompt {
        prompt: Prompt::Permission,
        lead: "Assistant A-7 asks",
        id: "write permission",
        title: "Write ~/miti-workspace/notes/corral-log.md (+6 lines)?",
        meta: "Allow once · 1 hour · session · Deny",
        age: "7m",
    },
    PendingPrompt {
        prompt: Prompt::Confirmation,
        lead: "Assistant A-7 waits",
        id: "corral mode pause",
        title: "Confirm on this device: pause autopilot for tonight",
        meta: "confirm on this device · mode",
        age: "10m",
    },
];

/// Prompt body for an Inbox detail pane; no cross-domain model imports are needed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptCard {
    /// Deferred write grant with phone presence requirements.
    Permission(PermissionCard),
    /// Waiting device-confirmed action.
    Confirmation(ActionCard),
}

/// Assistant-owned content for an Inbox prompt detail pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptPane {
    /// Needs-you state.
    pub state: Chip,
    /// Origin explanation.
    pub origin: &'static str,
    /// Detail heading.
    pub title: &'static str,
    /// Typed card shared with the quick thread.
    pub card: PromptCard,
    /// Cross-device single-winner explanation.
    pub note: &'static str,
    /// Opens the original quick thread.
    pub open: Button,
}

/// Builds either pending assistant prompt independently of Inbox's selection enum.
pub fn prompt_pane(prompt: Prompt, env: Env) -> PromptPane {
    PromptPane {
        state: Chip {
            label: "needs you".into(),
            hue: Hue::You,
        },
        origin: "from the quick thread · A-7 · you were not in the thread",
        title: if prompt == Prompt::Permission {
            "Assistant wants to write a file"
        } else {
            "Confirm corral mode pause"
        },
        card: match prompt {
            Prompt::Permission => PromptCard::Permission(permission_card(env)),
            Prompt::Confirmation => PromptCard::Confirmation(mode_pause_card()),
        },
        note: "One answer wins: answering here also settles it in the quick thread on every device.",
        open: button("Open the quick thread", Action::OpenQuickThread),
    }
}
