//! Verdict callbacks are bound to one review attempt, not to the reusable gate attempt that
//! every replacement review of the same candidate shares.
use corral_core::{CandidateIdentity, ReviewRecord};

/// The token a review's prompt carries: the gate attempt qualified by its callback attempt.
/// Legacy records without a route keep the bare gate token.
pub fn token(review: &ReviewRecord) -> Option<String> {
    let gate = review.identity.as_ref()?.attempt.clone();
    let route = review.route.as_deref();
    Some(
        match route.and_then(|r| r.callback.as_deref().or(r.attempt.as_deref())) {
            Some(attempt) => format!("{gate}:{attempt}"),
            None => gate,
        },
    )
}

pub(crate) fn matches(review: &ReviewRecord, expected: &str) -> bool {
    token(review).as_deref() == Some(expected)
}

/// A retry of an undelivered request (still retained in the transport) for the same candidate
/// keeps that request's callback attempt, so the retained text is submitted unchanged and no
/// reviewer ever saw the previous token.
pub(crate) fn inherited(
    run: &corral_core::RunHead,
    identity: Option<&CandidateIdentity>,
) -> Option<String> {
    let child = run
        .current_child
        .as_deref()
        .filter(|c| c.pending_reply.is_some())?;
    let review = child
        .review
        .as_ref()
        .filter(|r| r.identity.as_ref() == identity && r.verdict.is_none())?;
    let route = review.route.as_deref()?;
    route.callback.clone().or_else(|| route.attempt.clone())
}
