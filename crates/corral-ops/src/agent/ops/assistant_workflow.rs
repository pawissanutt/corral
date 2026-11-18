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
    /// Host-only, read-only: the exact current-revision continuation of this session's
    /// board-owned scope for one goal. Grants nothing; Start consumes it only once bound.
    ContinuePreview {
        goal: workflow::handoff::Goal,
    },
    /// Host-only, read-only: the exact context/scope a local choice would package.
    LocalPreview {
        local: Box<workflow::handoff::native_local::LocalChoice>,
    },
    /// Host-only, read-only: the binding a native selection must be journaled against.
    Binding {
        native: Box<workflow::handoff::native::NativeAction>,
        /// The `shown` digest of the read the sheet displayed.
        shown: String,
    },
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
            let action: HandoffAction = super::payload(
                envelope,
                "action, expected?, question?, question_revision?, native?, shown?, goal?, local?",
            )?;
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
                HandoffAction::Read {} => return handoff_read(ctx, envelope),
                HandoffAction::ContinuePreview { goal } => {
                    return handoff_continue_preview(ctx, envelope, goal);
                }
                HandoffAction::LocalPreview { local } => {
                    let origin = lookup(ctx, envelope)?;
                    let session = envelope
                        .identity
                        .assistant
                        .as_deref()
                        .context("assistant identity missing")?;
                    ctx.store
                        .inspect_assistant_container(session, |container, ended| {
                            admission::recheck_assistant(
                                container,
                                ended,
                                envelope.identity.generation,
                            )
                        })?;
                    let preview = workflow::handoff::native_local::preview(ctx, session, &local)?;
                    return Ok(unstored(&origin, preview));
                }
                HandoffAction::Binding { native, shown } => {
                    return handoff_binding(ctx, envelope, &native, &shown);
                }
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

/// Read-only shared responsibility projection; grants no transfer or execution consent.
fn handoff_read(ctx: &Ctx, envelope: &Envelope) -> Result<CallResponse> {
    let origin = lookup(ctx, envelope)?;
    let session = envelope
        .identity
        .assistant
        .as_deref()
        .context("assistant identity missing")?;
    // The local route's project: the session's own, else its single scratch project.
    let record = ctx.store.get_assistant(session)?;
    let mut projects: Vec<_> = record.scratch.iter().map(|s| s.project.clone()).collect();
    projects.dedup();
    let project = record
        .project
        .clone()
        .or_else(|| (projects.len() == 1).then(|| projects[0].clone()));
    let shown = |ctx: &Ctx| -> Result<String> {
        let c = ctx.store.get_assistant_container(session)?;
        Ok(workflow::handoff::native_view::view(ctx, session, &c)?.digest)
    };
    let before = shown(ctx)?;
    let state = ctx
        .store
        .inspect_assistant_container(session, |container, ended| {
            admission::recheck_assistant(container, ended, envelope.identity.generation)?;
            let local =
                workflow::handoff::native_local::offer(&ctx.store, container, project.clone())
                    .unwrap_or_else(|e| serde_json::json!({"unavailable": format!("{e:#}")}));
            // Implement here: only a ready local plan with a declared scope, while planning.
            let implement = workflow::handoff::native_implement::offer(&ctx.store, container)
                .unwrap_or_else(|e| serde_json::json!({"unavailable": format!("{e:#}")}));
            // Retained planning/local-review facts from the same canonical reads the
            // review tool uses: exact reviewed vs current revision, never inferred.
            let planning = match &container.workflow {
                Some(w) => serde_json::json!({
                    "plan_revision": w.current.current_plan.as_ref().map(|p| p.revision),
                    "plan_stage": w.current.current_plan.as_ref().map(|p| p.stage),
                    "reviews": workflow::review::read(&ctx.store, session, container, None, 0, 0)
                        .map(|r| r["reviews"].clone())
                        .unwrap_or_else(|e| serde_json::json!({"unavailable": format!("{e:#}")})),
                }),
                None => serde_json::Value::Null,
            };
            Ok((
                container
                    .workflow
                    .as_ref()
                    .map(|w| (w.current.owner.clone(), w.current.phase)),
                planning,
                local,
                implement,
            ))
        })?;
    let mut result = workflow::handoff::status::for_assistant(ctx, state.0)?;
    result["planning"] = state.1;
    result["local"] = state.2;
    result["implement"] = state.3;
    // Every native selection binds this digest; a change during the read is retried, not shown.
    anyhow::ensure!(
        shown(ctx)? == before,
        "handoff_changed_while_reading: retry /handoff"
    );
    result["shown"] = serde_json::json!(before);
    Ok(unstored(&origin, result))
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

/// Read-only continuation preview for this session's own board-owned scope only.
fn handoff_continue_preview(
    ctx: &Ctx,
    envelope: &Envelope,
    goal: workflow::handoff::Goal,
) -> Result<CallResponse> {
    let origin = lookup(ctx, envelope)?;
    let session = envelope
        .identity
        .assistant
        .as_deref()
        .context("assistant identity missing")?;
    let task = ctx
        .store
        .inspect_assistant_container(session, |container, ended| {
            admission::recheck_assistant(container, ended, envelope.identity.generation)?;
            match &workflow::current(container)?.owner {
                corral_core::assistant_workflow::ScopeOwner::Board { task } => Ok(task.clone()),
                _ => anyhow::bail!("handoff_owner: this scope is not board-owned"),
            }
        })?;
    let choice = workflow::handoff::continuation::preview(ctx, &task, goal)?;
    Ok(unstored(&origin, serde_json::json!({"choice": choice})))
}

/// Read-only binding for the host's native sheet; journaling and consumption are separate.
fn handoff_binding(
    ctx: &Ctx,
    envelope: &Envelope,
    native: &workflow::handoff::native::NativeAction,
    shown: &str,
) -> Result<CallResponse> {
    let origin = lookup(ctx, envelope)?;
    let session = envelope
        .identity
        .assistant
        .as_deref()
        .context("assistant identity missing")?;
    let container = ctx.store.get_assistant_container(session)?;
    anyhow::ensure!(
        workflow::handoff::native_view::view(ctx, session, &container)?.digest == shown,
        workflow::handoff::native_view::STALE
    );
    let result = ctx
        .store
        .inspect_assistant_container(session, |container, ended| {
            admission::recheck_assistant(container, ended, envelope.identity.generation)?;
            let (binding, digest) = workflow::handoff::native::binding(container, session, native, shown)?;
            Ok(serde_json::json!({"binding": binding, "digest": digest, "option": native.option()?}))
        })?;
    Ok(unstored(&origin, result))
}

/// Host-only native handoff consumer. The cited input must be a host-journaled Choice
/// bound to this exact action; it is consumed once before the shared C3 service runs.
/// A lost acknowledgement replays the stored receipt; a retry after an unrecorded effect
/// refuses as already consumed rather than repeating it.
pub(crate) fn handoff_choice(ctx: &Ctx, envelope: &Envelope) -> Result<CallResponse> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Payload {
        kind: String,
        input: String,
        native: workflow::handoff::native::NativeAction,
        shown: String,
    }
    let payload: Payload = super::payload(envelope, "kind, input, native, shown")?;
    if payload.kind != "handoff_choice"
        || crate::agent::envelope::event_id_parts(&envelope.request_id).is_none()
        || envelope.identity.assistant.is_none()
    {
        return Err(Rejection::new(
            RejectCode::BadRequest,
            "native handoff choice requires a host event",
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
    let evidence = ctx
        .store
        .inspect_assistant_container(session, |container, ended| {
            admission::recheck_assistant(container, ended, envelope.identity.generation)?;
            workflow::handoff::native::authenticate(
                container,
                session,
                envelope.identity.generation,
                &payload.input,
                &payload.native,
                &payload.shown,
            )
        })
        .map_err(|e| Rejection::new(RejectCode::BadRequest, format!("{e:#}")))?;
    let result = workflow::handoff::native::consume(ctx, evidence, payload.native, &payload.shown)
        .map_err(|e| Rejection::new(RejectCode::BadRequest, format!("{e:#}")))?;
    receipts::commit_assistant(ctx, &admitted, |_| {
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
