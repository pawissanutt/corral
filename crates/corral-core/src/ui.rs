//! Personal terminal preferences and board shortcut defaults.

use std::collections::{BTreeMap, BTreeSet};

use jiff::SignedDuration;
use serde::{Deserialize, Serialize};

use crate::{Status, parse_age};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Accent {
    #[default]
    Cyan,
    Blue,
    Magenta,
    Green,
    Orange,
}

impl Accent {
    pub const ALL: [Self; 5] = [
        Self::Cyan,
        Self::Blue,
        Self::Magenta,
        Self::Green,
        Self::Orange,
    ];

    pub const fn key(self) -> &'static str {
        match self {
            Self::Cyan => "cyan",
            Self::Blue => "blue",
            Self::Magenta => "magenta",
            Self::Green => "green",
            Self::Orange => "orange",
        }
    }
}

/// Action name, canonical key, and Settings label. Arrows and Enter remain available.
pub const BOARD_KEYS: [(&str, char, &str); 29] = [
    ("left", 'h', "Select left"),
    ("down", 'j', "Select down"),
    ("up", 'k', "Select up"),
    ("right", 'l', "Select right"),
    ("agent", 'a', "Open agent"),
    ("reply", 'r', "Reply"),
    ("dispatch", 'd', "Dispatch"),
    ("drop", 'X', "Drop task"),
    ("move_left", 'H', "Move left"),
    ("move_right", 'L', "Move right"),
    ("capture", 'n', "New task"),
    ("pm", 'P', "Open PM"),
    ("artifacts", 'A', "Artifacts"),
    ("gallery", 'V', "Gallery"),
    ("history", 'Y', "History"),
    ("search", '/', "Search"),
    ("menu", '.', "Card menu"),
    ("settings", ',', "Settings"),
    ("autopilot", 'O', "Autopilot"),
    ("help", '?', "Help"),
    ("tour", 't', "Welcome tour"),
    ("pin", '*', "Pin task"),
    ("shelf", 'g', "Focus shelf"),
    ("shelf_panel", 'G', "Shelf panel"),
    ("ideas", 'i', "Ideas only"),
    ("done", 'D', "Show Done"),
    ("leave", 'q', "Leave board"),
    ("messages", 'M', "Messages"),
    ("capture_alias", 'c', "New task alias"),
];

/// `[ui]`: personal preferences; workspace values override them key by key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiCfg {
    pub mouse: bool,
    pub done_window: String,
    pub done_max: usize,
    pub columns: Vec<Status>,
    pub show_wip: bool,
    pub accent: Accent,
    pub compact_cards: bool,
    /// Show route warning glyphs on board cards without changing review policy.
    pub route_warnings: bool,
    pub notify_needs_me: bool,
    pub notify_review: bool,
    /// A single printable ASCII character per board action; omitted actions retain defaults.
    pub keys: BTreeMap<String, String>,
}

impl Default for UiCfg {
    fn default() -> Self {
        Self {
            mouse: true,
            done_window: "7d".into(),
            done_max: 10,
            columns: Status::COLUMNS.to_vec(),
            show_wip: true,
            accent: Accent::Cyan,
            compact_cards: false,
            route_warnings: false,
            notify_needs_me: false,
            notify_review: false,
            keys: BTreeMap::new(),
        }
    }
}

impl UiCfg {
    pub fn done_window(&self) -> SignedDuration {
        parse_age(&self.done_window).unwrap_or(SignedDuration::from_hours(24 * 7))
    }

    /// Physical character for a canonical board action.
    pub fn bound_key(&self, canonical: char) -> char {
        BOARD_KEYS
            .iter()
            .find(|(_, key, _)| *key == canonical)
            .and_then(|(action, _, _)| self.keys.get(*action))
            .and_then(|key| key.chars().next())
            .unwrap_or(canonical)
    }

    /// Resolve once, so swaps do not cascade and replaced defaults stop firing.
    pub fn canonical_key(&self, physical: char) -> Option<char> {
        if physical.is_ascii_digit() {
            return Some(physical);
        }
        BOARD_KEYS
            .iter()
            .find(|(_, key, _)| self.bound_key(*key) == physical)
            .map(|(_, key, _)| *key)
    }

    pub(crate) fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.columns.is_empty() {
            out.push("ui.columns must include at least one board column".into());
        }
        let mut columns = BTreeSet::new();
        for column in &self.columns {
            if !Status::COLUMNS.contains(column) || !columns.insert(column.label()) {
                out.push(format!(
                    "ui.columns: invalid or duplicate board column {column:?}"
                ));
            }
        }
        for (action, value) in &self.keys {
            if !BOARD_KEYS.iter().any(|(name, _, _)| name == action) {
                out.push(format!("ui.keys.{action}: unknown or fixed board action"));
            }
            if value.len() != 1
                || !value
                    .bytes()
                    .all(|b| b.is_ascii_graphic() && !b.is_ascii_digit())
            {
                out.push(format!(
                    "ui.keys.{action}: use one printable ASCII character other than 0-9"
                ));
            }
        }
        let mut used = BTreeMap::new();
        for (action, key, _) in BOARD_KEYS {
            let bound = self.bound_key(key);
            if let Some(other) = used.insert(bound, action) {
                out.push(format!(
                    "ui.keys.{action}: {bound:?} is also bound to {other}"
                ));
            }
        }
        out
    }
}
