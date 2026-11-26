//! The Tasks table's optional columns on this device (`model::tasks::columns`), saved under
//! `corral.tasks.columns` on every change.

use crate::model::tasks::columns::{Chosen, KEY};

use super::{AppState, preference, remember};

pub(super) fn load() -> Chosen {
    preference(KEY, Chosen::parse).unwrap_or_default()
}

impl AppState {
    /// Shows or hides the optional column of `key` and keeps the choice.
    pub fn toggle_task_column(&self, key: &str) {
        self.change_task_columns(|chosen| chosen.toggle(key));
    }

    /// Back to the default columns, kept.
    pub fn reset_task_columns(&self) {
        self.change_task_columns(|chosen| *chosen = Chosen::default());
    }

    fn change_task_columns(&self, change: impl FnOnce(&mut Chosen)) {
        let mut next = self.task_columns.get_untracked();
        change(&mut next);
        if self.task_columns.with_untracked(|now| *now != next) {
            remember(KEY, &next.encode());
            self.task_columns.set(next);
        }
    }
}
