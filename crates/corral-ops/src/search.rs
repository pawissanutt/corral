//! Plain-scan task search shared by the CLI and live board finder.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use corral_core::query::{Query, SearchContext};
use corral_core::{PinRef, Store, Task};
use jiff::Timestamp;

/// A store snapshot; typing a query does not reread tasks or events.
pub struct SearchData {
    pub tasks: Vec<Task>,
    pub pinned: HashSet<String>,
    pub extra_text: HashMap<String, String>,
}

impl SearchData {
    /// Load tasks at every status plus pins and landed history metadata.
    pub fn load(store: &Store) -> Result<Self> {
        let tasks = store.list()?;
        let pinned = store
            .pins()?
            .into_iter()
            .filter_map(|p| PinRef::parse(&p.target).ok())
            .map(|r| r.task().to_string())
            .collect();
        let extra_text = crate::finishes(store, &tasks)?
            .into_iter()
            .filter_map(|(id, finish)| {
                let landed = finish.landed?;
                Some((
                    id,
                    format!(
                        "{} {} {}",
                        landed.sha,
                        landed.strategy,
                        landed.strategy_label()
                    ),
                ))
            })
            .collect();
        Ok(Self {
            tasks,
            pinned,
            extra_text,
        })
    }

    /// Task-local supplemental data, also usable by the history view.
    pub fn context(&self, task: &Task) -> SearchContext<'_> {
        SearchContext {
            pinned: self.pinned.contains(&task.id),
            extra_text: self.extra_text.get(&task.id).map_or("", String::as_str),
        }
    }

    /// Matching indexes, descending relevance, then newest update and highest id.
    pub fn ranked(&self, query: &Query) -> Vec<usize> {
        let mut matches: Vec<_> = self
            .tasks
            .iter()
            .enumerate()
            .filter_map(|(i, t)| query.score(t, self.context(t)).map(|score| (i, score)))
            .collect();
        matches.sort_by(|&(a, sa), &(b, sb)| {
            sb.cmp(&sa)
                .then_with(|| self.tasks[b].updated.cmp(&self.tasks[a].updated))
                .then_with(|| self.tasks[b].id.cmp(&self.tasks[a].id))
        });
        matches.into_iter().map(|(i, _)| i).collect()
    }
}

/// Search every task with the shared language, returning tasks in relevance order.
pub fn search_tasks(store: &Store, input: &str) -> Result<Vec<Task>> {
    let query = Query::parse(input, Timestamp::now())?;
    let data = SearchData::load(store)?;
    Ok(data
        .ranked(&query)
        .into_iter()
        .map(|i| data.tasks[i].clone())
        .collect())
}
