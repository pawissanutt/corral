//! Query and paging arguments of [`super::ReadView`].

use crate::blob_ref::BlobHash;

use super::rows::{BlobState, EventSeq, LifecycleOutcome, ScratchState, TaskPart};

#[path = "artifact_reviews.rs"]
pub(crate) mod artifact_reviews;

/// One page request. `after` is the opaque cursor a previous page returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageReq {
    pub after: Option<String>,
    pub limit: u32,
}

impl PageReq {
    pub fn first(limit: u32) -> Self {
        Self { after: None, limit }
    }
}

/// One page of results; `next` is the cursor of the following page, if any.
#[derive(Debug, Clone, PartialEq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeadQuery {
    /// Latest tasks of a project, ordered by `(updated DESC, id DESC)`.
    ProjectUpdated {
        project: String,
    },
    /// The open index, optionally limited to one project.
    Open {
        project: Option<String>,
    },
    /// Point lookups by task id (dependency sets); unknown ids are skipped.
    Ids(Vec<String>),
    Children {
        parent: String,
    },
    /// Tasks of any status in `project` created strictly after `after`, in `(created, id)`
    /// order: the operating policy's capped new-task page (`tasks_project_created`).
    ProjectCreatedAfter {
        project: String,
        after: jiff::Timestamp,
    },
    /// Tasks carrying an edge of `kind` to `other` (`task_edges_other`), by number.
    EdgeTo {
        kind: String,
        other: String,
    },
    /// Tasks of one status, most recently updated first, ties by number descending
    /// (`tasks_status_updated`): the Done column's page.
    RecentByStatus {
        status: String,
    },
    /// Tasks whose unpaused dashboard schedule is due by `now` (`tasks_dashboard_due`), by
    /// number.
    DashboardDue {
        now: jiff::Timestamp,
    },
    /// Tasks of any status holding an unreleased cleanup-ledger pane (open `cleanup_pane` rows,
    /// read through `task_rows_open`), by number: bounded by panes still owed cleanup.
    UnreleasedPanes,
}

/// Rows of one family of one task, in key order (or reverse key order), matching every given
/// filter. Backends select by key, key prefix, index token and flags without decoding the
/// documents of rows they do not return.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowQuery {
    pub part: TaskPart,
    /// Exactly this key.
    pub key: Option<String>,
    /// Keys strictly greater than this key, independent of the opaque page cursor.
    pub after_key: Option<String>,
    /// Keys starting with this text (a parent key followed by `/`).
    pub prefix: Option<String>,
    /// Rows carrying this index token.
    pub index: Option<String>,
    pub current_only: bool,
    pub open_only: bool,
    pub newest_first: bool,
}

impl RowQuery {
    pub fn part(part: TaskPart) -> Self {
        Self {
            part,
            key: None,
            after_key: None,
            prefix: None,
            index: None,
            current_only: false,
            open_only: false,
            newest_first: false,
        }
    }

    pub fn key(part: TaskPart, key: impl Into<String>) -> Self {
        Self {
            key: Some(key.into()),
            ..Self::part(part)
        }
    }

    pub fn under(part: TaskPart, prefix: impl Into<String>) -> Self {
        Self {
            prefix: Some(prefix.into()),
            ..Self::part(part)
        }
    }

    pub fn after_key(mut self, key: impl Into<String>) -> Self {
        self.after_key = Some(key.into());
        self
    }

    pub fn with_index(mut self, token: impl Into<String>) -> Self {
        self.index = Some(token.into());
        self
    }

    pub fn current(mut self) -> Self {
        self.current_only = true;
        self
    }

    pub fn open(mut self) -> Self {
        self.open_only = true;
        self
    }

    pub fn newest_first(mut self) -> Self {
        self.newest_first = true;
        self
    }

    /// Whether a stored row matches every filter but the order.
    pub fn matches(&self, row: &super::HistoryRow) -> bool {
        row.part == self.part
            && self.key.as_ref().is_none_or(|key| &row.key == key)
            && self.after_key.as_ref().is_none_or(|key| &row.key > key)
            && self
                .prefix
                .as_ref()
                .is_none_or(|prefix| row.key.starts_with(prefix.as_str()))
            && self
                .index
                .as_ref()
                .is_none_or(|token| row.index.contains(token))
            && (!self.current_only || row.current)
            && (!self.open_only || row.open)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskOrder {
    IdAsc,
    UpdatedDesc,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskQuery {
    /// Empty matches every status.
    pub statuses: Vec<String>,
    pub project: Option<String>,
    pub order: TaskOrder,
}

/// Events in ascending seq order matching every given filter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EventQuery {
    pub after: Option<EventSeq>,
    pub task: Option<String>,
    /// Empty matches every kind.
    pub kinds: Vec<String>,
    pub run: Option<String>,
    pub newest_first: bool,
    /// Excludes malformed legacy rows before paging.
    pub valid_only: bool,
}

/// Condition runs in `opened_seq` order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConditionQuery {
    pub task: Option<String>,
    pub kind: Option<String>,
    pub open_only: bool,
}

/// Records in `(area, key)` order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecordQuery {
    pub area: Option<String>,
    pub key: Option<String>,
}

/// Blob rows in hash order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlobQuery {
    pub hash: Option<BlobHash>,
    pub state: Option<BlobState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageQuery {
    /// Empty `states` matches every state.
    Scratch {
        states: Vec<ScratchState>,
    },
    Lifecycle {
        task: Option<String>,
    },
    /// The lifecycle row `(task, generation, target)` (a primary-key read).
    LifecycleKey {
        task: String,
        generation: String,
        target: String,
    },
    /// Lifecycle rows of `outcome` whose `next_check_at` is at or before `now`, earliest check
    /// first, read in `lifecycle_due` index order so a page costs only its rows.
    LifecycleDue {
        outcome: LifecycleOutcome,
        now: jiff::Timestamp,
    },
    /// Lifecycle rows whose `policy_digest` sorts before `digest`, in index order.
    LifecyclePolicyBelow {
        digest: String,
    },
    /// Lifecycle rows whose `policy_digest` sorts after `digest`, in index order.
    LifecyclePolicyAbove {
        digest: String,
    },
    Leases,
    Claims,
    Authorizations {
        task: Option<String>,
    },
    /// One authorization by its exact identity, independent of board and task history.
    Authorization {
        id: String,
    },
}
