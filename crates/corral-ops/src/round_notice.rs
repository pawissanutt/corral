//! New-round text for a reused agent conversation (T-476). A resumed session still holds its
//! earlier rounds' todo lists and verdicts, so a bare "read the prompt file" request can look
//! like a repeat of work that is already done. Each notice is one line (it travels inside a
//! quoted shell argument) and names the round, the attempt and, for reviews, the candidate.
use corral_core::{RoleSession, Run, SessionAction, Task};

pub(crate) fn reused_session(session: &RoleSession) -> bool {
    matches!(
        session.action,
        SessionAction::Reused | SessionAction::Resumed
    )
}

pub(crate) fn reused(run: &Run) -> bool {
    run.session.as_ref().is_some_and(reused_session)
}

/// The attention reason for a reused reviewer whose turn ended with the review still open.
pub(crate) fn review_silent_reason(child: &Run) -> String {
    let attempt = child
        .review
        .as_ref()
        .and_then(crate::reviewer::review_callback_token);
    format!(
        "Review attempt {} (attempt ID {}, candidate {}) ended a turn without a verdict",
        child.id,
        attempt.as_deref().unwrap_or("unknown"),
        child.review.as_ref().map_or("unknown", |r| r.sha.as_str()),
    )
}

/// The notice for the newest review attempt of `task`'s current run, when that reviewer's
/// conversation is reused.
pub(crate) fn latest_review(task: &Task) -> Option<String> {
    let parent = task.current_run()?;
    let child = parent.review_runs.last().filter(|child| reused(child))?;
    let record = parent.review.as_ref()?;
    let attempt = crate::reviewer::review_callback_token(record)?;
    Some(format!(
        "NEW REVIEW ROUND {round}: review attempt {child_id} (attempt/callback ID {attempt}) of \
         {task_id} run {run_id} reviews candidate {sha}. Every earlier review round is finished; \
         its todo list, verdict and \"already complete\" state are superseded and this request is \
         not a repeat. Review this candidate and record a verdict for attempt {attempt} before \
         you end your turn.",
        round = parent.review_runs.len(),
        child_id = child.id,
        task_id = task.id,
        run_id = parent.id,
        sha = record.sha,
    ))
}

/// The notice for a worker whose conversation is reused for another changes round. `round` is
/// the autopilot's round number; an owner-requested round has none.
pub(crate) fn changes(task: &Task, run: &Run, round: Option<u32>) -> String {
    let round = round.map_or_else(String::new, |round| format!(" {round}"));
    format!(
        "NEW CHANGES ROUND{round}: {task_id} run {run_id} was returned from review with changes \
         requested. Every earlier round is finished; its todo list, `corral done` report and \
         \"complete\" state are superseded and this request is not a repeat. Address the findings, \
         rebase, run your gate and report with `corral done` again.",
        task_id = task.id,
        run_id = run.id,
    )
}
