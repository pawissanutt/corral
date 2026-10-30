//! Automatic deadline/unattended transactions: real keyed questions and metadata, no PM guard.
use super::{Applied, Change, NeedsMe, Outcome, QuestionKey, RefusalKind, refuse};
use crate::Ctx;
use anyhow::Result;
use corral_core::agent::{self, AgentQuestion, ContainerRef, NativeRecord, QuestionActor};
use corral_core::inbox::{Inbox, Message, MessageBinding, Post, Urgency};
use corral_core::{TaskHeadMut, TaskRef};
use jiff::Timestamp;
use serde_json::json;

pub(in crate::questions) fn transact_system(
    ctx: &Ctx,
    key: &str,
    revision: Option<u32>,
    by: QuestionActor,
    now: Timestamp,
    plan: impl FnOnce(&Ctx, &TaskHeadMut, &AgentQuestion, Timestamp) -> Result<Option<Change>>,
) -> Result<Option<Applied>> {
    let parsed = QuestionKey::parse(key)?;
    let container = parsed.container.clone();
    ctx.store.head(parsed.task())?; // IO failures are not a benign vanished-question refusal.
    let inbox = super::super::recipient(&container)
        .map(|recipient| Inbox::new(&ctx.store, recipient))
        .transpose()?;
    let _posting = inbox.as_ref().map(Inbox::lock).transpose()?;
    let mut applied = None;
    let mut posted: Option<Message> = None;
    let mut raised = None;
    let mut notice = None;
    ctx.store.try_update_head(parsed.task(), |task| {
        let current = task
            .agent_question(&container, key)?
            .ok_or_else(|| refuse(RefusalKind::NotFound, format!("question {key} not found")))?;
        super::refuse_closed_reason(agent::container_closure_ref(task, &container)?, &current)?;
        if let Some(revision) = revision
            && revision != current.revision
        {
            return Err(refuse(
                RefusalKind::StaleRevision,
                format!("{key} is at revision {}, not {revision}", current.revision),
            ));
        }
        let Some(change) = plan(ctx, task, &current, now)? else {
            return Ok(false);
        };
        let mut question = change.question;
        super::super::bound_triage(&ctx.store, &mut question)?;
        let message = match (&inbox, change.message) {
            (Some(inbox), Some((kind, body))) => {
                let run = match &container {
                    ContainerRef::Worker { run, .. } | ContainerRef::Planner { run, .. } => {
                        Some(run.clone())
                    }
                    ContainerRef::Reviewer { review, .. }
                    | ContainerRef::PlanReviewer { review, .. } => Some(review.clone()),
                    ContainerRef::Pm => None,
                };
                let message = inbox.reserve_in_head(
                    task,
                    Post {
                        kind,
                        urgency: Urgency::Wake,
                        author: super::by_name(by),
                        body: &body,
                        binding: MessageBinding {
                            run,
                            ..MessageBinding::default()
                        },
                        subject: Some(json!({"key":key,"revision":question.revision})),
                        max_payload: corral_core::inbox::MAX_PAYLOAD,
                    },
                )?;
                if let Some(answer) = question.answer.as_mut() {
                    answer.message_id = Some(message.id.clone());
                }
                Some(message)
            }
            _ => None,
        };
        let outcome = Outcome {
            key: key.into(),
            revision: question.revision,
            state: question.state,
            message_id: message.as_ref().map(|message| message.id.clone()),
            delivery: if message.is_some() { "queued" } else { "none" },
        };
        task.put_agent_record(&container, NativeRecord::Question(question))?;
        if super::apply_needs_me_ref(task, &container, &change.needs_me)?
            && let NeedsMe::Raise(reason) = change.needs_me
        {
            raised = Some(reason);
            notice = Some(super::record_entry_notice_ref(ctx, task, now)?);
        }
        posted = message;
        applied = Some(Applied::Fresh {
            outcome,
            receipt: None,
        });
        Ok(true)
    })?;
    if let (Some(message), Some(inbox)) = (&posted, &inbox) {
        inbox.announce(message)?;
    }
    if applied.is_some() {
        super::super::events::reconcile_question(&ctx.store, key)?;
    }
    if let (Some(reason), Some(Applied::Fresh { outcome, .. })) = (raised, &applied) {
        crate::obligations::transition_with_notice(
            ctx,
            parsed.task(),
            "needs_me",
            json!({"run":super::container_run(&container),"question":key,"revision":outcome.revision,"reason":reason}),
            notice,
        )?;
    }
    Ok(applied)
}
