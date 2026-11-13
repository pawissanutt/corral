//! Plain-scan task search shared by the CLI and live board finder.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use corral_core::query::{Query, SearchContext};
use corral_core::{PinRef, Store, Task};

/// A store snapshot; typing a query does not reread tasks or events.
pub struct SearchData {
    pub tasks: Vec<Task>,
    pub pinned: HashSet<String>,
    pub extra_text: HashMap<String, String>,
}

impl SearchData {
    /// Rank task ids without retaining every task's history. Fuzzy task predicates still
    /// require each task, so visit one at a time and hydrate only selected output rows later.
    /// Incomplete catalogs use query-local fallback data without waiting for the builder.
    pub fn search_ids(store: &Store, input: &str, limit: usize) -> Result<Vec<String>> {
        match Self::search_ids_once(store, input, limit) {
            Err(error) if crate::content_index::catalog::is_corrupt(&error) => {
                crate::content_index::catalog::repair(&crate::Ctx::from_store(store.clone()))?;
                Self::search_ids_once(store, input, limit)
            }
            result => result,
        }
    }

    fn search_ids_once(store: &Store, input: &str, limit: usize) -> Result<Vec<String>> {
        let _span = corral_core::timing::span("search.tasks");
        let query = Query::parse(input, corral_core::clock::now())?;
        let ctx = crate::Ctx::from_store(store.clone());
        let published = crate::content_index::catalog::Catalog::open(&ctx, false)?;
        let _fallback = published
            .is_none()
            .then(|| corral_core::timing::span("search.fallback"));
        let catalog = match published {
            Some(catalog) => catalog,
            None => crate::content_index::catalog::Catalog::fallback(&ctx, false)?,
        };
        let ids: Vec<_> = catalog.task_ids().cloned().collect();
        let finishes = crate::finishes_for(&ctx.store, &ids)?;
        let pinned: HashSet<_> = ctx
            .store
            .pins()?
            .into_iter()
            .filter_map(|p| PinRef::parse(&p.target).ok())
            .map(|p| p.task().to_string())
            .collect();
        let mut ranked = Vec::new();
        {
            let _scan = corral_core::timing::span("search.task_scan");
            for id in ids {
                let task = ctx.store.get(&id)?;
                let mut extra = finishes
                    .get(&id)
                    .and_then(|finish| finish.landed.as_ref())
                    .map(|landed| {
                        format!(
                            "{} {} {}",
                            landed.sha,
                            landed.strategy,
                            landed.strategy_label()
                        )
                    })
                    .unwrap_or_default();
                extra.push_str(&catalog.task_text(&id)?);
                if let Some(score) = query.score(
                    &task,
                    SearchContext {
                        pinned: pinned.contains(&id),
                        extra_text: &extra,
                    },
                ) {
                    ranked.push((id, score, task.updated));
                }
            }
        }
        let _assembly = corral_core::timing::span("search.result_assembly");
        ranked.sort_by(|a, b| {
            b.1.cmp(&a.1)
                .then_with(|| b.2.cmp(&a.2))
                .then_with(|| b.0.cmp(&a.0))
        });
        ranked.truncate(limit);
        Ok(ranked.into_iter().map(|(id, _, _)| id).collect())
    }

    /// Load tasks at every status plus pins and landed history metadata.
    pub fn load(store: &Store) -> Result<Self> {
        let tasks = store.list()?;
        let pinned = store
            .pins()?
            .into_iter()
            .filter_map(|p| PinRef::parse(&p.target).ok())
            .map(|r| r.task().to_string())
            .collect();
        let mut extra_text: HashMap<String, String> = crate::finishes(store, &tasks)?
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
        for (id, text) in crate::content_search::task_text(store, &tasks)? {
            extra_text.entry(id).or_default().push_str(&text);
        }
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
    SearchData::search_ids(store, input, usize::MAX)?
        .into_iter()
        .map(|id| store.get(&id))
        .collect()
}
