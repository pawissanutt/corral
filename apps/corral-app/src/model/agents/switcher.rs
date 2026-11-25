//! The Agents switcher: the assistant's conversations (its chats and the build sessions), the
//! ones pinned on this device first in the order they were pinned, then the rest by their newest
//! activity; the conversation last opened here is selected, so Enter opens it again. New
//! assistant is an entry of its own: it starts a new chat and keys Ask to it, and the chat Ask
//! was on stays listed and readable; nothing is ended. Pins and the last opened conversation
//! are kept per authenticated enrollment and daemon store (`corral.agents.v2`), each by its exact `A-n` or `B-…`: never `quick`,
//! which moves to the next chat once the current one ends.

use std::collections::BTreeMap;

use corral_protocol::rows::{AssistantKind, AssistantRow, AssistantState};
use serde::{Deserialize, Serialize};

use super::{live, session_line};
use crate::model::daemon::Feed;
use crate::model::{Id, assistant};

/// The switcher's chord on every form, as the trigger and the palette name it.
pub const AGENTS_KEY: &str = "Ctrl G";
/// The same chord as a terminal's key line writes it.
pub const AGENTS_CHORD: &str = "^G";
pub const AGENTS_LABEL: &str = "Agents";
pub const NEW_ASSISTANT: &str = "New assistant";
pub const NEW_ASSISTANT_NOTE: &str = "a new chat for Ask; the one it was on stays here";
pub const SWITCHER_SUB: &str = "Pinned on this device, then recent. Enter opens; P pins or unpins.";
/// The same where the side pane is: S opens the focused conversation there.
pub const SWITCHER_SUB_PANE: &str =
    "Pinned on this device, then recent. Enter opens; S opens in the side pane; P pins or unpins.";
pub const SWITCHER_EMPTY: &str = "No assistant conversations yet.";
pub const PINNED: &str = "Pinned";
pub const RECENT_HEADING: &str = "Recent";
/// Pins kept per store, so the saved preference stays small.
const MOST_PINS: usize = 24;

/// One daemon store's pins and the conversation last opened from this device.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct StoreAgents {
    #[serde(default)]
    pub pinned: Vec<String>,
    #[serde(default)]
    pub last: Option<String>,
    #[serde(default)]
    pub docked: Vec<String>,
    #[serde(default)]
    pub visible_dock: Option<String>,
    /// The side pane's session tabs are kept but not shown; showing a session there clears it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pane_hidden: bool,
}

/// Pins keyed by an unambiguous authenticated enrollment and store pair.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Pins(BTreeMap<String, StoreAgents>);

impl Pins {
    pub fn parse(value: &str) -> Option<Self> {
        serde_json::from_str(value).ok()
    }
    pub fn encode(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
    pub fn get(&self, store: &str) -> Option<&StoreAgents> {
        self.0.get(store)
    }
    pub(crate) fn store_mut(&mut self, store: &str) -> &mut StoreAgents {
        self.0.entry(store.to_owned()).or_default()
    }
    pub fn pinned(&self, store: &str, id: &str) -> bool {
        self.get(store)
            .is_some_and(|saved| saved.pinned.iter().any(|pin| pin == id))
    }
    /// Pins `id` at the end of the store's pins, or unpins it; whether it is pinned now. Past
    /// the bound the oldest pin gives way.
    pub fn toggle(&mut self, store: &str, id: &str) -> bool {
        let saved = self.0.entry(store.to_owned()).or_default();
        if let Some(at) = saved.pinned.iter().position(|pin| pin == id) {
            saved.pinned.remove(at);
            return false;
        }
        saved.pinned.push(id.to_owned());
        if saved.pinned.len() > MOST_PINS {
            saved.pinned.remove(0);
        }
        true
    }
    /// Remembers `id` as the conversation last opened; whether that changed anything.
    pub fn used(&mut self, store: &str, id: &str) -> bool {
        let saved = self.0.entry(store.to_owned()).or_default();
        if saved.last.as_deref() == Some(id) {
            return false;
        }
        saved.last = Some(id.to_owned());
        true
    }
}

/// The exact conversation `id` names: `quick` as the chat it resolves to now, an `A-n` or a
/// `B-…` the daemon lists as itself; none for anything else (a task's run, `quick` with no
/// current chat).
pub fn exact(feed: &Feed, id: &str) -> Option<String> {
    if id == assistant::QUICK {
        return assistant::quick_id(feed);
    }
    feed.assistants.contains_key(id).then(|| id.to_owned())
}

/// One conversation in the switcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub id: Id,
    /// The chat's title or the build's name; its id when the host named neither.
    pub title: String,
    /// `chat · Idle`, `build · Working`, `chat · Gone · ended`.
    pub note: String,
    pub pinned: bool,
    /// The chat Ask sends to now.
    pub ask: bool,
    /// A chat or a build: what keys Ask when chosen (a chat that has not ended) and what only
    /// opens its page.
    pub keys_ask: bool,
}

/// The switcher's list.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Switcher {
    pub pinned: Vec<Entry>,
    pub recent: Vec<Entry>,
    /// The entry focused as it opens: the conversation last opened here, else the first.
    pub selected: Option<Id>,
}

impl Switcher {
    /// Every entry in the order the switcher lists them.
    pub fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.pinned.iter().chain(&self.recent)
    }
}

/// Whether choosing the conversation keys Ask to it: a chat that has not ended. A build only
/// opens its page.
pub fn keys_ask(row: &AssistantRow) -> bool {
    matches!(row.kind, AssistantKind::Quick | AssistantKind::Session)
        && row.state != AssistantState::Ended
}

fn entry(feed: &Feed, row: &AssistantRow, pinned: bool, ask: Option<&str>) -> Entry {
    let line = session_line(feed, &row.id);
    let title = line
        .as_ref()
        .map(|line| line.name.clone())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| row.id.clone());
    let role = if super::is_code(row) {
        "code"
    } else if row.kind == AssistantKind::Build {
        "build"
    } else {
        "chat"
    };
    let ended = row.state == AssistantState::Ended;
    let ask = ask == Some(row.id.as_str());
    let note = [Some(role), line.map(|line| line.state.word())]
        .into_iter()
        .flatten()
        .chain(ended.then_some("ended"))
        .chain(ask.then_some("Ask's chat"))
        .collect::<Vec<_>>()
        .join(" · ");
    Entry {
        id: Id::from(row.id.as_str()),
        title,
        note,
        pinned,
        ask,
        keys_ask: keys_ask(row),
    }
}

/// Unix seconds of the conversation's newest activity, as the host reports it.
fn activity(row: &AssistantRow) -> u64 {
    row.updated
        .as_deref()
        .or(row.created.as_deref())
        .and_then(live::unix)
        .unwrap_or(0)
}

fn number(id: &str) -> u64 {
    id.rsplit('-')
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

/// The switcher for the store's saved pins: the pinned conversations the daemon lists, in pin
/// order (a pin it does not list now is kept, not shown), then all others newest first. `ask` is the session Ask is keyed to (`quick` or a chat).
pub fn switcher(feed: &Feed, saved: Option<&StoreAgents>, ask: &str) -> Switcher {
    let ask = exact(feed, ask);
    let pins: &[String] = saved.map_or(&[], |saved| &saved.pinned);
    let pinned: Vec<Entry> = pins
        .iter()
        .filter_map(|id| feed.assistants.get(id.as_str()))
        .map(|row| entry(feed, row, true, ask.as_deref()))
        .collect();
    let mut others: Vec<&AssistantRow> = feed
        .assistants
        .values()
        .filter(|row| !pins.contains(&row.id))
        .collect();
    others.sort_by(|a, b| {
        activity(b)
            .cmp(&activity(a))
            .then_with(|| number(&b.id).cmp(&number(&a.id)))
            .then_with(|| a.id.cmp(&b.id))
    });
    let recent: Vec<Entry> = others
        .into_iter()
        .map(|row| entry(feed, row, false, ask.as_deref()))
        .collect();
    let mut shown = Switcher {
        pinned,
        recent,
        selected: None,
    };
    let last = saved.and_then(|saved| saved.last.as_deref());
    let selected = shown
        .entries()
        .find(|entry| Some(entry.id.as_str()) == last)
        .or_else(|| shown.entries().next())
        .map(|entry| entry.id.clone());
    shown.selected = selected;
    shown
}

#[cfg(test)]
mod tests {
    use super::super::source_tests::assistant as row;
    use super::*;

    fn feed(rows: &[(&str, AssistantKind, &str)]) -> Feed {
        let mut feed = Feed::default();
        for (id, kind, updated) in rows {
            let mut chat = row(id, *kind);
            chat.updated = Some((*updated).into());
            feed.assistants.insert((*id).into(), chat);
        }
        feed
    }

    /// A pin saved as `quick` would follow Ask to the next chat once the pinned one ended, so a
    /// pin names the chat `quick` resolves to; pins lead in pin order, the rest newest first,
    /// and the conversation last opened here is the one Enter opens.
    #[test]
    fn pins_lead_by_exact_id_and_the_last_opened_is_selected() {
        let feed = feed(&[
            ("A-1", AssistantKind::Quick, "2026-11-20T10:00:00Z"),
            ("A-2", AssistantKind::Session, "2026-11-21T09:00:00Z"),
            ("B-k3x9", AssistantKind::Build, "2026-11-21T10:00:00Z"),
        ]);
        let mut pins = Pins::default();
        let quick = exact(&feed, assistant::QUICK).unwrap_or_default();
        assert_eq!(quick, "A-1");
        assert!(pins.toggle("/s", &quick));
        assert!(pins.used("/s", "A-2"));
        assert!(!pins.used("/s", "A-2"));
        let shown = switcher(&feed, pins.get("/s"), assistant::QUICK);
        let order: Vec<_> = shown.entries().map(|entry| entry.id.to_string()).collect();
        assert_eq!(order, ["A-1", "B-k3x9", "A-2"]);
        assert_eq!(shown.selected.as_ref().map(Id::as_str), Some("A-2"));
        assert!(shown.pinned[0].ask && !shown.recent[0].keys_ask);
        let reread = Pins::parse(&pins.encode().unwrap_or_default()).unwrap_or_default();
        assert_eq!(reread, pins);
        assert!(
            switcher(&feed, reread.get("/other"), assistant::QUICK)
                .pinned
                .is_empty()
        );
    }
}
