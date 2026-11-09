//! Closed assistant-only operations. Model actions cannot publish host lifecycle events or
//! acquire execution authority; local review remains an exact-snapshot advisory operation.
use anyhow::{Context, Result};
use corral_core::agent::{AGENT_RECORD_VERSION, ReceiptState, RequestOrigin, RequestReceipt};
use serde::Deserialize;
use serde_json::Value;

use crate::Ctx;
use crate::agent::admission::{self, Admission, Mode};
use crate::agent::call::{AgentOp, CallResponse, RejectCode, Rejection};
use crate::agent::envelope::Envelope;
use crate::agent::receipts::{self, Commit};
use crate::assistant::workflow::{self, Expected, Publish};

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum PlanAction {
    Read {
        #[serde(default)]
        after: u32,
        #[serde(default = "page_limit")]
        limit: u32,
    },
    ReadRevision {
        revision: u32,
        #[serde(default)]
        offset: usize,
        #[serde(default = "text_limit")]
        limit: usize,
    },
    Stage {
        expected: Expected,
        offset: usize,
        chunk: String,
    },
    Publish(Publish),
    Enter {
        input: String,
    },
    Checkpoint {
        expected: Expected,
    },
    BeginExecution {
        expected: Expected,
        question: String,
        question_revision: u32,
    },
    Pause {
        expected: Expected,
    },
    Candidate {
        expected: Expected,
    },
    Close {
        expected: Expected,
    },
}

use workflow::review::Action as ReviewAction;

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum HandoffAction {
    Preview {
        expected: Expected,
    },
    Commit {
        expected: Expected,
        question: String,
        question_revision: u32,
    },
    Read {},
    Reconcile {
        expected: Expected,
    },
    Cancel {
        expected: Expected,
    },
    ReturnPreview {
        expected: Expected,
    },
    Return {
        expected: Expected,
        question: String,
        question_revision: u32,
    },
}

fn page_limit() -> u32 {
    3
}
fn text_limit() -> usize {
    1024
}

/// Read-only live authentication; lookups consume no receipt slots.
fn lookup(ctx: &Ctx, envelope: &Envelope) -> Result<RequestOrigin> {
    let admitted = match admission::admit(ctx, envelope, Mode::Lookup)? {
        Admission::New(admitted) => admitted,
        Admission::Replay(_) => {
            return Err(Rejection::new(
                RejectCode::BadRequest,
                "read operations require a fresh request ID",
            )
            .into());
        }
    };
    Ok(admitted.origin.clone())
}

fn unstored(origin: &RequestOrigin, result: Value) -> CallResponse {
    let at = corral_core::clock::now();
    CallResponse::Receipt {
        receipt: RequestReceipt {
            version: AGENT_RECORD_VERSION,
            request_id: origin.request_id.clone(),
            op: origin.op.clone(),
            payload_sha256: origin.payload_sha256.clone(),
            identity: origin.identity.clone(),
            state: ReceiptState::Committed,
            result,
            executor: None,
            effect: None,
            evidence: Vec::new(),
            post_pending: Vec::new(),
            at,
            committed_at: Some(at),
        },
        replayed: false,
    }
}

fn read(ctx: &Ctx, envelope: &Envelope, action: PlanAction) -> Result<CallResponse> {
    let origin = lookup(ctx, envelope)?;
    let session = envelope
        .identity
        .assistant
        .as_deref()
        .context("assistant identity missing")?;
    let result = ctx
        .store
        .inspect_assistant_container(session, |container, ended| {
            admission::recheck_assistant(container, ended, envelope.identity.generation)?;
            match action {
                PlanAction::Read { after, limit } => {
                    workflow::read_state(&ctx.store, session, container, after, limit)
                }
                PlanAction::ReadRevision {
                    revision,
                    offset,
                    limit,
                } => {
                    workflow::read_revision(&ctx.store, session, container, revision, offset, limit)
                }
                _ => unreachable!("read dispatch is closed"),
            }
        })?;
    Ok(unstored(&origin, result))
}

fn unavailable(ctx: &Ctx, envelope: &Envelope, owner: &str) -> Result<CallResponse> {
    lookup(ctx, envelope)?;
    let session = envelope
        .identity
        .assistant
        .as_deref()
        .context("assistant identity missing")?;
    ctx.store
        .inspect_assistant_container(session, |container, ended| {
            admission::recheck_assistant(container, ended, envelope.identity.generation)
        })?;
    Err(Rejection::new(
        RejectCode::OpUnavailable,
        format!("assistant lifecycle unavailable until {owner}; no transition was made"),
    )
    .into())
}

pub(crate) fn call(ctx: &Ctx, envelope: &Envelope, op: AgentOp) -> Result<CallResponse> {
    match op {
        AgentOp::AssistantPlan => plan(ctx, envelope),
        AgentOp::AssistantPlanReview => review(ctx, envelope),
        AgentOp::AssistantHandoff => {
            let action: HandoffAction =
                super::payload(envelope, "action, expected?, question?, question_revision?")?;
            match action {
                HandoffAction::Commit {
                    expected,
                    question,
                    question_revision,
                }
                | HandoffAction::Return {
                    expected,
                    question,
                    question_revision,
                } => {
                    let _ = (expected, question, question_revision);
                }
                HandoffAction::Preview { expected }
                | HandoffAction::Reconcile { expected }
                | HandoffAction::Cancel { expected }
                | HandoffAction::ReturnPreview { expected } => {
                    let _ = expected;
                }
                HandoffAction::Read {} => {}
            }
            unavailable(ctx, envelope, "assistant-handoff-package-and-ownership")
        }
        _ => Err(Rejection::new(
            RejectCode::OpNotAllowed,
            "not an assistant workflow operation",
        )
        .into()),
    }
}

fn review(ctx: &Ctx, envelope: &Envelope) -> Result<CallResponse> {
    let action: ReviewAction = super::payload(
        envelope,
        "action, expected?, input?, review?, offset?, pending_after?",
    )?;
    let session = envelope
        .identity
        .assistant
        .as_deref()
        .context("assistant identity missing")?;
    if let ReviewAction::Read {
        review,
        offset,
        pending_after,
    } = action
    {
        let origin = lookup(ctx, envelope)?;
        let result = ctx
            .store
            .inspect_assistant_container(session, |container, ended| {
                admission::recheck_assistant(container, ended, envelope.identity.generation)?;
                workflow::review::read(
                    &ctx.store,
                    session,
                    container,
                    review.as_deref(),
                    offset,
                    pending_after,
                )
            })?;
        return Ok(unstored(&origin, result));
    }
    let reference = admission::container_of(&envelope.identity);
    let admitted = match admission::admit(ctx, envelope, Mode::Mutation { settles_turn: None })? {
        Admission::Replay(stored) => return receipts::respond(ctx, &reference, *stored, true),
        Admission::New(admitted) => admitted,
    };
    receipts::commit_assistant(ctx, &admitted, |container| {
        let result = match action {
            ReviewAction::Start { expected, input } => {
                workflow::review::start(&ctx.store, session, container, &expected, &input)?
            }
            ReviewAction::Cancel { expected, review } => {
                workflow::review::cancel(&ctx.store, session, container, &expected, &review)?
            }
            ReviewAction::Read { .. } => unreachable!("closed read dispatch"),
        };
        Ok(Commit {
            result,
            effect: None,
            post_pending: Vec::new(),
        })
    })
}

/// Host-only lifecycle channel. ev IDs are authenticated just like owner-input provenance;
/// the model-callable assistant_plan schema cannot name these actions.
pub(crate) fn boundary(ctx: &Ctx, envelope: &Envelope) -> Result<CallResponse> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Payload {
        kind: String,
        transition: workflow::boundary::BoundaryAction,
    }
    let payload: Payload = super::payload(envelope, "kind, transition")?;
    if payload.kind != "assistant_boundary"
        || crate::agent::envelope::event_id_parts(&envelope.request_id).is_none()
        || envelope.identity.assistant.is_none()
    {
        return Err(Rejection::new(
            RejectCode::BadRequest,
            "assistant boundary requires a host event",
        )
        .into());
    }
    let reference = admission::container_of(&envelope.identity);
    let admitted = match admission::admit(ctx, envelope, Mode::Mutation { settles_turn: None })? {
        Admission::Replay(stored) => return receipts::respond(ctx, &reference, *stored, true),
        Admission::New(admitted) => admitted,
    };
    let session = envelope
        .identity
        .assistant
        .as_deref()
        .context("assistant identity missing")?;
    receipts::commit_assistant(ctx, &admitted, |container| {
        let result = workflow::boundary::apply(
            &ctx.store,
            session,
            container,
            envelope.identity.generation,
            payload.transition,
        )?;
        Ok(Commit {
            result,
            effect: None,
            post_pending: Vec::new(),
        })
    })
}

fn plan(ctx: &Ctx, envelope: &Envelope) -> Result<CallResponse> {
    let action: PlanAction = super::payload(
        envelope,
        "action, after?, limit?, revision?, offset?, expected?, chunk?, bytes?, digest?, stage?, execution_scope?, assumptions?, scope_changes?, input?, question?, question_revision?",
    )?;
    match action {
        action @ (PlanAction::Read { .. } | PlanAction::ReadRevision { .. }) => {
            read(ctx, envelope, action)
        }
        action @ (PlanAction::Stage { .. } | PlanAction::Publish(_)) => {
            let container = admission::container_of(&envelope.identity);
            let admitted =
                match admission::admit(ctx, envelope, Mode::Mutation { settles_turn: None })? {
                    Admission::Replay(stored) => {
                        return receipts::respond(ctx, &container, *stored, true);
                    }
                    Admission::New(admitted) => admitted,
                };
            let session = envelope
                .identity
                .assistant
                .as_deref()
                .context("assistant identity missing")?;
            receipts::commit_assistant(ctx, &admitted, |container| {
                let result = match action {
                    PlanAction::Stage {
                        expected,
                        offset,
                        chunk,
                    } => {
                        workflow::stage(&ctx.store, session, container, &expected, offset, &chunk)?
                    }
                    PlanAction::Publish(payload) => {
                        workflow::publish(&ctx.store, session, container, payload)?
                    }
                    _ => unreachable!("mutation dispatch is closed"),
                };
                Ok(Commit {
                    result,
                    effect: None,
                    post_pending: Vec::new(),
                })
            })
        }
        PlanAction::Enter { input } => {
            let _ = input;
            unavailable(ctx, envelope, "assistant-planning-execution-boundary")
        }
        PlanAction::BeginExecution {
            expected,
            question,
            question_revision,
        } => {
            let _ = (expected, question, question_revision);
            unavailable(
                ctx,
                envelope,
                "assistant-planning-execution-boundary and authenticated choices",
            )
        }
        PlanAction::Checkpoint { expected }
        | PlanAction::Pause { expected }
        | PlanAction::Candidate { expected }
        | PlanAction::Close { expected } => {
            let _ = expected;
            unavailable(
                ctx,
                envelope,
                "assistant-planning-execution-boundary and choice experience",
            )
        }
    }
}
