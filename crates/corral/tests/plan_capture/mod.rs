//! Fixture capture timestamps ordered against the verify process's gate timestamp.

use jiff::Timestamp;
use serde_json::Value;

/// Clamp the fixture's wall clock to the gate boundary. Clocks in different processes
/// can step backwards between verification and observation.
pub fn capture_time(attempt: &Value, now: Timestamp) -> Timestamp {
    let gate: Timestamp = attempt["gate"]["at"].as_str().unwrap().parse().unwrap();
    now.max(gate)
}
