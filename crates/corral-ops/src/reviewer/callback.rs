//! Verdict callbacks are bound to one review attempt, not to the reusable gate attempt that
//! a superseded routine review and its stronger replacement of the same candidate share.
use corral_core::{ReviewRecord, Run};

/// The token a review's prompt carries. A stronger replacement qualifies the gate attempt with
/// its review child, so the superseded reviewer's bare gate token cannot record on it; other
/// attempts keep the bare gate token, which a retained PM request must repeat exactly.
pub(crate) fn token(review: &ReviewRecord) -> Option<String> {
    let gate = review.identity.as_ref()?.attempt.clone();
    let route = review.route.as_deref();
    Some(
        match route
            .filter(|r| r.escalated_from.is_some())
            .and_then(|r| r.attempt.as_deref())
        {
            Some(child) => format!("{gate}:{child}"),
            None => gate,
        },
    )
}

pub(crate) fn matches(_run: &Run, review: &ReviewRecord, expected: &str) -> bool {
    token(review).as_deref() == Some(expected)
}
