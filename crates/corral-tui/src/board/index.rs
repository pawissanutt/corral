//! Derived board state: column order, the Done history order and slot usage. Rebuilt when a
//! snapshot is applied, so frames, keys and mouse input touch only the cards they show.
use std::cmp::Reverse;

use corral_core::Limits;
use corral_core::work::{Work, record};

use super::*;

#[derive(Default)]
pub(super) struct BoardIndex {
    /// Per column other than Done (slots of `Status::COLUMNS`): positions in the board's tasks,
    /// stably ordered by priority.
    columns: [Vec<usize>; 6],
    /// Done tasks newest first, with the time the Done column orders them by.
    done: Vec<(usize, Timestamp)>,
    /// Positions in `done` of pinned tasks, ascending.
    done_pinned: Vec<usize>,
    /// Ids of every Done task, for the open-dependency and tracker-progress card fields.
    pub(super) done_ids: HashSet<String>,
    usage: Option<(Limits, SlotUsage)>,
}

fn slot(col: Status) -> Option<usize> {
    Status::COLUMNS.iter().position(|s| *s == col)
}

impl BoardIndex {
    pub(super) fn build(
        tasks: &[Task],
        finishes: &HashMap<String, Finish>,
        pinned: &HashSet<String>,
        attention: &crate::attention::Snapshot,
    ) -> Self {
        record(Work::BoardVisit, tasks.len() as u64);
        let mut index = BoardIndex::default();
        for (i, t) in tasks.iter().enumerate() {
            if t.status == Status::Done {
                index
                    .done
                    .push((i, finishes.get(&t.id).map_or(t.updated, |f| f.at)));
                index.done_ids.insert(t.id.clone());
            } else if let Some(slot) = crate::attention::column(t, attention).and_then(slot) {
                index.columns[slot].push(i);
            }
        }
        for column in &mut index.columns {
            column.sort_by(|a, b| tasks[*a].priority.cmp(&tasks[*b].priority));
        }
        index.done.sort_by_key(|row| Reverse(row.1));
        index.done_pinned = (index.done.iter().enumerate())
            .filter(|(_, (i, _))| pinned.contains(&tasks[*i].id))
            .map(|(position, _)| position)
            .collect();
        index
    }

    pub(super) fn counts(&self) -> [usize; 6] {
        std::array::from_fn(|i| {
            if Status::COLUMNS[i] == Status::Done {
                self.done.len()
            } else {
                self.columns[i].len()
            }
        })
    }

    /// The tasks of column `col` (not Done) by priority; Backlog keeps only ideas when `ideas_only`.
    pub(super) fn column<'t>(
        &self,
        tasks: &'t [Task],
        col: Status,
        ideas_only: bool,
    ) -> Vec<&'t Task> {
        let Some(slot) = slot(col) else {
            return Vec::new();
        };
        self.columns[slot]
            .iter()
            .map(|i| &tasks[*i])
            .filter(|t| !(ideas_only && col == Status::Backlog) || t.is_idea())
            .collect()
    }

    /// See `BoardView::done_cards`: the Done tasks at or after `cutoff`, newest first and at most
    /// `max`, then every older pinned one; and how many Done tasks that leaves out.
    pub(super) fn done_cards<'t>(
        &self,
        tasks: &'t [Task],
        cutoff: Timestamp,
        max: usize,
    ) -> (Vec<&'t Task>, usize) {
        let recent = (self.done.iter())
            .take_while(|(_, at)| *at >= cutoff)
            .take(max)
            .count();
        let mut shown: Vec<&Task> = self.done[..recent]
            .iter()
            .map(|(i, _)| &tasks[*i])
            .collect();
        shown.extend(
            (self.done_pinned.iter())
                .filter(|position| **position >= recent)
                .map(|position| &tasks[self.done[*position].0]),
        );
        let older = self.done.len() - shown.len();
        (shown, older)
    }

    /// Slot usage of `tasks` under `limits`, recomputed only when the limits differ.
    pub(super) fn usage(&mut self, tasks: &[Task], limits: &Limits) -> &SlotUsage {
        if self.usage.as_ref().is_none_or(|(known, _)| known != limits) {
            record(Work::BoardVisit, tasks.len() as u64);
            self.usage = Some((limits.clone(), SlotUsage::from_tasks(tasks, limits)));
        }
        let (_, usage) = self.usage.get_or_insert_with(Default::default);
        usage
    }
}
