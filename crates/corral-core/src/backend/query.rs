//! Query and paging arguments of [`super::ReadView`].

use crate::blob_ref::BlobHash;

use super::rows::{BlobState, EventSeq, ScratchState};

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
    /// The open index, optionally limited to one project.
    Open {
        project: Option<String>,
    },
    /// Point lookups by task id (dependency sets); unknown ids are skipped.
    Ids(Vec<String>),
    Children {
        parent: String,
    },
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
    Leases,
    Claims,
    Authorizations {
        task: Option<String>,
    },
}
