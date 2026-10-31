//! Fixture capture timestamps ordered against the verify process's gate timestamp.

use jiff::Timestamp;
use serde_json::Value;

/// Use the persisted boundary, never a cross-process wall-clock reading that can
/// step either before the gate or into the validation process's future.
pub fn capture_time(attempt: &Value) -> Timestamp {
    attempt["gate"]["at"].as_str().unwrap().parse().unwrap()
}
