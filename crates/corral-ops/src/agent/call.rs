//! `corral agent call --op <op>`: the enumerated v1 op list with its allowed roles and owning
//! child, rejection codes, the response shape and routing to the op handlers.
use anyhow::Result;
use corral_core::agent::{
    AGENT_RECORD_VERSION, AgentRefusal, AgentRole, RefusalCode, RequestIdentity, RequestReceipt,
};
use serde_json::{Value, json};

use crate::Ctx;

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
}

const ALL_ROLES: &[AgentRole] = &[AgentRole::Worker, AgentRole::Reviewer, AgentRole::Pm];
const AGENTS: &[AgentRole] = &[AgentRole::Worker, AgentRole::Reviewer];
const WORKER: &[AgentRole] = &[AgentRole::Worker];
const REVIEWER: &[AgentRole] = &[AgentRole::Reviewer];
const PM: &[AgentRole] = &[AgentRole::Pm];

impl AgentOp {
    pub const ALL: [AgentOp; 27] = [
        AgentOp::Hello,
        AgentOp::Event,
        AgentOp::ReceiptLookup,
        AgentOp::Ask,
        AgentOp::Withdraw,
        AgentOp::Wait,
        AgentOp::Progress,
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
        }
    }

    pub fn parse(name: &str) -> Option<AgentOp> {
        AgentOp::ALL.into_iter().find(|op| op.as_str() == name)
    }

    /// The Rust authority on which role may call the op; role loadouts are a convenience.
    pub fn roles(self) -> &'static [AgentRole] {
        match self {
            AgentOp::Hello | AgentOp::Event | AgentOp::ReceiptLookup => ALL_ROLES,
            AgentOp::Ask | AgentOp::Withdraw | AgentOp::Evidence | AgentOp::Artifact => AGENTS,
            AgentOp::Wait | AgentOp::Progress | AgentOp::Checkpoint => WORKER,
            AgentOp::Gate | AgentOp::Done => WORKER,
            AgentOp::Verdict => REVIEWER,
            _ => PM,
        }
    }

    /// The T-252 child that lands the op's handler for this role; `None` once implemented.
    pub fn owner(self, role: AgentRole) -> Option<&'static str> {
        match self {
            AgentOp::Hello | AgentOp::Event | AgentOp::ReceiptLookup => None,
            AgentOp::Ask | AgentOp::Withdraw => Some("question-lifecycle"),
            AgentOp::QuestionAnswer | AgentOp::QuestionEscalate | AgentOp::QuestionCancel => {
                Some("question-lifecycle")
            }
            AgentOp::Evidence | AgentOp::Artifact if role == AgentRole::Reviewer => {
                Some("reviewer-role")
            }
            AgentOp::Verdict => Some("reviewer-role"),
            AgentOp::Wait
            | AgentOp::Progress
            | AgentOp::Checkpoint
            | AgentOp::Evidence
            | AgentOp::Artifact
            | AgentOp::Gate
            | AgentOp::Done => Some("worker-role"),
            _ => Some("pm-role"),
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

impl std::fmt::Display for Rejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.reason)
    }
}

impl std::error::Error for Rejection {}

#[derive(Debug, Clone, PartialEq)]
pub enum CallResponse {
    /// Committed now, replayed from the container, or read by receipt_lookup.
    Receipt {
        receipt: RequestReceipt,
        replayed: bool,
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
            CallResponse::Rejected { .. } => None,
        }
    }

    pub fn rejection(&self) -> Option<&Rejection> {
        match self {
            CallResponse::Rejected { rejection, .. } => Some(rejection),
            CallResponse::Receipt { .. } => None,
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
            super::receipts::log_rejection(ctx, &response)?;
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
            other => {
                let owner = other.owner(role).unwrap_or("request-receipts");
                Err(Rejection {
                    owner: Some(owner),
                    ..Rejection::new(
                        RejectCode::OpUnavailable,
                        format!("{} lands with T-252 child {owner}", other.as_str()),
                    )
                }
                .into())
            }
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
        super::receipts::log_rejection(ctx, &response)?;
    }
    Ok(response)
}
