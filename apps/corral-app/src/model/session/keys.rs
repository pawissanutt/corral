//! The session page's keys: one catalog of bindings (`keys()`), each naming what it does and the
//! forms that bind it, the reducer that turns a pressed key into the page's action (`press`), and
//! the legend the terminal's key line and the composer's hint show for the focused surface.
//! Every part of the page adds its own bindings through its `keys()` and resolves them in its
//! `press`, which answers only while its tab (or the sidebar) has the keys.
//!
//! The terminal UI's keys are remapped where another owner holds them: the web build never binds
//! a key the browser keeps for itself (`WEB_RESERVED`: help, reload, full screen, the address
//! bar, new and closed tabs and windows, print), whatever a part declares; the model picker is
//! Ctrl+Shift+M, not Ctrl+L; thinking is Alt+H, not Ctrl+T; Tab and Shift+Tab keep walking the
//! focus. A chord (Alt, Ctrl, a function key, Escape) is the page's while it is open, even in the
//! composer's field; a bare key (a letter, an arrow) is a view's and is taken only while no field
//! is edited, so typing in the composer is never swallowed.

use corral_protocol::session_state::SendPolicy;

use crate::model::agents::Control;
use crate::model::nav::Form;

use super::transcript::EXPAND_ALL;
use super::{
    Action, CommandAction, Cx, Focus, PickerAction, PickerKind, SessionRole, Tab, board, changes,
    dock, fleet, proof, questions, sidebar, task, usage,
};

/// The keys a browser keeps for itself, as `state/keys.rs::stroke` names them (`mod` written as
/// `ctrl`): a page that binds them either never sees them or takes them from the browser.
pub const WEB_RESERVED: &[&str] = &[
    "f1",
    "f3",
    "f5",
    "f6",
    "f7",
    "f11",
    "f12",
    "tab",
    "shift+tab",
    "ctrl+l",
    "ctrl+t",
    "ctrl+n",
    "ctrl+w",
    "ctrl+p",
    "ctrl+r",
    "ctrl+q",
    "ctrl+tab",
    "ctrl+shift+tab",
    "ctrl+shift+t",
    "ctrl+shift+n",
    "ctrl+shift+w",
    "ctrl+shift+r",
];

/// What a binding does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    /// Open the tab at this position (Alt+1…Alt+6).
    TabAt(usize),
    /// Open this tab by its terminal UI key.
    Tab(Tab),
    /// Close the open picker or `/` menu; back to Chat and the composer from a view; stop the
    /// turn from the composer.
    Escape,
    /// Show or hide every thinking block.
    Thinking,
    /// Open or fold every tool row (Ctrl+O).
    ExpandAll,
    /// Back to the transcript's newest rows (End with an empty draft, Ctrl+End).
    Jump,
    /// Open the model picker (Ctrl+Shift+M); bound app-wide by `state/keys.rs`.
    Model,
    /// The current model's next effort level.
    Effort,
    /// The next (1) or previous (-1) model.
    NextModel(i8),
    /// Next receiver.
    Receiver,
    /// Send the draft with this policy.
    Send(SendPolicy),
    /// Resolved by the part of the page that declares the binding (its `press`).
    Part,
}

/// One binding of the catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyBinding {
    /// As `state/keys.rs::stroke` names it: `f9`, `alt+1`, `escape`, `alt+enter`.
    pub key: &'static str,
    /// The key as the legend writes it.
    pub cap: &'static str,
    pub label: &'static str,
    pub action: KeyAction,
    /// Bound in the web build; a key in [`WEB_RESERVED`] never is.
    pub web: bool,
}

impl KeyBinding {
    /// Whether it acts while the composer's field has the keys: a chord, a function key, Escape,
    /// or End (which also reaches the field, see [`KeyBinding::pass_through`]). A bare key is a
    /// view's: the app's key router takes it only while no field is edited.
    pub fn typing(&self) -> bool {
        let key = self.key;
        key.contains("alt+")
            || key.contains("ctrl+")
            || key.contains("mod+")
            || key == "escape"
            || (key.len() > 1
                && key.starts_with('f')
                && key[1..].bytes().all(|byte| byte.is_ascii_digit()))
            || self.action == KeyAction::Jump
    }

    /// Whether the key goes on to the focused field after the page acts: End still moves the
    /// caret.
    pub fn pass_through(&self) -> bool {
        self.action == KeyAction::Jump
    }
}

pub(super) const fn bind(
    key: &'static str,
    cap: &'static str,
    label: &'static str,
    action: KeyAction,
) -> KeyBinding {
    KeyBinding {
        key,
        cap,
        label,
        action,
        web: true,
    }
}

/// A part of the page with its own bindings.
struct Part {
    /// The tab whose view has these keys; none for the dock's and the Details panel's.
    tab: Option<Tab>,
    keys: fn() -> Vec<KeyBinding>,
    press: fn(&str, &Cx<'_>) -> Option<Action>,
}

/// Each part's bindings, in precedence order.
const PARTS: &[Part] = &[
    Part {
        tab: None,
        keys: dock::keys,
        press: dock::press,
    },
    Part {
        tab: None,
        keys: questions::keys,
        press: questions::press,
    },
    Part {
        tab: None,
        keys: sidebar::keys,
        press: sidebar::press,
    },
    Part {
        tab: Some(Tab::Chat),
        keys: usage::keys,
        press: usage::press,
    },
    Part {
        tab: Some(Tab::Changes),
        keys: changes::keys,
        press: changes::press,
    },
    Part {
        tab: Some(Tab::Board),
        keys: board::keys,
        press: board::press,
    },
    Part {
        tab: Some(Tab::Task),
        keys: task::keys,
        press: task::press,
    },
    Part {
        tab: Some(Tab::Proof),
        keys: proof::keys,
        press: proof::press,
    },
    Part {
        tab: Some(Tab::Agents),
        keys: fleet::keys,
        press: fleet::press,
    },
];

/// Every session binding.
pub fn keys() -> Vec<KeyBinding> {
    let mut keys = vec![
        bind("f5", "F5", "Task", KeyAction::Tab(Tab::Task)),
        bind("f9", "F9", "Chat", KeyAction::Tab(Tab::Chat)),
        bind("f8", "F8", "Agents", KeyAction::Tab(Tab::Agents)),
        bind("f7", "F7", "Changes", KeyAction::Tab(Tab::Changes)),
        bind("f10", "F10", "Board or Proof", KeyAction::Tab(Tab::Board)),
        bind("alt+1", "Alt+1", "first tab", KeyAction::TabAt(0)),
        bind("alt+2", "Alt+2", "second tab", KeyAction::TabAt(1)),
        bind("alt+3", "Alt+3", "third tab", KeyAction::TabAt(2)),
        bind("alt+4", "Alt+4", "fourth tab", KeyAction::TabAt(3)),
        bind("alt+5", "Alt+5", "fifth tab", KeyAction::TabAt(4)),
        bind("alt+6", "Alt+6", "sixth tab", KeyAction::TabAt(5)),
        bind("escape", "esc", "back to chat · stop", KeyAction::Escape),
        bind(
            "alt+h",
            "Alt+H",
            "show or hide thinking",
            KeyAction::Thinking,
        ),
        bind("ctrl+o", "Ctrl+O", "expand all", KeyAction::ExpandAll),
        bind("end", "End", "latest", KeyAction::Jump),
        bind("ctrl+end", "Ctrl+End", "latest", KeyAction::Jump),
        bind("mod+shift+m", "Ctrl+Shift+M", "model", KeyAction::Model),
        bind("alt+e", "Alt+E", "effort", KeyAction::Effort),
        bind("alt+p", "Alt+P", "next model", KeyAction::NextModel(1)),
        bind(
            "alt+shift+p",
            "Alt+Shift+P",
            "previous model",
            KeyAction::NextModel(-1),
        ),
        bind("alt+t", "Alt+T", "next receiver", KeyAction::Receiver),
        bind(
            "enter",
            "⏎",
            "send or steer",
            KeyAction::Send(SendPolicy::Steer),
        ),
        bind(
            "alt+enter",
            "Alt+⏎",
            "queue a follow-up",
            KeyAction::Send(SendPolicy::FollowUp),
        ),
    ];
    for part in PARTS {
        keys.extend((part.keys)());
    }
    keys
}

/// `key` as the browser sees it: `mod` is Ctrl off Apple keyboards, and the reserved list names
/// it so.
fn reserved_on_web(key: &str) -> bool {
    let key = key.replace("mod+", "ctrl+");
    WEB_RESERVED.contains(&key.as_str())
}

/// The bindings a form takes: the web build leaves the browser's keys alone; the phone has none.
pub fn bound(form: Form, web: bool) -> Vec<KeyBinding> {
    if form == Form::Phone {
        return Vec::new();
    }
    keys()
        .into_iter()
        .filter(|binding| !web || (binding.web && !reserved_on_web(binding.key)))
        .collect()
}

/// Whether the `/` menu could be open: the composer has the keys on Chat and its draft is a one
/// line `/` command the owner has not closed the menu on.
fn slash_typed(cx: &Cx<'_>) -> bool {
    let draft = &cx.ui.draft;
    cx.ui.focus == Focus::Composer
        && cx.ui.tab == Tab::Chat
        && draft.starts_with('/')
        && !draft.contains('\n')
        && cx.ui.picker.slash_closed.as_deref() != Some(draft.as_str())
}

/// Whether the page has a live session with a composer: not a child's transcript, not a log.
fn talks(cx: &Cx<'_>) -> bool {
    *cx.role != SessionRole::Child && !cx.log_only()
}

/// What a pressed key does on the page `cx` reads. Enter and Alt+Enter are the composer's own:
/// the composer sends the field's text through `Action::Send`, so they are not resolved here.
/// Escape first closes an open picker or `/` menu, then what the dock holds (a confirmation,
/// reason or answer mode, an open card).
pub fn press(key: &str, cx: &Cx<'_>, web: bool) -> Option<Action> {
    let binding = bound(cx.env.form, web)
        .into_iter()
        .find(|binding| binding.key == key)?;
    let tabs = super::shown_tabs(cx);
    let tabs = tabs.as_slice();
    let session = cx.id.clone();
    match binding.action {
        KeyAction::TabAt(at) => tabs.get(at).map(|tab| Action::Tab { session, tab: *tab }),
        KeyAction::Tab(Tab::Board) => tabs
            .iter()
            .find(|tab| matches!(tab, Tab::Board | Tab::Proof))
            .map(|tab| Action::Tab { session, tab: *tab }),
        KeyAction::Tab(tab) => tabs.contains(&tab).then_some(Action::Tab { session, tab }),
        KeyAction::Escape if cx.ui.picker.open.is_some() => {
            Some(Action::Picker(PickerAction::Close(session)))
        }
        KeyAction::Escape if slash_typed(cx) && super::slash(cx, &super::actions(cx)).is_some() => {
            Some(Action::Command(CommandAction::CloseMenu(session)))
        }
        KeyAction::Escape => dock::escape(cx).or_else(|| {
            if cx.ui.tab != Tab::Chat || cx.ui.focus != Focus::Composer {
                Some(Action::Tab {
                    session,
                    tab: Tab::Chat,
                })
            } else {
                (super::composer::working(cx) && cx.controls().contains(&Control::Cancel))
                    .then_some(Action::Control {
                        session,
                        control: Control::Cancel,
                    })
            }
        }),
        KeyAction::Thinking => Some(Action::Thinking(session)),
        KeyAction::ExpandAll => (!cx.log_only()).then(|| Action::Expand {
            session,
            key: EXPAND_ALL.into(),
            fetch: None,
        }),
        KeyAction::Jump => {
            let draft_empty = cx.ui.draft.trim().is_empty();
            (cx.ui.tab == Tab::Chat && !cx.log_only() && (key != "end" || draft_empty))
                .then_some(Action::JumpEnd(session))
        }
        // While an agent's conversation is open the model keys are that agent's: the model key
        // opens its model choices, and its effort is not changed from here.
        KeyAction::Model | KeyAction::Effort | KeyAction::NextModel(_)
            if talks(cx) && super::fleet::opened_agent(cx).is_some() =>
        {
            let model = super::fleet::opened_agent(cx)?.model;
            (matches!(binding.action, KeyAction::Model) && model.unavailable.is_none())
                .then_some(model.opens)
                .flatten()
        }
        KeyAction::Model => talks(cx).then(|| {
            if cx.ui.picker.open == Some(PickerKind::Model) {
                Action::Picker(PickerAction::Close(session))
            } else {
                Action::Picker(PickerAction::Open {
                    session,
                    kind: PickerKind::Model,
                })
            }
        }),
        KeyAction::Effort => talks(cx).then(|| Action::Picker(PickerAction::CycleEffort(session))),
        KeyAction::NextModel(delta) => {
            talks(cx).then(|| Action::Picker(PickerAction::NextModel { session, delta }))
        }
        KeyAction::Receiver => Some(Action::CycleReceiver(session)),
        KeyAction::Send(_) => None,
        KeyAction::Part => PARTS.iter().find_map(|part| (part.press)(key, cx)),
    }
}

/// The legend of the composer and of the cards, in the terminal UI's priority: a view leads
/// with the way back; the composer while working leads with steer, stop and queue, and Ctrl+O
/// comes last among the composer's own so it is the first to give way.
pub fn legend(focus: Focus, tab: Tab, working: bool) -> Vec<(&'static str, &'static str)> {
    match focus {
        Focus::Queue => return vec![("Alt+↓", "next"), ("esc", "chat")],
        Focus::Questions => return vec![("Alt+A", "answer"), ("Alt+↓", "next"), ("esc", "chat")],
        Focus::Dock => {
            return vec![
                ("1-9", "pick"),
                ("⏎", "choose"),
                ("Alt+↓", "next"),
                ("esc", "not now"),
            ];
        }
        Focus::Composer | Focus::Main => {}
    }
    let mut legend = Vec::new();
    if focus != Focus::Composer || tab != Tab::Chat {
        legend.push(("esc", "chat"));
        return legend;
    }
    if working {
        legend.extend([("⏎", "steer"), ("esc", "stop"), ("Alt+⏎", "queue")]);
    } else {
        legend.extend([("⏎", "send"), ("/", "commands")]);
    }
    legend.push(("Alt+H", "thinking"));
    // Details' key while the line has room for it; a working agent's three keys come first.
    if !working {
        legend.push(("Alt+B", "details"));
    }
    legend.extend([("Ctrl+K", "actions"), ("Ctrl+O", "expand")]);
    legend
}

/// The bindings of the part whose view has the keys on `tab`, in the part's order and one per
/// action: a second key for the same action (↓ beside `j`) only takes the line's room from the
/// next action.
fn part_legend(tab: Tab) -> Vec<(&'static str, &'static str)> {
    let mut legend: Vec<(&'static str, &'static str)> = Vec::new();
    for binding in PARTS
        .iter()
        .filter(|part| part.tab == Some(tab) && tab != Tab::Chat)
        .flat_map(|part| (part.keys)())
    {
        if !binding.label.is_empty() && !legend.iter().any(|(_, label)| *label == binding.label) {
            legend.push((binding.cap, binding.label));
        }
    }
    legend
}

/// The legend for what has the keys on the page `cx` reads, showing `tab`: an open picker, the
/// `/` menu (`slash`: it shows), the transcript, a view or the sidebar with its own keys, a
/// card, or the composer.
pub fn legend_of(cx: &Cx<'_>, tab: Tab, slash: bool) -> Vec<(&'static str, &'static str)> {
    if let Some(kind) = cx.ui.picker.open {
        return vec![
            ("↑ ↓", "move"),
            (
                "⏎",
                match kind {
                    PickerKind::Model => "use model",
                    PickerKind::Effort => "use effort",
                },
            ),
            ("esc", "close"),
        ];
    }
    let focus = cx.ui.focus;
    if slash && focus == Focus::Composer && tab == Tab::Chat {
        return vec![("↑ ↓", "move"), ("⏎", "run"), ("esc", "keep text")];
    }
    match focus {
        Focus::Main if tab == Tab::Chat => vec![
            ("PgUp PgDn", "scroll"),
            ("End", "latest"),
            ("Ctrl+O", "expand"),
            ("Alt+H", "thinking"),
            ("esc", "composer"),
        ],
        Focus::Main => {
            let mut legend = vec![("esc", "chat")];
            legend.extend(part_legend(tab));
            legend
        }
        _ => legend(focus, tab, super::composer::working(cx)),
    }
}
