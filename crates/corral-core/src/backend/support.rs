//! Injectable clock and fault points shared by every backend and the conformance suite.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use anyhow::anyhow;
use jiff::{SignedDuration, Timestamp};

use serde_json::Value;

use super::ApplyError;
use crate::blob_ref::BlobHash;

pub trait Clock: Send + Sync {
    fn now(&self) -> Timestamp;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        crate::clock::now()
    }
}

/// A clock that moves only when a test advances it.
#[derive(Clone)]
pub struct ManualClock(Arc<Mutex<Timestamp>>);

impl ManualClock {
    pub fn new(start: Timestamp) -> Self {
        Self(Arc::new(Mutex::new(start)))
    }

    pub fn advance(&self, by: Duration) {
        let mut now = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let step = SignedDuration::try_from(by).unwrap_or(SignedDuration::MAX);
        *now = now.saturating_add(step).unwrap_or(*now);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Timestamp {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

pub fn elapsed(earlier: Timestamp, later: Timestamp) -> Duration {
    Duration::try_from(later.duration_since(earlier)).unwrap_or(Duration::ZERO)
}

pub fn add(at: Timestamp, by: Duration) -> Timestamp {
    let step = SignedDuration::try_from(by).unwrap_or(SignedDuration::MAX);
    at.saturating_add(step).unwrap_or(at)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackendConfig {
    /// Reads on a view older than this fail with `ViewError::Expired` (plan section 2).
    pub view_max_age: Duration,
}

impl Default for BackendConfig {
    fn default() -> Self {
        Self {
            view_max_age: Duration::from_secs(120),
        }
    }
}

/// Counts every primitive write a backend performs inside `apply` and, when armed, fails the
/// operation right after the n-th one so tests can prove each operation is all-or-nothing.
#[derive(Debug, Default)]
pub struct FaultInjector {
    armed_at: AtomicU64,
    writes: AtomicU64,
}

impl FaultInjector {
    /// Fails the `nth` primitive write (1-based) counted from now.
    pub fn arm(&self, nth: u64) {
        self.writes.store(0, Ordering::SeqCst);
        self.armed_at.store(nth, Ordering::SeqCst);
    }

    pub fn disarm(&self) {
        self.armed_at.store(0, Ordering::SeqCst);
    }

    pub fn reset_count(&self) {
        self.writes.store(0, Ordering::SeqCst);
    }

    pub fn writes(&self) -> u64 {
        self.writes.load(Ordering::SeqCst)
    }

    /// Called by a backend after each primitive write of an operation in progress.
    pub fn after_write(&self, what: &str) -> Result<(), ApplyError> {
        let n = self.writes.fetch_add(1, Ordering::SeqCst) + 1;
        let armed = self.armed_at.load(Ordering::SeqCst);
        if armed != 0 && armed == n {
            self.disarm();
            return Err(ApplyError::Io(anyhow!(
                "injected failure after primitive write {n} ({what})"
            )));
        }
        Ok(())
    }
}

/// SHA-256 of `detail` as canonical JSON (object keys sorted at every depth), so equal
/// condition details digest equally whatever their key order (plan section 9).
pub fn detail_digest(detail: &Value) -> String {
    let mut text = String::new();
    canonical(detail, &mut text);
    BlobHash::of(text.as_bytes()).to_string()
}

fn canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(fields) => {
            let mut keys: Vec<&String> = fields.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                canonical(&fields[key], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical(item, out);
            }
            out.push(']');
        }
        scalar => out.push_str(&scalar.to_string()),
    }
}
