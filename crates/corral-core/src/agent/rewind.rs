//! Worker turn checkpoints and owner rewinds (T-575). The worker host snapshots its worktree
//! into private per-worktree refs at each turn's start and end and appends one
//! `corral_checkpoint` session entry per snapshot; `corral agent rewind` keeps one durable
//! `RewindRecord` per attempt, whose stage says how far a restore got so an interrupted one can
//! be continued or aborted. Receipts, turns, usage and approvals are never rewound.
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use super::AgentRunState;

/// The custom Pi session entry the worker host appends after each checkpoint snapshot.
pub const CHECKPOINT_ENTRY: &str = "corral_checkpoint";
/// Rewind attempts a container keeps; the oldest finished one is evicted first.
pub const REWINDS_MAX: usize = 32;
/// Restored paths a record lists; `restored_count` keeps the full number.
pub const REWIND_PATHS_MAX: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointPhase {
    Start,
    End,
}

/// One `corral_checkpoint` entry's data. `id` is `g<generation>-t<turn>`; a start checkpoint
/// also records the session leaf the turn's prompt was appended under. A failed snapshot has
/// an `error` and no commit, and is never a rewind target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointEntry {
    pub id: String,
    pub phase: CheckpointPhase,
    pub generation: u32,
    pub turn: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
    pub git_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leaf: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// How far a rewind got. Each stage is committed before the work of the next one starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RewindStage {
    /// Checked and recorded; nothing is fenced or written yet.
    Prepared,
    /// The old generation is fenced and its owned job groups are reaped.
    Fenced,
    /// The worktree as the fence left it is saved under `before_ref`.
    Saved,
    /// Files are being restored; an interruption leaves some restored, all recoverable.
    Restoring,
    /// The checkpoint's files and HEAD are back; the next generation opens at `leaf`.
    Restored,
    /// The next generation was opened on the checkpoint's session cursor.
    Launched,
    /// Abandoned by the owner; any restored files were put back from `before`.
    Aborted,
}

impl RewindStage {
    pub fn finished(self) -> bool {
        matches!(self, RewindStage::Launched | RewindStage::Aborted)
    }
}

/// One owner rewind of a worker run to a checkpoint (`AgentRunState.rewinds`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RewindRecord {
    /// `rw-<n>`, unique within the container.
    pub id: String,
    /// The checkpoint ID (`g<generation>-t<turn>`) whose start snapshot is restored.
    pub checkpoint: String,
    pub commit: String,
    pub head: String,
    /// The session entry the next generation continues from.
    pub leaf: String,
    /// The generation that was current when the rewind was recorded, and is fenced.
    pub from_generation: u32,
    pub stage: RewindStage,
    /// The snapshot of the worktree after the fence, before any restore write.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_ref: Option<String>,
    /// The generation opened on the checkpoint's cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<u32>,
    /// Paths written back (at most `REWIND_PATHS_MAX`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub restored: Vec<String>,
    #[serde(default)]
    pub restored_count: usize,
    /// Paths left as they were because they changed while the rewind ran.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kept: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub at: Timestamp,
    pub updated: Timestamp,
}

impl AgentRunState {
    /// The one rewind that has neither launched nor been aborted.
    pub fn pending_rewind(&self) -> Option<&RewindRecord> {
        self.rewinds.iter().rev().find(|r| !r.stage.finished())
    }

    pub fn rewind_mut(&mut self, id: &str) -> Option<&mut RewindRecord> {
        self.rewinds.iter_mut().find(|r| r.id == id)
    }

    /// The session cursor the next generation opens at: only a restored, unlaunched rewind's.
    pub fn rewind_cursor(&self) -> Option<&str> {
        self.pending_rewind()
            .filter(|r| r.stage == RewindStage::Restored)
            .map(|r| r.leaf.as_str())
    }

    /// Appends a record, evicting the oldest finished ones beyond `REWINDS_MAX`.
    pub fn push_rewind(&mut self, record: RewindRecord) {
        self.rewinds.push(record);
        while self.rewinds.len() > REWINDS_MAX {
            match self.rewinds.iter().position(|r| r.stage.finished()) {
                Some(at) => {
                    self.rewinds.remove(at);
                }
                None => break,
            }
        }
    }
}
