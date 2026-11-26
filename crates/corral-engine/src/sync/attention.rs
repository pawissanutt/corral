//! Advisory run attention. No task lifecycle transition or recovery action is implied.

use std::time::Duration;

use anyhow::{Context, Result};
use jiff::Timestamp;
use rusqlite::{OptionalExtension, params};

use crate::{
    Role, Run, Store,
    runner::pi::{Activity, InputState, Pickup},
};

const PICKUP_WINDOW_SECS: u64 = 30;

impl Store {
    pub(super) fn obligation_pickup(&self, run: &Run) -> Result<Option<Pickup>> {
        let kind = match run.role {
            Role::Worker => "sent_back",
            Role::Reviewer => "review_started",
        };
        let pickup: Option<String> = self
            .connection
            .query_row(
                "SELECT detail -> 'pickup' FROM events WHERE task=?1 AND kind=?2 AND run=?3
             AND seq > COALESCE((SELECT MAX(seq) FROM events WHERE task=?1 AND kind='done'),0)
             ORDER BY seq DESC LIMIT 1",
                params![run.task, kind, run.id],
                |row| row.get(0),
            )
            .optional()
            .context("read current board obligation input")?
            .flatten();
        match pickup {
            Some(pickup) => serde_json::from_str(&pickup).context("decode board obligation input"),
            None => Ok(None),
        }
    }
}

pub(super) fn elapsed(since: Option<Timestamp>, now: Timestamp, seconds: u64) -> bool {
    since.is_some_and(|since| {
        now >= since && now.duration_since(since).unsigned_abs() >= Duration::from_secs(seconds)
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Reason {
    PickupFailed,
    PickupTimedOut,
    Unfinished,
    PossiblyStalled,
}

impl Reason {
    pub(super) fn message(self, role: Role) -> &'static str {
        match (self, role) {
            (Self::PickupFailed | Self::PickupTimedOut, Role::Worker) => {
                "Send-back feedback wasn’t picked up. Open the session to follow up."
            }
            (Self::PickupFailed | Self::PickupTimedOut, Role::Reviewer) => {
                "Review request wasn’t picked up. Open the session to follow up."
            }
            (Self::Unfinished, Role::Worker) => {
                "Worker settled without submitting. Open the session to follow up."
            }
            (Self::Unfinished, Role::Reviewer) => {
                "Reviewer settled without a verdict. Open the session to follow up."
            }
            (Self::PossiblyStalled, _) => "Possibly stalled. Open the session to check progress.",
        }
    }
}

pub(super) fn pi_reason(
    activity: &Activity,
    pickup: Option<&Pickup>,
    since: Option<Timestamp>,
    now: Timestamp,
) -> Option<Reason> {
    if activity.waiting || activity.busy {
        return None;
    }
    if let Some(pickup) = pickup {
        let state = activity.inputs.get(&pickup.input_id).copied();
        match state {
            Some(InputState::Applied) => (),
            Some(InputState::Failed { turn }) if activity.turn > turn => (),
            Some(InputState::Failed { .. }) => return Some(Reason::PickupFailed),
            Some(InputState::Pending { quiet_ms }) => {
                return (quiet_ms >= 1_000 * PICKUP_WINDOW_SECS
                    && elapsed(since, now, PICKUP_WINDOW_SECS))
                .then_some(Reason::PickupTimedOut);
            }
            // A new turn after a failed or unknown pickup can still owe work,
            // but an earlier settled turn is never evidence for this request.
            None if pickup.before_turn.is_some_and(|turn| activity.turn > turn) => (),
            None if pickup.rejected => return Some(Reason::PickupFailed),
            None => return None,
        }
    }
    activity.settled.then_some(Reason::Unfinished)
}
