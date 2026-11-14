//! Host-only `owner_ask`: canonical PM/assistant `ask_user` mirror. `open`, `answer` and
//! `withdraw` are receipted; `get` is an authenticated observation with no retained receipt.
//! Pane dialogs and app devices CAS the same revision. Remote hosts only poll; a bridge
//! answer is never an owner input or permission to execute an owner action.
use anyhow::{Context, Result};
use corral_core::agent::{
    AgentQuestion, AgentRole, ContainerRef, QuestionActor, QuestionKind, QuestionState,
    QuestionWakeRoute, ReceiptEffect, pm_state,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::Ctx;
use crate::agent::admission::{self, Admission, Mode};
use crate::agent::call::{CallResponse, RejectCode, Rejection};
use crate::agent::envelope::Envelope;
use crate::agent::receipts::{self, Commit};
use crate::questions::{self, AnswerInput, AskPayload};

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    Open {
        text: String,
        #[serde(default)]
        options: Vec<String>,
        #[serde(default)]
        recommendation: Option<String>,
        #[serde(default)]
        kind: Option<QuestionKind>,
        #[serde(default)]
        blocking: bool,
        #[serde(default)]
        option_details: Vec<corral_core::agent::QuestionOption>,
        #[serde(default)]
        evidence: Vec<corral_core::agent::QuestionEvidenceRef>,
        #[serde(default)]
        owner_decisions: Vec<String>,
    },
    Get {
        key: String,
    },
    Answer {
        key: String,
        revision: u32,
        text: String,
        #[serde(default)]
        option: Option<String>,
    },
    Withdraw {
        key: String,
        revision: u32,
    },
}

fn summary(question: &AgentQuestion) -> Value {
    json!({"key": question.key, "revision": question.revision, "state": question.state})
}

fn own_key(container: &ContainerRef, key: &str) -> Result<()> {
    if corral_core::agent::owner_question::owner_question_container(key).as_ref() != Some(container)
    {
        return Err(Rejection::new(
            RejectCode::OutOfScope,
            "owner ask belongs to another container",
        )
        .into());
    }
    Ok(())
}

fn snapshot(ctx: &Ctx, envelope: &Envelope, key: &str) -> Result<Value> {
    let container = admission::container_of(&envelope.identity);
    own_key(&container, key)?;
    let view = |state: &corral_core::agent::AgentRunState| -> Result<Value> {
        let question = state
            .questions
            .iter()
            .find(|q| q.key == key)
            .ok_or_else(|| Rejection::new(RejectCode::QuestionNotFound, "owner ask not found"))?;
        let mut result = summary(question);
        result["question"] = serde_json::to_value(question)?;
        Ok(result)
    };
    match &container {
        ContainerRef::Pm => pm_state::update_selected(
            &ctx.store,
            &pm_state::Selection::current().key("questions", format!("key:{key}")),
            |pm| {
                let pm = pm.as_ref().context("PM container disappeared")?;
                admission::recheck_pm(pm, envelope.identity.generation)?;
                Ok((view(&pm.state)?, false))
            },
        ),
        ContainerRef::Assistant { session } => {
            ctx.store
                .inspect_assistant_container(session, |assistant, ended| {
                    admission::recheck_assistant(assistant, ended, envelope.identity.generation)?;
                    view(&assistant.state)
                })
        }
        _ => Err(Rejection::new(
            RejectCode::OpNotAllowed,
            "only PM/assistant hosts own owner asks",
        )
        .into()),
    }
}

pub(crate) fn call(ctx: &Ctx, envelope: &Envelope) -> Result<CallResponse> {
    if !matches!(envelope.identity.role, AgentRole::Pm | AgentRole::Assistant) {
        return Err(Rejection::new(
            RejectCode::OpNotAllowed,
            "only PM/assistant hosts mirror ask_user",
        )
        .into());
    }
    let action: Action = super::payload(
        envelope,
        "action (open, get, answer, withdraw) and action fields",
    )?;
    let lookup = matches!(action, Action::Get { .. });
    let container = admission::container_of(&envelope.identity);
    let admitted = match admission::admit(
        ctx,
        envelope,
        if lookup {
            Mode::Lookup
        } else {
            Mode::Mutation { settles_turn: None }
        },
    )? {
        Admission::Replay(stored) => return receipts::respond(ctx, &container, *stored, true),
        Admission::New(admitted) => admitted,
    };
    if let Action::Get { key } = &action {
        let result = snapshot(ctx, envelope, key)?;
        let at = ctx.store.now();
        return Ok(CallResponse::Unstored {
            envelope: json!({
                "version": 1, "request_id": envelope.request_id, "op": envelope.op,
                "payload_sha256": envelope.payload_sha256, "identity": envelope.identity,
                "state": "committed", "result": result, "at": at, "committed_at": at,
                "evidence": [], "post_pending": []
            }),
        });
    }
    let selection = receipts::pm_selection(&admitted.origin).family("questions");
    receipts::commit_selected(ctx, &admitted, &selection, |state, _| {
        let (key, event) = match action {
            Action::Open {
                text,
                options,
                recommendation,
                kind,
                blocking,
                option_details,
                evidence,
                owner_decisions,
            } => {
                let payload = AskPayload {
                    text,
                    options,
                    recommendation,
                    kind: kind.unwrap_or(QuestionKind::Clarification),
                    blocking,
                    context_refs: Vec::new(),
                    needed_by: None,
                    option_details,
                    evidence,
                    owner_decisions,
                };
                let opened = questions::open(
                    state,
                    &container,
                    &admitted.origin,
                    payload,
                    QuestionWakeRoute::None,
                    ctx.store.now(),
                )?;
                (opened.key, "question_opened")
            }
            Action::Answer {
                key,
                revision,
                text,
                option,
            } => {
                own_key(&container, &key)?;
                if state
                    .generation(envelope.identity.generation)
                    .is_some_and(|g| g.daemon_hosted)
                {
                    return Err(Rejection::new(RejectCode::OpNotAllowed,
                        "daemon hosts observe canonical owner answers; they cannot submit dialog answers").into());
                }
                let question = state
                    .questions
                    .iter_mut()
                    .find(|q| q.key == key)
                    .ok_or_else(|| {
                        Rejection::new(RejectCode::QuestionNotFound, "owner ask not found")
                    })?;
                questions::owner::answer_record(
                    ctx,
                    question,
                    revision,
                    &AnswerInput {
                        text,
                        option,
                        ..Default::default()
                    },
                )?;
                (key, "question_answered")
            }
            Action::Withdraw { key, revision } => {
                own_key(&container, &key)?;
                let question = state
                    .questions
                    .iter_mut()
                    .find(|q| q.key == key)
                    .ok_or_else(|| {
                        Rejection::new(RejectCode::QuestionNotFound, "owner ask not found")
                    })?;
                questions::owner::check(question, revision)?;
                questions::transition(
                    question,
                    QuestionState::Withdrawn,
                    "withdraw",
                    QuestionActor::Agent,
                    Some("asking host withdrew ask_user".into()),
                    ctx.store.now(),
                );
                (key, "question_withdrawn")
            }
            Action::Get { .. } => {
                return Err(Rejection::new(
                    RejectCode::BadRequest,
                    "owner ask observation cannot enter a mutation",
                )
                .into());
            }
        };
        let question = state
            .questions
            .iter_mut()
            .find(|q| q.key == key)
            .context("owner ask disappeared")?;
        questions::bound_triage(&ctx.store, question)?;
        let (at, actor) = if event == "question_opened" {
            (question.asked.at, envelope.identity.role.as_str())
        } else {
            (
                ctx.store.now(),
                if event == "question_answered" {
                    "owner"
                } else {
                    "agent"
                },
            )
        };
        ctx.store.log_event(
            None,
            event,
            json!({
                "key": key, "revision": question.revision, "at": at.to_string(),
                "container": container.key(), "actor": actor,
            }),
        )?;
        Ok(Commit {
            result: summary(question),
            effect: Some(ReceiptEffect {
                kind: "owner_ask".into(),
                reference: key,
            }),
            post_pending: Vec::new(),
        })
    })
}
