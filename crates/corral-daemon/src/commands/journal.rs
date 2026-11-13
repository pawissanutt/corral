//! The command journal's records, its in-memory index, and in-flight commands.
use std::collections::{BTreeSet, HashMap};
use std::sync::{Condvar, Mutex};
use std::time::Instant;

use corral_core::backend::StoredRecord;
use corral_ops::command;
use corral_protocol::cmd::{CmdError, CmdOutput, CmdState};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use super::lock;

/// Settled output is kept for replays only up to this many bytes (stdout + stderr).
const RETAIN_MAX: usize = 64 * 1024;
const EVICT_BATCH: usize = 8;

/// One journal entry.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct Journal {
    /// `corral_ops::command::digest` of the submit; empty for tombstones.
    pub digest: String,
    pub state: JournalState,
    pub issued_at: String,
    pub expires_at: String,
    pub result: Option<Stored>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum JournalState {
    Reserved,
    Settled,
    Abandoned,
}

/// A settled result; both outputs are `None` when together they exceed `RETAIN_MAX`.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct Stored {
    state: CmdState,
    exit: u8,
    error: Option<CmdError>,
    stdout: Option<String>,
    stderr: Option<String>,
}

impl Stored {
    pub fn replay(self) -> Answer {
        let output = match (self.stdout, self.stderr) {
            (Some(stdout), Some(stderr)) => Some(CmdOutput { stdout, stderr }),
            _ => None,
        };
        Answer {
            state: self.state,
            exit: self.exit,
            error: self.error,
            output,
            replayed: true,
        }
    }
}

/// What a connection answers: a `CmdResult` plus the output its DATA frames carry.
#[derive(Debug, Clone)]
pub(super) struct Answer {
    pub state: CmdState,
    pub exit: u8,
    pub error: Option<CmdError>,
    pub output: Option<CmdOutput>,
    pub replayed: bool,
}

impl Answer {
    pub fn bare(state: CmdState) -> Self {
        Self {
            state,
            exit: u8::from(state != CmdState::Done),
            error: None,
            output: None,
            replayed: false,
        }
    }

    pub fn of(outcome: command::Outcome) -> Self {
        Self {
            state: outcome.state,
            exit: outcome.exit,
            error: outcome.error,
            output: Some(CmdOutput {
                stdout: outcome.stdout,
                stderr: outcome.stderr,
            }),
            replayed: false,
        }
    }

    pub fn stored(&self) -> Stored {
        let (stdout, stderr) = match &self.output {
            Some(output) if output.stdout.len() + output.stderr.len() <= RETAIN_MAX => {
                (Some(output.stdout.clone()), Some(output.stderr.clone()))
            }
            _ => (None, None),
        };
        Stored {
            state: self.state,
            exit: self.exit,
            error: self.error.clone(),
            stdout,
            stderr,
        }
    }
}

/// A command a worker took and has not finished; registered before its reservation.
pub(super) struct Flight {
    pub digest: String,
    landing: Mutex<Landing>,
    landed: Condvar,
}

enum Landing {
    Pending,
    Answered(Answer),
    /// Refused before execution: waiters read the journal instead.
    Released,
}

impl Flight {
    pub fn new(digest: String) -> Self {
        Self {
            digest,
            landing: Mutex::new(Landing::Pending),
            landed: Condvar::new(),
        }
    }

    pub fn land(&self, answer: Option<Answer>) {
        *lock(&self.landing) = answer.map_or(Landing::Released, Landing::Answered);
        self.landed.notify_all();
    }

    /// `None` while still pending at `deadline`; `Some(None)` when released without an answer.
    pub fn wait(&self, deadline: Instant) -> Option<Option<Answer>> {
        let timeout = deadline.saturating_duration_since(Instant::now());
        let (landing, _) = self
            .landed
            .wait_timeout_while(lock(&self.landing), timeout, |landing| {
                matches!(landing, Landing::Pending)
            })
            .unwrap_or_else(|error| error.into_inner());
        match &*landing {
            Landing::Pending => None,
            Landing::Answered(answer) => Some(Some(answer.clone())),
            Landing::Released => Some(None),
        }
    }
}

/// One principal's journal entries; `Some(expires)` entries are final and evictable.
#[derive(Default)]
struct Ledger {
    entries: HashMap<String, Option<Timestamp>>,
    done: BTreeSet<(Timestamp, String)>,
}

/// The in-memory journal index per principal, loaded once at start.
#[derive(Default)]
pub(super) struct Index(HashMap<String, Ledger>);

impl Index {
    pub fn load(rows: Vec<StoredRecord>) -> Self {
        let mut index = Self::default();
        for row in rows {
            // Nothing is in flight yet: reservations of an earlier run are as final as settled
            // entries (never re-executed), so they age out the same way.
            let expires = serde_json::from_value::<Journal>(row.doc)
                .ok()
                .map_or(Timestamp::MIN, |journal| expiry(&journal.expires_at));
            index.done(&row.key, expires);
        }
        index
    }

    fn ledger(&mut self, key: &str) -> &mut Ledger {
        self.0.entry(principal(key).to_owned()).or_default()
    }

    /// A new reservation.
    pub fn live(&mut self, key: &str) {
        self.ledger(key)
            .entries
            .entry(key.to_owned())
            .or_insert(None);
    }

    /// A settled or abandoned entry, evictable once `expires` is 10 minutes past.
    pub fn done(&mut self, key: &str, expires: Timestamp) {
        let ledger = self.ledger(key);
        if let Some(Some(old)) = ledger.entries.insert(key.to_owned(), Some(expires)) {
            ledger.done.remove(&(old, key.to_owned()));
        }
        ledger.done.insert((expires, key.to_owned()));
    }

    /// At most `EVICT_BATCH` final entries of `principal` that expired before `cutoff`.
    pub fn evictable(&self, principal: &str, cutoff: Timestamp) -> Vec<(Timestamp, String)> {
        self.0.get(principal).map_or_else(Vec::new, |ledger| {
            ledger
                .done
                .iter()
                .take_while(|(expires, _)| *expires < cutoff)
                .take(EVICT_BATCH)
                .cloned()
                .collect()
        })
    }

    /// Forgets an evicted entry unless it was reserved again meanwhile.
    pub fn evicted(&mut self, (expires, key): &(Timestamp, String)) {
        let ledger = self.ledger(key);
        if ledger.entries.get(key) == Some(&Some(*expires)) {
            ledger.entries.remove(key);
            ledger.done.remove(&(*expires, key.clone()));
        }
    }

    /// Every entry of `principal` the journal holds: reserved, settled or abandoned.
    pub fn count(&self, principal: &str) -> usize {
        self.0
            .get(principal)
            .map_or(0, |ledger| ledger.entries.len())
    }
}

/// The principal part of a journal key `local-<uid>/<cmd_id>`.
pub(super) fn principal(key: &str) -> &str {
    key.split_once('/').map_or(key, |(principal, _)| principal)
}

/// Unparsable expiries (never written by the daemon) count as long expired.
pub(super) fn expiry(text: &str) -> Timestamp {
    text.parse().unwrap_or(Timestamp::MIN)
}
