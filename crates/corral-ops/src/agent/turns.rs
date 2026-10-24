//! Turn classification on `turn_ended` (completed, cancelled, provider_failed, owner_engaged,
//! awaiting_answer, waiting_external, no_report or no_verdict) and the OD11 nudge.
//! Owner: T-252 child worker-role. Until it lands no turn is classified.

/// The T-252 child that lands turn classification.
pub const OWNER: &str = "worker-role";
