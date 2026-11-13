//! Bounded, conservative classification of a pinned store's change feed.
use std::collections::{BTreeMap, BTreeSet};

use corral_core::backend::{ChangeScope, ChangeSeq, ReadView, TaskNum};

const LEASE_AREA: &str = "leases";
// Keep in sync with corral_protocol::COMMANDS_AREA: journal writes are not application changes.
const COMMANDS_AREA: &str = "app-commands";

#[derive(Debug, Clone, Default)]
pub struct FeedDelta {
    /// Gap, read failure, backwards view or more than `limit` entries.
    pub overflow: bool,
    pub tasks: BTreeSet<TaskNum>,
    pub task_ids: BTreeSet<String>,
    /// Record area -> keys, excluding lease bookkeeping and the command journal.
    pub areas: BTreeMap<String, BTreeSet<String>>,
    pub event_kinds: BTreeSet<String>,
    pub scratch: bool,
    pub lifecycle: bool,
    pub aggregates: bool,
    /// A blob was reserved, written or released; no task is implied.
    /// The owning task's Artifact/Task scope is recorded separately.
    pub blobs: bool,
    /// Newest entry timestamp for each event kind.
    pub newest_event_at: BTreeMap<String, jiff::Timestamp>,
    pub through: ChangeSeq,
}

impl FeedDelta {
    pub fn since(view: &dyn ReadView, after: ChangeSeq, limit: usize) -> FeedDelta {
        let through = view.seq();
        let mut delta = Self {
            through,
            ..Self::default()
        };
        if after > through || view.ensure_fresh().is_err() {
            delta.overflow = true;
            return delta;
        }
        let mut cursor = after;
        let mut scanned = 0;
        while cursor < through {
            if scanned == limit {
                delta.overflow = true;
                break;
            }
            let page_limit = (limit - scanned).min(128) as u32;
            // FeedError::Gap and every other read failure both invalidate the delta.
            let Ok(page) = view.changes(cursor, page_limit) else {
                delta.overflow = true;
                break;
            };
            if page.entries.is_empty() || page.entries.len() > page_limit as usize {
                delta.overflow = true;
                break;
            }
            scanned += page.entries.len();
            for entry in page.entries {
                if entry.seq <= cursor || entry.seq > through || entry.scopes.is_empty() {
                    delta.overflow = true;
                    return delta;
                }
                cursor = entry.seq;
                for scope in entry.scopes {
                    match scope {
                        ChangeScope::Task { num, .. } => {
                            delta.tasks.insert(num);
                        }
                        ChangeScope::Record { area, key } => {
                            if area != LEASE_AREA && area != COMMANDS_AREA {
                                delta.areas.entry(area).or_default().insert(key);
                            }
                        }
                        ChangeScope::Events { task, kind } => {
                            if let Some(task) = task {
                                delta.task_ids.insert(task);
                            }
                            delta.event_kinds.insert(kind.clone());
                            delta
                                .newest_event_at
                                .entry(kind)
                                .and_modify(|at| *at = (*at).max(entry.at))
                                .or_insert(entry.at);
                        }
                        ChangeScope::Artifact { task } | ChangeScope::Condition { task, .. } => {
                            delta.task_ids.insert(task);
                        }
                        ChangeScope::Lifecycle { task } => {
                            delta.lifecycle = true;
                            delta.task_ids.insert(task);
                        }
                        ChangeScope::Scratch { .. } => delta.scratch = true,
                        ChangeScope::Aggregate { .. } => delta.aggregates = true,
                        ChangeScope::Blob { .. } => delta.blobs = true,
                    }
                }
            }
            if !page.more {
                delta.overflow |= cursor != through;
                break;
            }
            if cursor == through {
                delta.overflow = true;
            }
        }
        delta
    }

    /// Whether any application change (or an incomplete classification) was observed.
    pub fn any(&self) -> bool {
        self.overflow
            || !self.tasks.is_empty()
            || !self.task_ids.is_empty()
            || !self.areas.is_empty()
            || !self.event_kinds.is_empty()
            || self.scratch
            || self.lifecycle
            || self.aggregates
            || self.blobs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use corral_core::{Status, Store};

    #[test]
    fn lease_only_feed_is_quiet_and_task_write_is_classified() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::init(tmp.path()).unwrap();
        let after = store.view().unwrap().seq();
        store
            .save_record(LEASE_AREA, "reader", &serde_json::json!({"held": true}))
            .unwrap();
        let view = store.view().unwrap();
        let delta = FeedDelta::since(view.as_ref(), after, 128);
        assert!(delta.through > after);
        assert!(!delta.any());

        let after = delta.through;
        store
            .save_record(
                COMMANDS_AREA,
                "command",
                &serde_json::json!({"state": "done"}),
            )
            .unwrap();
        assert!(!FeedDelta::since(store.view().unwrap().as_ref(), after, 128).any());

        let after = store.view().unwrap().seq();
        let task = store.create("Changed", None, Status::Ready, "").unwrap();
        let view = store.view().unwrap();
        let num = view.last_task_num().unwrap().unwrap();
        let delta = FeedDelta::since(view.as_ref(), after, 128);
        assert!(delta.any());
        assert!(!delta.overflow);
        assert_eq!(delta.tasks, BTreeSet::from([num]));
        assert!(delta.task_ids.iter().all(|id| id == &task.id));

        let after = delta.through;
        store.put_gate_blob(b"gate evidence").unwrap();
        let delta = FeedDelta::since(store.view().unwrap().as_ref(), after, 128);
        assert!(delta.blobs);
        assert!(delta.any());
        assert!(!delta.overflow);
        assert!(delta.tasks.is_empty());
        assert!(delta.task_ids.is_empty());
    }
}
