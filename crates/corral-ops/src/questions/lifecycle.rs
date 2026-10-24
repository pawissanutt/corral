//! Sweeps over one task that the sync hook (integration-seams) calls per live Pi-native
//! container: F4 terminalization, owner deadlines and the unattended rule. Each is idempotent.
use anyhow::Result;
use corral_core::Task;
use corral_core::agent::{
    self, AgentQuestion, AnswerBy, ContainerRef, QuestionActor, QuestionAnswer, QuestionState,
    QuestionTerminal, TurnClass,
};
use corral_core::inbox::{Inbox, ReceiptLevel, TerminalState};
use jiff::Timestamp;

use super::triage::{Actor, Change, NeedsMe, answer_body, transact};
use super::{PI_NATIVE_MESSAGE_KINDS, QuestionKey, QuestionRefusal, transition, triage, views};
use crate::Ctx;

/// Persists F4 closure once: every recorded-pending question of a closed worker run or review
/// child becomes cancelled with the closure reason, and the container's undelivered Pi-native
/// messages are cancelled `recipient_closed`. Returns the keys cancelled by this call.
pub fn terminalize(ctx: &Ctx, task_id: &str) -> Result<Vec<String>> {
    let now = Timestamp::now();
    let mut cancelled = Vec::new();
    let mut closed = Vec::new();
    ctx.store.try_update(task_id, |task| {
        for container in super::containers(task) {
            let Some(reason) = agent::container_closure(task, &container) else {
                continue;
            };
            let Some(state) = agent::container_state_mut(task, &container) else {
                continue;
            };
            for question in state.questions.iter_mut().filter(|q| q.recorded_pending()) {
                transition(
                    question,
                    QuestionState::Cancelled,
                    "terminalize",
                    QuestionActor::System,
                    Some(reason.as_str().into()),
                    now,
                );
                question.terminal = Some(QuestionTerminal {
                    state: QuestionState::Cancelled,
                    reason: reason.as_str().into(),
                    at: now,
                });
                cancelled.push(question.key.clone());
            }
            closed.push(container);
        }
        Ok(!cancelled.is_empty())
    })?;
    super::reconcile_events(&ctx.store, task_id)?;
    for container in &closed {
        cancel_messages(ctx, container)?;
    }
    Ok(cancelled)
}

fn cancel_messages(ctx: &Ctx, container: &ContainerRef) -> Result<()> {
    let Some(recipient) = super::recipient(container) else {
        return Ok(());
    };
    let inbox = Inbox::new(&ctx.store, recipient)?;
    for message in inbox.messages()? {
        if !PI_NATIVE_MESSAGE_KINDS.contains(&message.kind.as_str())
            || inbox.terminal(message.seq)?.is_some()
            || inbox
                .receipt(message.seq, ReceiptLevel::Observed)?
                .is_some()
        {
            continue;
        }
        inbox.mark_terminal(
            message.seq,
            TerminalState::Cancelled,
            "recipient_closed",
            "questions::terminalize",
        )?;
    }
    Ok(())
}

/// Runs one sweep step; a refusal means the question moved on since the snapshot.
fn sweep_one(
    applied: Result<Option<super::Applied>>,
    key: String,
    out: &mut Vec<String>,
) -> Result<()> {
    match applied {
        Ok(Some(_)) => out.push(key),
        Ok(None) => {}
        Err(error) if error.is::<QuestionRefusal>() => {}
        Err(error) => return Err(error),
    }
    Ok(())
}

fn deadline_due(question: &AgentQuestion, now: Timestamp) -> bool {
    question.recorded_pending()
        && question
            .deadline
            .as_ref()
            .is_some_and(|d| d.at <= now && d.for_revision == question.revision)
}

/// Expires pending questions whose owner deadline passed at an unchanged revision, answering
/// them `owner_deadline` with the owner's instruction, delivered like any answer. Without an
/// owner deadline nothing ever expires.
pub fn expire_deadlines(ctx: &Ctx, task_id: &str, now: Timestamp) -> Result<Vec<String>> {
    let task = ctx.store.get(task_id)?;
    let due: Vec<String> = views(&task)
        .into_iter()
        .filter(|view| view.pending && deadline_due(&view.question, now))
        .map(|view| view.question.key)
        .collect();
    let mut expired = Vec::new();
    for key in due {
        let applied = transact(
            ctx,
            &key,
            None,
            &Actor::Owner,
            QuestionActor::System,
            now,
            |_, _, current, at| {
                if !deadline_due(current, now) {
                    return Ok(None);
                }
                let instruction = current
                    .deadline
                    .as_ref()
                    .map(|d| d.instruction.clone())
                    .unwrap_or_default();
                let mut question = current.clone();
                transition(
                    &mut question,
                    QuestionState::Expired,
                    "expire",
                    QuestionActor::System,
                    None,
                    at,
                );
                let answer = QuestionAnswer {
                    by: AnswerBy::OwnerDeadline,
                    text: instruction,
                    option: None,
                    basis_refs: Vec::new(),
                    scope_note: None,
                    at,
                    message_id: None,
                    superseded_by: None,
                };
                let body = answer_body(&question, &answer, "question_answer");
                question.answer = Some(answer);
                Ok(Some(Change {
                    question,
                    message: Some(("question_answer", body)),
                    needs_me: NeedsMe::Settle,
                }))
            },
        );
        sweep_one(applied, key, &mut expired)?;
    }
    super::reconcile_events(&ctx.store, task_id)?;
    Ok(expired)
}

fn last_turn_awaiting(task: &Task, container: &ContainerRef) -> bool {
    agent::container_state(task, container)
        .and_then(|state| state.turns.iter().max_by_key(|t| (t.generation, t.seq)))
        .is_some_and(|turn| turn.class == Some(TurnClass::AwaitingAnswer))
}

fn unattended_due(task: &Task, question: &AgentQuestion, now: Timestamp, secs: u64) -> bool {
    let Ok(parsed) = QuestionKey::parse(&question.key) else {
        return false;
    };
    let since = question
        .triage
        .iter()
        .map(|t| t.at)
        .max()
        .map_or(question.asked.at, |at| at.max(question.asked.at));
    let waited = now.as_second().saturating_sub(since.as_second());
    question.recorded_pending()
        && question.state == QuestionState::Open
        && !super::unattended_marked(question)
        && (question.blocking || last_turn_awaiting(task, &parsed.container))
        && u64::try_from(waited).is_ok_and(|waited| waited >= secs)
}

/// The unattended rule: a blocking question, or one whose asking container's last turn ended
/// `awaiting_answer`, with no triage for `[agent] question_unattended_secs` is marked once per
/// revision and raises Needs me `question_unattended`.
pub fn unattended(ctx: &Ctx, task_id: &str, now: Timestamp) -> Result<Vec<String>> {
    let secs = ctx.store.config.agent.question_unattended_secs;
    let task = ctx.store.get(task_id)?;
    let due: Vec<String> = views(&task)
        .into_iter()
        .filter(|view| view.pending && unattended_due(&task, &view.question, now, secs))
        .map(|view| view.question.key)
        .collect();
    let mut marked = Vec::new();
    for key in due {
        let applied = transact(
            ctx,
            &key,
            None,
            &Actor::Owner,
            QuestionActor::System,
            now,
            |_, task, current, at| {
                if !unattended_due(task, current, now, secs) {
                    return Ok(None);
                }
                let mut question = current.clone();
                question.unattended_at = Some(at);
                triage(&mut question, "unattended", QuestionActor::System, None, at);
                Ok(Some(Change {
                    question,
                    message: None,
                    needs_me: NeedsMe::Raise("question_unattended"),
                }))
            },
        );
        sweep_one(applied, key, &mut marked)?;
    }
    super::reconcile_events(&ctx.store, task_id)?;
    Ok(marked)
}
