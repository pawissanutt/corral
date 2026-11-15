//! The keys bar under a pane's screen, for a session corrald types into (`controls.keys`: a pane
//! it owns, no host bridge). The owner gathers named keys into one sequence, adds text if any,
//! and sends both as one `session keys` command: the keys are pressed first, then the text is
//! typed, never submitted. The command is Admin-class, so the root previews it and asks this
//! device to confirm, which is why keys are gathered rather than sent one press at a time. One
//! command is in flight per session; presses meanwhile are dropped, never queued, so keys cannot
//! arrive out of order. Once it settles the pane's screen is read again at once.

use corral_protocol::cmd::Command;
use corral_protocol::cmd::session::SessionCommand;
use corral_protocol::detail::DetailRef;
use corral_protocol::rows::AgentRow;

use crate::model::daemon::{self, Link};
use crate::model::effect::{Effect, Env};
use crate::model::hue::Hue;
use crate::model::nav::Form;
use crate::model::{Act, ActStatus, Id, Live, act};

use super::controls::{NO_SESSION, OFFLINE, in_flight, refusal_words};
use super::live::shown_id;
use super::{Action, State, reread};

/// corrald's bounds on one command.
const MAX_KEYS: usize = 32;
const MAX_TEXT: usize = 4 * 1024;

/// The keys on the bar: the control keys, then the cursor keys and Enter.
const CONTROL: [&str; 4] = ["esc", "tab", "ctrl-c", "ctrl-d"];
const CURSOR: [&str; 6] = ["left", "up", "down", "right", "backspace", "enter"];
/// The rest of corrald's named keys, offered in the chooser.
const NAVIGATION: [&str; 7] = [
    "pageup", "pagedown", "home", "end", "insert", "delete", "space",
];
const FUNCTION: [&str; 12] = [
    "f1", "f2", "f3", "f4", "f5", "f6", "f7", "f8", "f9", "f10", "f11", "f12",
];
const CTRL: [&str; 26] = [
    "ctrl-a", "ctrl-b", "ctrl-c", "ctrl-d", "ctrl-e", "ctrl-f", "ctrl-g", "ctrl-h", "ctrl-i",
    "ctrl-j", "ctrl-k", "ctrl-l", "ctrl-m", "ctrl-n", "ctrl-o", "ctrl-p", "ctrl-q", "ctrl-r",
    "ctrl-s", "ctrl-t", "ctrl-u", "ctrl-v", "ctrl-w", "ctrl-x", "ctrl-y", "ctrl-z",
];

/// Refusals that need no words under the screen: a bridged session is driven through its chat,
/// and an ended one takes nothing.
const SILENT: [&str; 2] = ["has_bridge", "ended"];

const ADMIN: &str =
    "Refused: this device may not type into panes; that needs admin authority. Nothing was sent.";

/// A press on the keys bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyPress {
    /// Add a named key to the end of the sequence; from the chooser, also close it.
    Add(&'static str),
    RemoveLast,
    Clear,
    /// Open or close the chooser of the other keys.
    Choose(bool),
    /// Send the sequence, then these words when they are not blank, as one command.
    Send(String),
    /// The latest command settled: read the screen again, and empty the sequence if it ran.
    Settled(Id),
}

/// One key button: it adds its key to the sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyButton {
    pub label: String,
    pub test_id: String,
    pub action: Action,
}

/// The chooser of the keys the bar does not show, by kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chooser {
    pub title: &'static str,
    pub sub: &'static str,
    pub groups: Vec<(&'static str, Vec<KeyButton>)>,
    pub close: Action,
}

/// A command that settled: the screen reads the pane again once, and clears the field once
/// when the text it holds is the text that was typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settled {
    pub key: Id,
    pub typed: Option<String>,
}

/// The bar, while corrald types into this session's pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyBar {
    /// The control keys, then the cursor keys and Enter.
    pub groups: [Vec<KeyButton>; 2],
    /// Opens the chooser of the other keys.
    pub more: KeyButton,
    pub chooser: Option<Chooser>,
    pub sequence_label: &'static str,
    /// The gathered keys in order, as their caps read.
    pub sequence: Vec<String>,
    pub empty: &'static str,
    /// The sequence as a screen reader says it.
    pub spoken: String,
    /// Remove last and Clear; no action while there is nothing to remove or the bar takes no
    /// presses.
    pub remove: (&'static str, Option<Action>),
    pub clear: (&'static str, Option<Action>),
    pub placeholder: &'static str,
    pub field_label: &'static str,
    pub send: &'static str,
    /// That the keys go first and the text is typed without Enter.
    pub hint: &'static str,
    /// False while this device cannot send or a command is in flight; presses then do nothing.
    pub enabled: bool,
    /// Why this device sends nothing, or why the latest press was not taken.
    pub note: Option<String>,
    pub outcome: Option<(Hue, String)>,
    /// "Check result" for a command whose outcome is unknown.
    pub recover: Option<Action>,
    pub settled: Option<Settled>,
}

/// What the terminal shows for keys: the bar, or one quiet line saying why there is none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Keys {
    Open(KeyBar),
    Closed(String),
}

/// A session's gathered keys and its latest keys command.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Latest {
    pending: Vec<&'static str>,
    sent: Option<Typed>,
    held: Option<String>,
    choosing: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Typed {
    key: Id,
    /// "Esc ↑ Enter, then 12 bytes of text".
    what: String,
    text: Option<String>,
}

fn allowed(name: &str) -> bool {
    [
        &CONTROL[..],
        &CURSOR[..],
        &NAVIGATION[..],
        &FUNCTION[..],
        &CTRL[..],
    ]
    .iter()
    .any(|set| set.contains(&name))
}

/// A key as its cap reads; the phone sets Backspace and Enter as their symbols to fit a row.
fn label(name: &str, form: Form) -> String {
    let phone = form == Form::Phone;
    let fixed = match name {
        "esc" => "Esc",
        "tab" => "Tab",
        "enter" if phone => "⏎",
        "enter" => "Enter",
        "backspace" if phone => "⌫",
        "backspace" => "Backspace",
        "left" => "←",
        "up" => "↑",
        "down" => "↓",
        "right" => "→",
        "pageup" => "Page Up",
        "pagedown" => "Page Down",
        "home" => "Home",
        "end" => "End",
        "insert" => "Insert",
        "delete" => "Delete",
        "space" => "Space",
        _ => {
            return match name.strip_prefix("ctrl-") {
                Some(letter) => format!("Ctrl-{}", letter.to_uppercase()),
                None => name.to_uppercase(),
            };
        }
    };
    fixed.into()
}

fn button(name: &'static str, form: Form) -> KeyButton {
    KeyButton {
        label: label(name, form),
        test_id: format!("btn-key-{name}"),
        action: Action::Keys(KeyPress::Add(name)),
    }
}

fn buttons(names: &[&'static str], form: Form) -> Vec<KeyButton> {
    names.iter().map(|&name| button(name, form)).collect()
}

/// "Esc ↑ Enter", "12 bytes of text", or both joined by "then".
fn describe(keys: &[&str], text: Option<&str>) -> String {
    let named = keys
        .iter()
        .map(|name| label(name, Form::Desktop))
        .collect::<Vec<_>>()
        .join(" ");
    let text = text.map(|text| match text.len() {
        1 => "1 byte of text".to_owned(),
        bytes => format!("{bytes} bytes of text"),
    });
    match text {
        Some(text) if named.is_empty() => text,
        Some(text) => format!("{named}, then {text}"),
        None => named,
    }
}

/// Why this device sends no keys now: no live link, no app session yet, or no admin authority.
fn device(feed: &daemon::Feed, env: Env) -> Option<String> {
    if env.conn.refuses() || feed.link != Link::Up {
        return Some(OFFLINE.into());
    }
    let Some(session) = &feed.session else {
        return Some(NO_SESSION.into());
    };
    (!session.authority.admin).then(|| ADMIN.into())
}

/// The latest command and where it stands in the command journal.
fn status<'a>(latest: &'a Latest, feed: &daemon::Feed) -> Option<(&'a Typed, ActStatus)> {
    let typed = latest.sent.as_ref()?;
    act::status(feed, &typed.key).map(|status| (typed, status))
}

fn busy(latest: Option<&Latest>, feed: &daemon::Feed) -> bool {
    latest
        .and_then(|latest| status(latest, feed))
        .is_some_and(|(_, status)| in_flight(&status))
}

fn outcome(typed: &Typed, status: &ActStatus) -> (Hue, String) {
    let what = &typed.what;
    match status {
        ActStatus::Done if typed.text.is_some() => (
            Hue::Ok,
            format!("Sent {what} · the text was not submitted; send Enter to submit it"),
        ),
        ActStatus::Done => (Hue::Ok, format!("Sent {what}")),
        ActStatus::Failed(_) | ActStatus::Refused(_) | ActStatus::Conflict(_) => {
            (Hue::Fail, format!("{what} · {}", status.line()))
        }
        ActStatus::Unknown | ActStatus::Abandoned => {
            (Hue::Held, format!("{what} · {}", status.line()))
        }
        _ => (Hue::Neutral, format!("Sending {what} · {}", status.line())),
    }
}

/// The keys part of session `id`'s terminal: the bar while corrald offers keys, a quiet line for
/// a refusal worth saying, nothing otherwise.
pub(super) fn keys(
    state: &State,
    id: &Id,
    row: &AgentRow,
    feed: &daemon::Feed,
    env: Env,
) -> Option<Keys> {
    let offered = &row.controls.keys;
    if !offered.available {
        let code = offered.refusal.as_deref()?;
        return (!SILENT.contains(&code)).then(|| {
            Keys::Closed(format!(
                "Keys are not sent from here: {}.",
                refusal_words(code)
            ))
        });
    }
    let form = env.form;
    let latest = state.keys.get(id.as_str());
    let device = device(feed, env);
    let shown = latest.and_then(|latest| status(latest, feed));
    let busy = shown.as_ref().is_some_and(|(_, status)| in_flight(status));
    let enabled = device.is_none() && !busy;
    let pending = latest.map_or(&[][..], |latest| latest.pending.as_slice());
    let editable = enabled && !pending.is_empty();
    let chooser = latest
        .filter(|latest| latest.choosing && enabled)
        .map(|_| Chooser {
            title: "More keys",
            sub: "Adds one key to the end of the keys to send.",
            groups: vec![
                ("Navigation", buttons(&NAVIGATION, form)),
                ("Function", buttons(&FUNCTION, form)),
                (
                    "Control",
                    buttons(
                        &CTRL
                            .into_iter()
                            .filter(|name| !CONTROL.contains(name))
                            .collect::<Vec<_>>(),
                        form,
                    ),
                ),
            ],
            close: Action::Keys(KeyPress::Choose(false)),
        });
    Some(Keys::Open(KeyBar {
        groups: [buttons(&CONTROL, form), buttons(&CURSOR, form)],
        more: KeyButton {
            label: if form == Form::Phone {
                "More".into()
            } else {
                "More keys…".into()
            },
            test_id: "btn-keys-more".into(),
            action: Action::Keys(KeyPress::Choose(true)),
        },
        chooser,
        sequence_label: "Keys",
        sequence: pending.iter().map(|name| label(name, form)).collect(),
        empty: "none added",
        spoken: if pending.is_empty() {
            "No keys added".into()
        } else {
            format!("Keys to send: {}", describe(pending, None))
        },
        remove: (
            "Remove last",
            editable.then(|| Action::Keys(KeyPress::RemoveLast)),
        ),
        clear: ("Clear", editable.then(|| Action::Keys(KeyPress::Clear))),
        placeholder: if form == Form::Phone {
            "Text to type"
        } else {
            "Text to type into the pane"
        },
        field_label: "Text to type into the pane",
        send: if busy { "Sending…" } else { "Send" },
        hint: match form {
            Form::Desktop => {
                "The keys are pressed first, then the text is typed. The text never ends with \
                 Enter: send Enter on its own afterwards to submit it."
            }
            Form::Phone => "Keys first, then the text. The text gets no Enter; send ⏎ after it.",
            Form::Terminal => {
                "keys first, then the text · the text gets no Enter: send Enter after it to submit"
            }
        },
        enabled,
        note: device.or_else(|| latest.and_then(|latest| latest.held.clone())),
        outcome: shown.as_ref().map(|(typed, status)| outcome(typed, status)),
        recover: shown
            .as_ref()
            .filter(|(_, status)| *status == ActStatus::Unknown)
            .map(|(typed, _)| Action::Recover(typed.key.clone())),
        settled: shown
            .as_ref()
            .filter(|(_, status)| status.settled() || *status == ActStatus::Unknown)
            .map(|(typed, status)| Settled {
                key: typed.key.clone(),
                typed: typed.text.clone().filter(|_| *status == ActStatus::Done),
            }),
    }))
}

impl State {
    /// Gather one sequence per session; only the shared command path may send it.
    pub(super) fn press_keys(
        &mut self,
        session: Id,
        press: KeyPress,
        feed: &daemon::Feed,
        env: Env,
    ) -> Vec<Effect> {
        match press {
            KeyPress::Settled(key) => return self.keys_settled(session, &key, feed),
            KeyPress::Choose(false) => {
                if let Some(latest) = self.keys.get_mut(&session) {
                    latest.choosing = false;
                }
                return Vec::new();
            }
            _ => {}
        }
        let Some(row) = feed
            .agents
            .get(session.as_str())
            .filter(|row| row.controls.keys.available)
        else {
            return Vec::new();
        };
        if device(feed, env).is_some() || busy(self.keys.get(session.as_str()), feed) {
            return Vec::new();
        }
        let latest = self.keys.entry(session.clone()).or_default();
        latest.held = None;
        match press {
            KeyPress::Add(name) => {
                latest.choosing = false;
                if !allowed(name) {
                    latest.held = Some(format!(
                        "{name} is not a key corrald types; nothing was added."
                    ));
                } else if latest.pending.len() >= MAX_KEYS {
                    latest.held = Some(format!(
                        "At most {MAX_KEYS} keys go in one command; send these first."
                    ));
                } else {
                    latest.pending.push(name);
                }
                Vec::new()
            }
            KeyPress::RemoveLast => {
                latest.pending.pop();
                Vec::new()
            }
            KeyPress::Clear => {
                latest.pending.clear();
                Vec::new()
            }
            KeyPress::Choose(open) => {
                latest.choosing = open;
                Vec::new()
            }
            KeyPress::Send(text) => send(&mut self.ids, latest, &session, row, text),
            KeyPress::Settled(_) => Vec::new(),
        }
    }

    fn keys_settled(&mut self, session: Id, key: &Id, feed: &daemon::Feed) -> Vec<Effect> {
        if let Some(latest) = self.keys.get_mut(&session)
            && latest.sent.as_ref().is_some_and(|typed| typed.key == *key)
            && act::status(feed, key) == Some(ActStatus::Done)
        {
            latest.pending.clear();
        }
        vec![reread(
            feed,
            DetailRef::Terminal {
                id: session.to_string(),
            },
        )]
    }
}

fn send(
    ids: &mut crate::model::id::IdSupply,
    latest: &mut Latest,
    session: &Id,
    row: &AgentRow,
    text: String,
) -> Vec<Effect> {
    let text = (!text.trim().is_empty()).then_some(text);
    if latest.pending.is_empty() && text.is_none() {
        latest.held = Some("Nothing to send: add a key or type some text.".into());
        return Vec::new();
    }
    if let Some(text) = &text
        && text.len() > MAX_TEXT
    {
        latest.held = Some(format!(
            "The text is {} bytes; corrald types at most {MAX_TEXT} at once. Nothing was sent.",
            text.len()
        ));
        return Vec::new();
    }
    let what = describe(&latest.pending, text.as_deref());
    let act = Act::new(
        ids,
        format!("Send {what} to {}", shown_id(row)),
        Command::Session(SessionCommand::Keys {
            id: row.id.clone(),
            keys: latest
                .pending
                .iter()
                .map(|name| (*name).to_owned())
                .collect(),
            text: text.clone(),
            json: false,
        }),
    )
    .about(Live::Session(session.clone()));
    latest.sent = Some(Typed {
        key: act.key.clone(),
        what,
        text,
    });
    vec![Effect::Act(act)]
}

#[cfg(test)]
#[path = "keys_tests.rs"]
mod tests;
