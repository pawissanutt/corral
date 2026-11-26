//! Which optional columns the Tasks table shows on this device: a display preference kept under
//! [`KEY`] as the shown columns' keys joined by commas, never sent to corrald. Status, id, title
//! and the next step always show. The default leaves out Cost (corrald serves no per-task cost,
//! so every cell is a dash) and Profile (the inspector names it, and it was the first to give
//! way), which pays for the Priority column and leaves the title its room.

use std::collections::BTreeSet;

use crate::model::list::Column;
use crate::model::projects::Scope;

pub const KEY: &str = "corral.tasks.columns";
/// The Columns control's words and its spoken name.
pub const CONTROL: &str = "Columns";
pub const CONTROL_LABEL: &str = "Choose the Tasks table's columns";
/// The control's last row, back to [`Chosen::default`].
pub const RESET: &str = "Restore default columns";

/// The columns the owner may show or hide, in the table's order, with their menu names.
pub const OPTIONAL: [(&str, &str); 9] = [
    ("priority", "Priority"),
    ("now", "Now"),
    ("pipeline", "Pipeline"),
    ("size", "Size"),
    ("difficulty", "Difficulty"),
    ("project", "Project"),
    ("profile", "Profile"),
    ("cost", "Cost"),
    ("age", "Age"),
];

const HIDDEN_BY_DEFAULT: [&str; 2] = ["profile", "cost"];

/// The optional columns the owner chose to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chosen {
    shown: BTreeSet<&'static str>,
}

impl Default for Chosen {
    fn default() -> Self {
        Self {
            shown: OPTIONAL
                .iter()
                .map(|(key, _)| *key)
                .filter(|key| !HIDDEN_BY_DEFAULT.contains(key))
                .collect(),
        }
    }
}

fn known(key: &str) -> Option<&'static str> {
    OPTIONAL
        .iter()
        .map(|(known, _)| *known)
        .find(|known| *known == key)
}

impl Chosen {
    /// Whether the column of `key` shows; a column that is not optional always does.
    pub fn shows(&self, key: &str) -> bool {
        known(key).is_none_or(|key| self.shown.contains(key))
    }

    /// Shows a hidden optional column or hides a shown one.
    pub fn toggle(&mut self, key: &str) {
        if let Some(key) = known(key)
            && !self.shown.remove(key)
        {
            self.shown.insert(key);
        }
    }

    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Reads what [`Chosen::encode`] wrote. A key this version does not know is passed over; a
    /// value naming none it knows is no preference.
    pub fn parse(stored: &str) -> Option<Chosen> {
        let keys: Vec<&str> = stored
            .split(',')
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .collect();
        let shown: BTreeSet<&'static str> = keys.iter().filter_map(|key| known(key)).collect();
        (shown.len() == keys.len() || !shown.is_empty()).then_some(Chosen { shown })
    }

    pub fn encode(&self) -> String {
        OPTIONAL
            .iter()
            .map(|(key, _)| *key)
            .filter(|key| self.shown.contains(key))
            .collect::<Vec<_>>()
            .join(",")
    }

    /// For each of `columns`, whether the owner shows it.
    pub fn mask(&self, columns: &[Column]) -> Vec<bool> {
        columns
            .iter()
            .map(|column| self.shows(column.key))
            .collect()
    }
}

/// One row of the Columns control: the column's key, its name and whether it shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub key: &'static str,
    pub label: &'static str,
    pub on: bool,
}

/// The Columns control's rows. With one project in scope the table has no Project column, so
/// the control does not offer it.
pub fn choices(chosen: &Chosen, scope: &Scope) -> Vec<Choice> {
    OPTIONAL
        .iter()
        .filter(|(key, _)| *key != "project" || scope.project().is_none())
        .map(|(key, label)| Choice {
            key,
            label,
            on: chosen.shows(key),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default traded the always-empty Cost and the inspector's Profile for Priority, and a
    /// toggled choice survives the trip through this device's storage until Restore.
    #[test]
    fn choices_toggle_survive_storage_and_restore_the_default() {
        let mut chosen = Chosen::default();
        assert!(chosen.shows("priority") && chosen.shows("title"));
        assert!(!chosen.shows("cost") && !chosen.shows("profile"));
        chosen.toggle("priority");
        chosen.toggle("cost");
        chosen.toggle("title");
        assert!(!chosen.shows("priority") && chosen.shows("cost") && chosen.shows("title"));
        let kept = Chosen::parse(&chosen.encode());
        assert_eq!(kept.as_ref(), Some(&chosen));
        assert!(!chosen.is_default());
        // Every optional column hidden is a choice too, not a missing preference.
        let mut none = Chosen::default();
        for (key, _) in OPTIONAL {
            if none.shows(key) {
                none.toggle(key);
            }
        }
        assert_eq!(Chosen::parse(&none.encode()), Some(none));
        // A key from another version is passed over; a value of none known is no preference.
        assert_eq!(Chosen::parse("age,later"), Chosen::parse("age"));
        assert_eq!(Chosen::parse("later"), None);
    }

    #[test]
    fn a_project_scope_offers_no_project_column() {
        let offered = |scope: &Scope| {
            choices(&Chosen::default(), scope)
                .iter()
                .any(|choice| choice.key == "project")
        };
        assert!(offered(&Scope::All));
        assert!(!offered(&Scope::Project("site".into())));
    }
}
