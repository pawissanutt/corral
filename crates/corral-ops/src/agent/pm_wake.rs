//! The keyed PM wake outbox: one `question_wake` per question transition into the Pi-native PM
//! inbox, reposted after a crash and deduplicated by key. Owner: T-252 child pm-role.
use anyhow::Result;

use crate::Ctx;

/// The T-252 child that lands the outbox.
pub const OWNER: &str = "pm-role";

/// Reposts undelivered outbox entries; returns how many were posted. Nothing before pm-role.
pub fn repost(_ctx: &Ctx) -> Result<usize> {
    Ok(0)
}
