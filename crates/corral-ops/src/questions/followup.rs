//! Revisioned follow-ups share the answer transaction and recipient inbox lock. A follow-up
//! never answers or reopens a question: concurrent answer/follow-up commands have one winner.
use anyhow::{Context, Result};
use corral_core::agent::{
    AgentQuestion, QUESTION_FOLLOWUP_MAX_BYTES, QUESTION_FOLLOWUPS_MAX, QuestionActor,
    QuestionAttribution, QuestionFollowup,
};

use super::triage::{Change, NeedsMe};
use super::{Actor, Applied, RefusalKind, bounded, refuse, transition};
use crate::Ctx;

pub(super) fn attribution(ctx: &Ctx) -> Result<QuestionAttribution> {
    let (_, via, detail) = ctx.store.event_attribution()?;
    let via = via.unwrap_or_else(|| "cli".into());
    let device = via.strip_prefix("app:").map(str::to_owned).or_else(|| {
        detail
            .get("owner_authority")
            .and_then(|d| d.get("device"))
            .and_then(|v| v.as_str())
            .map(str::to_owned)
    });
    let name = detail
        .get("attribution")
        .and_then(|d| d.get("name"))
        .or_else(|| detail.get("device").and_then(|d| d.get("name")))
        .or_else(|| detail.get("owner_authority").and_then(|d| d.get("name")))
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    Ok(QuestionAttribution { via, device, name })
}

pub(super) fn append(
    question: &mut AgentQuestion,
    text: &str,
    attribution: QuestionAttribution,
    now: jiff::Timestamp,
) -> Result<()> {
    bounded(text, QUESTION_FOLLOWUP_MAX_BYTES, "follow-up")?;
    if !question.recorded_pending() {
        return Err(refuse(
            RefusalKind::State,
            "only pending questions take follow-ups",
        ));
    }
    if question.followups.len() >= QUESTION_FOLLOWUPS_MAX {
        return Err(refuse(
            RefusalKind::Limit,
            "question follow-up limit reached",
        ));
    }
    let state = question.state;
    let unattended_at = question.unattended_at;
    transition(question, state, "followup", QuestionActor::Owner, None, now);
    question.unattended_at = unattended_at;
    question.followups.push(QuestionFollowup {
        revision: question.revision,
        by: QuestionActor::Owner,
        attribution,
        at: now,
        text: text.into(),
    });
    Ok(())
}

/// Adds owner context without deciding the question. Refuses overflow, closure, settled state
/// and a stale canonical question revision; never truncates conversation or retries a race.
pub fn followup(ctx: &Ctx, key: &str, revision: u32, text: &str) -> Result<Applied> {
    if corral_core::agent::owner_question::owner_question_container(key).is_some() {
        return super::owner::followup(ctx, key, revision, text);
    }
    let attribution = attribution(ctx)?;
    super::triage::transact(
        ctx,
        key,
        Some(revision),
        &Actor::Owner,
        QuestionActor::Owner,
        ctx.store.now(),
        |_, _, current, now| {
            let mut question = current.clone();
            append(&mut question, text, attribution, now)?;
            let body = format!(
                "Owner follow-up to {} (revision {}):\n{}",
                key, question.revision, text
            );
            Ok(Some(Change {
                question,
                message: Some(("question_followup", body)),
                needs_me: NeedsMe::Unchanged,
            }))
        },
    )?
    .context("the follow-up produced no change")
}
