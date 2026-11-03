//! Closed assistant-only operations. Only read/stage/publish are implemented in this member;
//! lifecycle transitions explicitly refuse until the corresponding policy/service is present.
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

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum ReviewAction {
    Start { expected: Expected, input: String },
    Read { review: Option<String> },
    Cancel { expected: Expected, review: String },
}

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
        AgentOp::AssistantPlanReview => {
            let action: ReviewAction =
                super::payload(envelope, "action, expected?, input?, review?")?;
            // Fields are deliberately parsed even for unavailable actions: unknown or
            // host-only lifecycle fields must never become an open-ended effect request.
            match action {
                ReviewAction::Start { expected, input } => {
                    let _ = (expected, input);
                }
                ReviewAction::Read { review } => {
                    let _ = review;
                }
                ReviewAction::Cancel { expected, review } => {
                    let _ = (expected, review);
                }
            }
            unavailable(ctx, envelope, "assistant-local-plan-review-lifecycle")
        }
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
