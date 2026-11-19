//! The session page's keys: one catalog of bindings (`keys()`), each naming what it does and the
//! forms that bind it, the reducer that turns a pressed key into the page's action (`press`), and
//! the legend the terminal's key line and the composer's hint show for the focused surface.
//! Every part of the page adds its own bindings through its `keys()` and resolves them in its
//! `press`. The web build leaves F1, F5 and F11 to the browser; Tab and Shift+Tab keep walking
//! the focus.

use corral_protocol::session_state::SendPolicy;

use crate::model::agents::Control;
use crate::model::nav::Form;

use super::{Action, Cx, Focus, Tab, board, changes, dock, proof, questions, sidebar, task, usage};

/// What a binding does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    /// Open the tab at this position (Alt+1…Alt+6).
    TabAt(usize),
    /// Open this tab by its terminal UI key.
    Tab(Tab),
    /// Back to Chat and the composer from a view; stop the turn from the composer.
    Escape,
    /// Show or hide every thinking block.
    Thinking,
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
    /// Bound in the web build; the browser keeps F1, F5 and F11.
    pub web: bool,
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
    keys: fn() -> Vec<KeyBinding>,
    press: fn(&str, &Cx<'_>) -> Option<Action>,
}

/// Each part's bindings, in precedence order.
const PARTS: &[Part] = &[
    Part {
        keys: dock::keys,
        press: dock::press,
    },
    Part {
        keys: questions::keys,
        press: questions::press,
    },
    Part {
        keys: sidebar::keys,
        press: sidebar::press,
    },
    Part {
        keys: usage::keys,
        press: usage::press,
    },
    Part {
        keys: changes::keys,
        press: changes::press,
    },
    Part {
        keys: board::keys,
        press: board::press,
    },
    Part {
        keys: task::keys,
        press: task::press,
    },
    Part {
        keys: proof::keys,
        press: proof::press,
    },
];

/// Every session binding.
pub fn keys() -> Vec<KeyBinding> {
    let mut keys = vec![
        KeyBinding {
            web: false,
            ..bind("f5", "F5", "Task", KeyAction::Tab(Tab::Task))
        },
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

/// The bindings a form takes: the web build leaves the browser's keys alone; the phone has none.
pub fn bound(form: Form, web: bool) -> Vec<KeyBinding> {
    if form == Form::Phone {
        return Vec::new();
    }
    keys()
        .into_iter()
        .filter(|binding| !web || binding.web)
        .collect()
}

/// What a pressed key does on the page `cx` reads. Enter and Alt+Enter are the composer's own:
/// the composer sends the field's text through `Action::Send`, so they are not resolved here.
/// Escape first closes what the dock holds (a confirmation, reason or answer mode, an open card).
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
        KeyAction::Receiver => Some(Action::CycleReceiver(session)),
        KeyAction::Send(_) => None,
        KeyAction::Part => PARTS.iter().find_map(|part| (part.press)(key, cx)),
    }
}

/// The legend for the focused surface, in the terminal UI's priority: a view leads with the way
/// back; the composer while working leads with steer, stop and queue.
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
        Focus::Composer | Focus::Main | Focus::Sidebar => {}
    }
    let mut legend = Vec::new();
    if focus != Focus::Composer || tab != Tab::Chat {
        legend.push(("esc", "chat"));
        return legend;
    }
    if working {
        legend.extend([("⏎", "steer"), ("esc", "stop"), ("Alt+⏎", "queue")]);
    } else {
        legend.push(("⏎", "send"));
    }
    legend.extend([("Alt+H", "thinking"), ("Ctrl+K", "actions")]);
    legend
}
