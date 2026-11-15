//! PM/assistant-authored owner asks. These live on their actual host container, not on an
//! unrelated worker task. All app answers and pane dialog answers use this same record/CAS.
use anyhow::{Context, Result};
use corral_core::agent::ANSWER_TEXT_MAX_BYTES;
use corral_core::agent::{
    AgentQuestion, AgentRunState, AnswerBy, ClosureReason, ContainerRef, QuestionActor,
    QuestionAnswer, QuestionState, owner_question::owner_question_container, pm_state,
};

use super::{
    Actor, AnswerInput, Applied, Outcome, QuestionView, RefusalKind, bounded, refuse, transition,
};
use crate::Ctx;

fn container(key: &str) -> Result<ContainerRef> {
    owner_question_container(key)
        .ok_or_else(|| refuse(RefusalKind::NotFound, "not an owner ask key"))
}

pub(super) fn find<'a>(state: &'a AgentRunState, key: &str) -> Result<&'a AgentQuestion> {
    state
        .questions
        .iter()
        .find(|q| q.key == key)
        .ok_or_else(|| refuse(RefusalKind::NotFound, format!("question {key} not found")))
}

pub(crate) fn check(question: &AgentQuestion, revision: u32) -> Result<()> {
    if question.revision != revision {
        return Err(refuse(
            RefusalKind::StaleRevision,
            format!(
                "{} is at revision {}, not {revision}",
                question.key, question.revision
            ),
        ));
    }
    if !question.recorded_pending() {
        return Err(refuse(RefusalKind::State, "owner ask is already settled"));
    }
    Ok(())
}

pub(crate) fn answer_record(
    ctx: &Ctx,
    question: &mut AgentQuestion,
    revision: u32,
    input: &AnswerInput,
) -> Result<()> {
    check(question, revision)?;
    bounded(&input.text, ANSWER_TEXT_MAX_BYTES, "answer text")?;
    if input
        .option
        .as_ref()
        .is_some_and(|option| !question.options.contains(option))
    {
        return Err(refuse(
            RefusalKind::Bounds,
            "answer option is not a declared option",
        ));
    }
    let now = ctx.store.now();
    transition(
        question,
        QuestionState::Answered,
        "answer",
        QuestionActor::Owner,
        None,
        now,
    );
    question.answer = Some(QuestionAnswer {
        by: AnswerBy::Owner,
        text: input.text.clone(),
        option: input.option.clone(),
        basis_refs: Vec::new(),
        scope_note: None,
        at: now,
        message_id: None,
        superseded_by: None,
        attribution: Some(super::followup::attribution(ctx)?),
    });
    Ok(())
}

fn mutate(
    ctx: &Ctx,
    key: &str,
    revision: u32,
    apply: impl FnOnce(&mut AgentQuestion) -> Result<()>,
) -> Result<Applied> {
    let container = container(key)?;
    let edit = |state: &mut AgentRunState| -> Result<Outcome> {
        let question = state
            .questions
            .iter_mut()
            .find(|q| q.key == key)
            .ok_or_else(|| refuse(RefusalKind::NotFound, "owner ask not found"))?;
        check(question, revision)?;
        apply(question)?;
        super::bound_triage(&ctx.store, question)?;
        let entry = question
            .triage
            .last()
            .context("question mutation missing triage")?;
        let kind = match entry.action.as_str() {
            "answer" => "question_answered",
            "followup" => "question_followup",
            "cancel" => "question_cancelled",
            _ => "question_changed",
        };
        ctx.store.log_event(
            None,
            kind,
            serde_json::json!({
                "key": key, "revision": question.revision, "at": entry.at.to_string(),
                "by": "owner", "container": container.key(),
            }),
        )?;
        Ok(Outcome {
            key: key.into(),
            revision: question.revision,
            state: question.state,
            message_id: None,
            delivery: "none",
        })
    };
    let outcome = match &container {
        ContainerRef::Pm => pm_state::update_selected(
            &ctx.store,
            &pm_state::Selection::current().key("questions", format!("key:{key}")),
            |pm| {
                let pm = pm
                    .as_mut()
                    .ok_or_else(|| refuse(RefusalKind::Closed, "PM container missing"))?;
                let outcome = edit(&mut pm.state)?;
                Ok((outcome, true))
            },
        )?,
        ContainerRef::Assistant { session } => {
            ctx.store
                .settle_assistant_container(session, |assistant, ended| {
                    if ended {
                        return Err(refuse(RefusalKind::Closed, "assistant session ended"));
                    }
                    edit(&mut assistant.state)
                })?
        }
        _ => {
            return Err(refuse(
                RefusalKind::OutOfScope,
                "not an owner ask container",
            ));
        }
    };
    Ok(Applied::Fresh {
        outcome,
        receipt: None,
    })
}

pub(super) fn answer(
    ctx: &Ctx,
    key: &str,
    revision: u32,
    actor: Actor<'_>,
    input: AnswerInput,
) -> Result<Applied> {
    if !matches!(actor, Actor::Owner) {
        return Err(refuse(
            RefusalKind::Rule,
            "only the owner answers a host-authored owner ask",
        ));
    }
    let applied = mutate(ctx, key, revision, |q| {
        answer_record(ctx, q, revision, &input)
    })?;
    if let Some(recipient) = super::recipient(&container(key)?) {
        crate::session_control::pause::lift_recipient_owner(ctx, &recipient)?;
    }
    Ok(applied)
}

pub(super) fn followup(ctx: &Ctx, key: &str, revision: u32, text: &str) -> Result<Applied> {
    let attribution = super::followup::attribution(ctx)?;
    mutate(ctx, key, revision, |q| {
        super::followup::append(q, text, attribution, ctx.store.now())
    })
}

pub(super) fn cancel(
    ctx: &Ctx,
    key: &str,
    revision: u32,
    actor: Actor<'_>,
    reason: &str,
) -> Result<Applied> {
    if !matches!(actor, Actor::Owner) {
        return Err(refuse(
            RefusalKind::Rule,
            "only the owner cancels a host-authored owner ask",
        ));
    }
    bounded(reason, super::NOTE_MAX_BYTES, "reason")?;
    mutate(ctx, key, revision, |q| {
        transition(
            q,
            QuestionState::Cancelled,
            "cancel",
            QuestionActor::Owner,
            Some(reason.into()),
            ctx.store.now(),
        );
        Ok(())
    })
}

pub(super) fn show(ctx: &Ctx, key: &str) -> Result<QuestionView> {
    let container = container(key)?;
    let (question, closed) = match &container {
        ContainerRef::Pm => {
            let pm = pm_state::load_selected(
                &ctx.store,
                &pm_state::Selection::default().key("questions", format!("key:{key}")),
            )?
            .context("PM container not found")?;
            (find(&pm.state, key)?.clone(), false)
        }
        ContainerRef::Assistant { session } => {
            let ended = ctx.store.get_assistant(session)?.ended.is_some();
            let state = ctx.store.get_assistant_container(session)?;
            (find(&state.state, key)?.clone(), ended)
        }
        _ => {
            return Err(refuse(
                RefusalKind::OutOfScope,
                "not an owner ask container",
            ));
        }
    };
    Ok(super::question_view(
        "",
        &container,
        closed.then_some(ClosureReason::RunEnded),
        question,
    ))
}

pub(super) fn list(ctx: &Ctx, all: bool) -> Result<Vec<QuestionView>> {
    let mut rows = Vec::new();
    let selection = if all {
        pm_state::Selection::default().family("questions")
    } else {
        pm_state::Selection::work()
    };
    if let Some(pm) = pm_state::load_selected(&ctx.store, &selection)? {
        for question in pm.state.questions {
            if all || question.recorded_pending() {
                rows.push(super::question_view("", &ContainerRef::Pm, None, question));
            }
        }
    }
    for session in ctx.store.list_assistants()? {
        if !all && session.ended.is_some() {
            continue;
        }
        let state = ctx.store.get_assistant_container(&session.id)?;
        let container = ContainerRef::Assistant {
            session: session.id,
        };
        for question in state.state.questions {
            if all || question.recorded_pending() {
                rows.push(super::question_view(
                    "",
                    &container,
                    session.ended.map(|_| ClosureReason::RunEnded),
                    question,
                ));
            }
        }
    }
    Ok(rows)
}
