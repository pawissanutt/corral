//! `corral agent call --op <op>`: the enumerated v1 op list with its allowed roles and owning
//! child, rejection codes, the response shape and routing to the op handlers.
use anyhow::Result;
use corral_core::Task;
use corral_core::agent::{
    AGENT_RECORD_VERSION, AgentRefusal, AgentRole, ContainerRef, QuestionKind, ReceiptEffect,
    RefusalCode, RequestIdentity, RequestReceipt, pm_state,
};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::admission::{self, Admission, Mode};
use super::envelope::{Envelope, error_category};
use super::receipts::{self, Commit};
use crate::Ctx;
use crate::questions::{
    self, Actor, AnswerInput, Applied, PmRequest, QuestionRefusal, RefusalKind,
};

/// Every v1 op. A host never sends a command string; ops outside this list are bad requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentOp {
    Hello,
    Event,
    ReceiptLookup,
    Ask,
    Withdraw,
    Wait,
    Progress,
    Todo,
    Checkpoint,
    Evidence,
    Artifact,
    Gate,
    Done,
    Verdict,
    QuestionAnswer,
    QuestionEscalate,
    QuestionCancel,
    Reply,
    Comment,
    Capture,
    ReadBrief,
    ReadNext,
    ReadShow,
    ReadList,
    ReadSearch,
    ReadRoute,
    ReadArtifacts,
    ReadLessons,
    Job,
    JobMonitor,
}

const ALL_ROLES: &[AgentRole] = &[
    AgentRole::Worker,
    AgentRole::Reviewer,
    AgentRole::Planner,
    AgentRole::PlanReviewer,
    AgentRole::Pm,
];
const AGENTS: &[AgentRole] = &[AgentRole::Worker, AgentRole::Reviewer];
const WORKER: &[AgentRole] = &[AgentRole::Worker];
const REVIEWER: &[AgentRole] = &[AgentRole::Reviewer];
const PM: &[AgentRole] = &[AgentRole::Pm];
const TODO: &[AgentRole] = &[AgentRole::Worker, AgentRole::Pm];

impl AgentOp {
    pub const ALL: [AgentOp; 30] = [
        AgentOp::Hello,
        AgentOp::Event,
        AgentOp::ReceiptLookup,
        AgentOp::Ask,
        AgentOp::Withdraw,
        AgentOp::Wait,
        AgentOp::Progress,
        AgentOp::Todo,
        AgentOp::Checkpoint,
        AgentOp::Evidence,
        AgentOp::Artifact,
        AgentOp::Gate,
        AgentOp::Done,
        AgentOp::Verdict,
        AgentOp::QuestionAnswer,
        AgentOp::QuestionEscalate,
        AgentOp::QuestionCancel,
        AgentOp::Reply,
        AgentOp::Comment,
        AgentOp::Capture,
        AgentOp::ReadBrief,
        AgentOp::ReadNext,
        AgentOp::ReadShow,
        AgentOp::ReadList,
        AgentOp::ReadSearch,
        AgentOp::ReadRoute,
        AgentOp::ReadArtifacts,
        AgentOp::ReadLessons,
        AgentOp::Job,
        AgentOp::JobMonitor,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            AgentOp::Hello => "hello",
            AgentOp::Event => "event",
            AgentOp::ReceiptLookup => "receipt_lookup",
            AgentOp::Ask => "ask",
            AgentOp::Withdraw => "withdraw",
            AgentOp::Wait => "wait",
            AgentOp::Progress => "progress",
            AgentOp::Todo => "todo",
            AgentOp::Checkpoint => "checkpoint",
            AgentOp::Evidence => "evidence",
            AgentOp::Artifact => "artifact",
            AgentOp::Gate => "gate",
            AgentOp::Done => "done",
            AgentOp::Verdict => "verdict",
            AgentOp::QuestionAnswer => "question_answer",
            AgentOp::QuestionEscalate => "question_escalate",
            AgentOp::QuestionCancel => "question_cancel",
            AgentOp::Reply => "reply",
            AgentOp::Comment => "comment",
            AgentOp::Capture => "capture",
            AgentOp::ReadBrief => "read_brief",
            AgentOp::ReadNext => "read_next",
            AgentOp::ReadShow => "read_show",
            AgentOp::ReadList => "read_list",
            AgentOp::ReadSearch => "read_search",
            AgentOp::ReadRoute => "read_route",
            AgentOp::ReadArtifacts => "read_artifacts",
            AgentOp::ReadLessons => "read_lessons",
            AgentOp::Job => "job",
            AgentOp::JobMonitor => "job_monitor",
        }
    }

    pub fn parse(name: &str) -> Option<AgentOp> {
        AgentOp::ALL.into_iter().find(|op| op.as_str() == name)
    }

    /// The Rust authority on which role may call the op; role loadouts are a convenience.
    pub fn roles(self) -> &'static [AgentRole] {
        match self {
            AgentOp::Hello | AgentOp::Event | AgentOp::ReceiptLookup => ALL_ROLES,
            AgentOp::Todo => TODO,
            AgentOp::Ask | AgentOp::Withdraw | AgentOp::Evidence | AgentOp::Artifact => AGENTS,
            AgentOp::Wait | AgentOp::Progress | AgentOp::Checkpoint => WORKER,
            AgentOp::Gate | AgentOp::Done | AgentOp::Job | AgentOp::JobMonitor => WORKER,
            AgentOp::Verdict => REVIEWER,
            _ => PM,
        }
    }

    /// The T-252 child that lands the op's handler for this role; `None` once implemented.
    /// Each role module reports its own landed ops, so landing one never edits this table.
    pub fn owner(self, role: AgentRole) -> Option<&'static str> {
        use super::ops::{pm, reviewer, worker};
        let (owner, lands): (&'static str, fn(AgentOp) -> bool) = match self.route(role)? {
            Route::Worker => (worker::OWNER, worker::lands),
            Route::Reviewer => (reviewer::OWNER, reviewer::lands),
            Route::Pm => (pm::OWNER, pm::lands),
        };
        (!lands(self)).then_some(owner)
    }

    /// The role module that handles the op; `None` for the ops `call` handles itself.
    fn route(self, role: AgentRole) -> Option<Route> {
        match self {
            AgentOp::Hello | AgentOp::Event | AgentOp::ReceiptLookup => None,
            AgentOp::Todo | AgentOp::Job | AgentOp::JobMonitor => None,
            AgentOp::Ask | AgentOp::Withdraw => None,
            AgentOp::QuestionAnswer | AgentOp::QuestionEscalate | AgentOp::QuestionCancel => None,
            AgentOp::Evidence | AgentOp::Artifact if role == AgentRole::Reviewer => {
                Some(Route::Reviewer)
            }
            AgentOp::Verdict => Some(Route::Reviewer),
            AgentOp::Wait
            | AgentOp::Progress
            | AgentOp::Checkpoint
            | AgentOp::Evidence
            | AgentOp::Artifact
            | AgentOp::Gate
            | AgentOp::Done => Some(Route::Worker),
            AgentOp::Reply
            | AgentOp::Comment
            | AgentOp::Capture
            | AgentOp::ReadBrief
            | AgentOp::ReadNext
            | AgentOp::ReadShow
            | AgentOp::ReadList
            | AgentOp::ReadSearch
            | AgentOp::ReadRoute
            | AgentOp::ReadArtifacts
            | AgentOp::ReadLessons => Some(Route::Pm),
        }
    }

    /// What hello advertises: the ops this role may call that have landed.
    pub fn implemented_for(role: AgentRole) -> Vec<&'static str> {
        AgentOp::ALL
            .into_iter()
            .filter(|op| op.roles().contains(&role) && op.owner(role).is_none())
            .map(AgentOp::as_str)
            .collect()
    }
}

/// The role module an op routes to (integration-seams).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    Worker,
    Reviewer,
    Pm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RejectCode {
    BadRequest,
    OpUnavailable,
    OpNotAllowed,
    ContainerNotFound,
    ForeignStore,
    ForeignProject,
    AuthFailed,
    NonceConsumed,
    PayloadMismatch,
    AgentFenced,
    RunEnded,
    ReviewClosed,
    PaneMismatch,
    OutOfScope,
    ReceiptLimit,
    QuestionNotFound,
    QuestionClosed,
    QuestionWithdrawn,
    StaleRevision,
    QuestionState,
    QuestionRule,
    QuestionLimit,
}

impl RejectCode {
    pub fn as_str(self) -> &'static str {
        match self {
            RejectCode::BadRequest => "bad_request",
            RejectCode::OpUnavailable => "op_unavailable",
            RejectCode::OpNotAllowed => "op_not_allowed",
            RejectCode::ContainerNotFound => "container_not_found",
            RejectCode::ForeignStore => "foreign_store",
            RejectCode::ForeignProject => "foreign_project",
            RejectCode::AuthFailed => "auth_failed",
            RejectCode::NonceConsumed => "nonce_consumed",
            RejectCode::PayloadMismatch => "payload_mismatch",
            RejectCode::AgentFenced => "agent_fenced",
            RejectCode::RunEnded => "run_ended",
            RejectCode::ReviewClosed => "review_closed",
            RejectCode::PaneMismatch => "pane_mismatch",
            RejectCode::OutOfScope => "out_of_scope",
            RejectCode::ReceiptLimit => "receipt_limit",
            RejectCode::QuestionNotFound => "question_not_found",
            RejectCode::QuestionClosed => "question_closed",
            RejectCode::QuestionWithdrawn => "question_withdrawn",
            RejectCode::StaleRevision => "stale_revision",
            RejectCode::QuestionState => "question_state",
            RejectCode::QuestionRule => "question_rule",
            RejectCode::QuestionLimit => "question_limit",
        }
    }
}

/// A refusal before any effect. Reasons never contain secret bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejection {
    pub code: RejectCode,
    pub reason: String,
    pub owner: Option<&'static str>,
}

impl Rejection {
    pub fn new(code: RejectCode, reason: impl Into<String>) -> Self {
        Self {
            code,
            reason: reason.into(),
            owner: None,
        }
    }
}

impl From<AgentRefusal> for Rejection {
    fn from(refusal: AgentRefusal) -> Self {
        let code = match refusal.code {
            RefusalCode::AgentFenced => RejectCode::AgentFenced,
            RefusalCode::RunEnded => RejectCode::RunEnded,
            RefusalCode::ReviewClosed => RejectCode::ReviewClosed,
        };
        Rejection::new(code, refusal.detail)
    }
}

impl From<QuestionRefusal> for Rejection {
    fn from(refusal: QuestionRefusal) -> Self {
        let code = match refusal.kind {
            RefusalKind::NotFound => RejectCode::QuestionNotFound,
            RefusalKind::Closed => RejectCode::QuestionClosed,
            RefusalKind::Withdrawn => RejectCode::QuestionWithdrawn,
            RefusalKind::StaleRevision => RejectCode::StaleRevision,
            RefusalKind::State => RejectCode::QuestionState,
            RefusalKind::Rule => RejectCode::QuestionRule,
            RefusalKind::Limit => RejectCode::QuestionLimit,
            RefusalKind::Bounds => RejectCode::BadRequest,
            RefusalKind::OutOfScope => RejectCode::OutOfScope,
            RefusalKind::PayloadMismatch => RejectCode::PayloadMismatch,
        };
        Rejection::new(code, refusal.reason)
    }
}

impl std::fmt::Display for Rejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.reason)
    }
}

impl std::error::Error for Rejection {}

#[derive(Debug, Clone, PartialEq)]
pub enum CallResponse {
    /// Committed now or replayed from the container.
    Receipt {
        receipt: RequestReceipt,
        replayed: bool,
    },
    /// receipt_lookup: the found receipt exactly as stored, so its result keeps the v1 bound.
    /// Nothing is stored for the lookup itself.
    Lookup {
        request_id: String,
        identity: RequestIdentity,
        target: String,
        found: Option<Box<RequestReceipt>>,
    },
    /// Refused before any effect; nothing was stored.
    Rejected {
        rejection: Rejection,
        op: String,
        request_id: Option<String>,
        identity: Option<RequestIdentity>,
    },
}

impl CallResponse {
    pub fn receipt(&self) -> Option<&RequestReceipt> {
        match self {
            CallResponse::Receipt { receipt, .. } => Some(receipt),
            CallResponse::Lookup { .. } | CallResponse::Rejected { .. } => None,
        }
    }

    /// The receipt a lookup found.
    pub fn found(&self) -> Option<&RequestReceipt> {
        match self {
            CallResponse::Lookup { found, .. } => found.as_deref(),
            CallResponse::Receipt { .. } | CallResponse::Rejected { .. } => None,
        }
    }

    pub fn rejection(&self) -> Option<&Rejection> {
        match self {
            CallResponse::Rejected { rejection, .. } => Some(rejection),
            CallResponse::Receipt { .. } | CallResponse::Lookup { .. } => None,
        }
    }

    /// The single JSON object `corral agent call` prints.
    pub fn to_json(&self) -> Result<Value> {
        Ok(match self {
            CallResponse::Receipt { receipt, replayed } => {
                let mut value = serde_json::to_value(receipt)?;
                if *replayed {
                    value["replayed"] = json!(true);
                }
                value
            }
            CallResponse::Lookup {
                request_id,
                identity,
                target,
                found,
            } => {
                let lookup =
                    json!({"by": request_id, "request_id": target, "found": found.is_some()});
                match found {
                    Some(receipt) => {
                        let mut value = serde_json::to_value(receipt)?;
                        value["lookup"] = lookup;
                        value
                    }
                    None => json!({
                        "version": AGENT_RECORD_VERSION, "request_id": request_id,
                        "op": AgentOp::ReceiptLookup.as_str(), "identity": identity,
                        "state": "committed", "stored": false, "result": {"found": false},
                        "lookup": lookup, "at": jiff::Timestamp::now(),
                    }),
                }
            }
            CallResponse::Rejected {
                rejection,
                op,
                request_id,
                identity,
            } => {
                let mut result =
                    json!({"code": rejection.code.as_str(), "reason": rejection.reason});
                if let Some(owner) = rejection.owner {
                    result["owner"] = json!(owner);
                }
                json!({
                    "version": AGENT_RECORD_VERSION, "request_id": request_id, "op": op,
                    "identity": identity, "state": "rejected", "stored": false,
                    "result": result, "at": jiff::Timestamp::now(),
                })
            }
        })
    }
}

/// One host request: bounds, role and availability, then the op's own admission and commit.
/// Rejections are responses; `Err` is reserved for failures to read or write the store.
pub fn call(ctx: &Ctx, op_flag: &str, input: &[u8]) -> Result<CallResponse> {
    let (envelope, op) = match super::envelope::parse(op_flag, input) {
        Ok(parsed) => parsed,
        Err(refused) => {
            let (rejection, partial) = *refused;
            let response = CallResponse::Rejected {
                rejection,
                op: op_flag.into(),
                request_id: partial.request_id,
                identity: partial.identity,
            };
            super::receipts::log_rejection(ctx, &response, input)?;
            return Ok(response);
        }
    };
    let role = envelope.identity.role;
    let outcome = if !op.roles().contains(&role) {
        Err(Rejection::new(
            RejectCode::OpNotAllowed,
            format!("role {} may not call {}", role.as_str(), op.as_str()),
        )
        .into())
    } else {
        match op {
            AgentOp::Hello => super::ops::hello(ctx, &envelope),
            AgentOp::Event => super::ops::event(ctx, &envelope),
            AgentOp::ReceiptLookup => super::ops::receipt_lookup(ctx, &envelope),
            AgentOp::Todo => super::ops::todo::call(ctx, &envelope),
            AgentOp::Job | AgentOp::JobMonitor => super::ops::jobs::call(ctx, &envelope, op),
            AgentOp::Ask => ask(ctx, &envelope),
            AgentOp::Withdraw => withdraw(ctx, &envelope),
            AgentOp::QuestionAnswer | AgentOp::QuestionEscalate | AgentOp::QuestionCancel => {
                pm_question(ctx, &envelope, op)
            }
            other => match other.route(role) {
                Some(Route::Worker) => super::ops::worker::call(ctx, &envelope, other),
                Some(Route::Reviewer) => super::ops::reviewer::call(ctx, &envelope, other),
                Some(Route::Pm) => super::ops::pm::call(ctx, &envelope, other),
                None => Err(anyhow::anyhow!("{} has no handler", other.as_str())),
            },
        }
    };
    let error = match outcome {
        Ok(response) => return Ok(response),
        Err(error) => error,
    };
    let rejection = if let Some(rejection) = error.downcast_ref::<Rejection>() {
        rejection.clone()
    } else if let Some(refusal) = error.downcast_ref::<AgentRefusal>() {
        Rejection::from(refusal.clone())
    } else if let Some(refusal) = error.downcast_ref::<QuestionRefusal>() {
        Rejection::from(refusal.clone())
    } else {
        return Err(error);
    };
    let response = CallResponse::Rejected {
        rejection,
        op: envelope.op.clone(),
        request_id: Some(envelope.request_id.clone()),
        identity: Some(envelope.identity.clone()),
    };
    if response
        .rejection()
        .is_some_and(|r| r.code != RejectCode::OpUnavailable)
    {
        super::receipts::log_rejection(ctx, &response, input)?;
    }
    Ok(response)
}

fn parse<T: DeserializeOwned>(envelope: &Envelope, expected: &str) -> Result<T> {
    serde_json::from_value(envelope.payload.clone()).map_err(|error| {
        Rejection::new(
            RejectCode::BadRequest,
            format!(
                "malformed {} payload ({} error); expected fields: {expected}",
                envelope.op,
                error_category(&error)
            ),
        )
        .into()
    })
}

fn question_effect(key: &str) -> Option<ReceiptEffect> {
    Some(ReceiptEffect {
        kind: "question".into(),
        reference: key.into(),
    })
}

/// Recovers a question receipt's owed events, on a fresh commit and on every replay.
fn reconciled(ctx: &Ctx, response: CallResponse) -> Result<CallResponse> {
    let effect = response.receipt().and_then(|r| r.effect.as_ref());
    if let Some(effect) = effect.filter(|e| e.kind == "question") {
        questions::reconcile_question(&ctx.store, &effect.reference)?;
    }
    Ok(response)
}

/// ask: opens Q<n> on the caller's own run or review child and returns its key and revision at
/// once, blocking or not (OD6).
fn ask(ctx: &Ctx, envelope: &Envelope) -> Result<CallResponse> {
    let payload: questions::AskPayload = parse(
        envelope,
        "kind, blocking?, text, options?, recommendation?, context_refs?, needed_by?",
    )?;
    questions::validate_ask(&payload)?;
    let route = questions::wake_route(ctx)?;
    let container = admission::container_of(&envelope.identity);
    let admitted = match admission::admit(ctx, envelope, Mode::Mutation { settles_turn: None })? {
        Admission::Replay(stored) => {
            return reconciled(ctx, receipts::respond(ctx, &container, *stored, true)?);
        }
        Admission::New(admitted) => admitted,
    };
    let origin = admitted.origin.clone();
    let response = receipts::commit(ctx, &admitted, |state, _| {
        let now = jiff::Timestamp::now();
        let question = questions::open(state, &container, &origin, payload, route, now)?;
        Ok(Commit {
            result: json!({
                "key": question.key, "revision": question.revision, "state": "open",
                "blocking": question.blocking,
            }),
            effect: question_effect(&question.key),
            post_pending: Vec::new(),
        })
    })?;
    reconciled(ctx, response)
}

/// withdraw: the asking agent retracts its own pending question with a reason. A Needs me block
/// that only questions raised is settled in the same transaction.
fn withdraw(ctx: &Ctx, envelope: &Envelope) -> Result<CallResponse> {
    let payload: questions::WithdrawPayload = parse(envelope, "key, reason")?;
    let container = admission::container_of(&envelope.identity);
    let admitted = match admission::admit(ctx, envelope, Mode::Mutation { settles_turn: None })? {
        Admission::Replay(stored) => {
            return reconciled(ctx, receipts::respond(ctx, &container, *stored, true)?);
        }
        Admission::New(admitted) => admitted,
    };
    let response = receipts::commit_with_task(ctx, &admitted, |task| {
        let state = corral_core::agent::container_state_mut(task, &container)
            .ok_or_else(|| Rejection::new(RejectCode::ContainerNotFound, "no agent binding"))?;
        let (key, revision) = questions::withdraw(
            &ctx.store,
            state,
            &container,
            &payload,
            jiff::Timestamp::now(),
        )?;
        questions::settle_needs_me(task, &container);
        Ok(Commit {
            result: json!({"key": key, "revision": revision, "state": "withdrawn"}),
            effect: question_effect(&key),
            post_pending: Vec::new(),
        })
    })?;
    reconciled(ctx, response)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PmAnswer {
    key: String,
    rev: u32,
    text: String,
    #[serde(default)]
    option: Option<String>,
    #[serde(default)]
    basis_refs: Vec<String>,
    #[serde(default)]
    scope_note: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PmEscalate {
    key: String,
    rev: u32,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    kind: Option<QuestionKind>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PmCancel {
    key: String,
    rev: u32,
    reason: String,
}

enum PmCall {
    Answer(PmAnswer),
    Escalate(PmEscalate),
    Cancel(PmCancel),
}

/// question_answer, question_escalate and question_cancel from the Pi-native PM: committed in the
/// target task's transaction with a receipt in `Task.agent_pm_receipts`, after the PM's liveness,
/// fence, saved pane and project scope are rechecked there (F1.c).
fn pm_question(ctx: &Ctx, envelope: &Envelope, op: AgentOp) -> Result<CallResponse> {
    let generation = envelope.identity.generation;
    let guard = |task: &Task| -> Result<()> {
        let pm = pm_state::load(&ctx.store)?.ok_or_else(|| {
            Rejection::new(RejectCode::ContainerNotFound, "no Pi-native PM is bound")
        })?;
        admission::recheck_pm(&pm, generation)?;
        if task.project.as_deref() != Some(pm.state.binding.project.as_str()) {
            return Err(Rejection::new(
                RejectCode::OutOfScope,
                format!("{} lies outside the PM's project scope", task.id),
            )
            .into());
        }
        Ok(())
    };
    let call = match op {
        AgentOp::QuestionAnswer => PmCall::Answer(parse(
            envelope,
            "key, rev, text, option?, basis_refs?, scope_note?",
        )?),
        AgentOp::QuestionEscalate => PmCall::Escalate(parse(envelope, "key, rev, note?, kind?")?),
        _ => PmCall::Cancel(parse(envelope, "key, rev, reason")?),
    };
    // Authentication and receipt lookup come before liveness: a committed request replays from
    // the target task even after its generation is fenced; transact checks liveness for new ones.
    let admitted = match admission::admit(ctx, envelope, Mode::Lookup)? {
        Admission::Replay(stored) => {
            return receipts::respond(ctx, &ContainerRef::Pm, *stored, true);
        }
        Admission::New(admitted) => admitted,
    };
    let actor = || {
        Actor::Pm(Some(PmRequest {
            origin: &admitted.origin,
            guard: &guard,
        }))
    };
    let applied = match call {
        PmCall::Answer(p) => {
            let input = AnswerInput {
                text: p.text,
                option: p.option,
                basis_refs: p.basis_refs,
                scope_note: p.scope_note,
            };
            questions::answer(ctx, &p.key, p.rev, actor(), input)?
        }
        PmCall::Escalate(p) => questions::escalate(ctx, &p.key, p.rev, actor(), p.note, p.kind)?,
        PmCall::Cancel(p) => questions::cancel(ctx, &p.key, p.rev, actor(), &p.reason)?,
    };
    match applied {
        Applied::Replayed(receipt) => Ok(CallResponse::Receipt {
            receipt,
            replayed: true,
        }),
        Applied::Fresh {
            receipt: Some(receipt),
            ..
        } => Ok(CallResponse::Receipt {
            receipt,
            replayed: false,
        }),
        Applied::Fresh { receipt: None, .. } => Err(anyhow::anyhow!(
            "a PM question op committed without a receipt"
        )),
    }
}
