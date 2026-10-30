//! `owner_action` (T-544 owner-authority): an assistant executes one owner-level corral command
//! the owner clearly asked for in its pane, citing that journaled owner input. The instruction
//! is the authorization: corral checks only that the cited input is a real pane-typed owner
//! input of this session, then runs the command once as a direct child `corral` process that
//! claims the reservation (`crate::assistant::owner_action`), so the same CLI path, locks and refusals apply as from
//! the owner's shell. The confirm class first waits for the owner's y or n, which the host
//! journals; n records a declined receipt. A replay returns the first receipt; a reservation
//! whose executor died is reconciled from the claim and the event log, never re-executed.
#[cfg(all(test, target_os = "linux"))]
mod tests;

use std::process::Output;

use anyhow::{Context, Result};
use corral_core::agent::{
    AGENT_RECORD_VERSION, EVIDENCE_MAX, Executor, RECEIPTS_MAX, RESULT_MAX_BYTES, ReceiptEffect,
    ReceiptEvidence, ReceiptState, RequestReceipt,
};
use corral_core::assistant::{AssistantContainer, OwnerInput, OwnerInputSource};
use jiff::Timestamp;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::Ctx;
use crate::agent::admission::{self, Admission, Admitted, Mode, Resolved};
use crate::agent::call::{CallResponse, RejectCode, Rejection};
use crate::agent::effects;
use crate::agent::envelope::{Envelope, error_category};
use crate::agent::receipts::{self, Commit};
use crate::assistant::owner_action::{self as child, kinds};
use kinds::{Action, Class};

const FIELDS: &str = "kind, target?, args?, expected_revision?, input, confirmation?";
/// Output and refusal text a settled receipt keeps.
const EXCERPT_MAX: usize = 2048;
/// The reserved result, which names the exact argv, leaves room for the excerpt beside it.
const BASE_MAX: usize = RESULT_MAX_BYTES - EXCERPT_MAX - 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    kind: String,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    expected_revision: Option<u64>,
    input: String,
    #[serde(default)]
    confirmation: Option<String>,
}

fn reject(code: RejectCode, reason: impl Into<String>) -> anyhow::Error {
    Rejection::new(code, reason).into()
}

/// The cited owner input and, for the confirm class, the owner's journaled answer.
struct Cited {
    input: OwnerInput,
    confirmation: Option<OwnerInput>,
}

fn cite(assistant: &AssistantContainer, payload: &Payload, action: &Action) -> Result<Cited> {
    let input = assistant
        .owner_input(&payload.input)
        .filter(|entry| entry.source == OwnerInputSource::Pane)
        .cloned()
        .ok_or_else(|| {
            reject(
                RejectCode::NotOwnerInput,
                format!(
                    "{} is not a pane-typed owner input of this session; only the owner's own \
                     messages authorize an owner action, never a corral wake, PM message or \
                     recovery notice",
                    payload.input
                ),
            )
        })?;
    let confirmation = match (action.class, payload.confirmation.as_deref()) {
        (_, None) => None,
        (Class::Ordinary, Some(_)) => {
            return Err(reject(
                RejectCode::BadRequest,
                format!(
                    "{} runs on the owner's instruction and takes no confirmation",
                    action.kind
                ),
            ));
        }
        (Class::Confirm, Some(id)) => {
            let entry = assistant
                .owner_input(id)
                .filter(|entry| {
                    entry.source == OwnerInputSource::Confirmation
                        && entry.command.as_deref() == Some(action.command.as_str())
                })
                .cloned()
                .ok_or_else(|| {
                    reject(
                        RejectCode::NotOwnerInput,
                        format!(
                            "{id} is not the owner's answer to a confirmation of `{}`",
                            action.command
                        ),
                    )
                })?;
            let used = assistant.state.receipts.iter().any(|receipt| {
                receipt.op == "owner_action" && receipt.result["confirmation"].as_str() == Some(id)
            });
            if used {
                return Err(reject(
                    RejectCode::BadRequest,
                    format!("confirmation {id} already answered another owner action"),
                ));
            }
            Some(entry)
        }
    };
    Ok(Cited {
        input,
        confirmation,
    })
}

fn base(action: &Action, cited: &Cited) -> Value {
    let confirmation = cited.confirmation.as_ref();
    json!({
        "kind": action.kind, "command": action.command, "argv": action.argv,
        "class": action.class.as_str(), "input": cited.input.id, "sha256": cited.input.sha256,
        "confirmation": confirmation.map(|entry| &entry.id),
        "confirmed": confirmation.is_some_and(|entry| entry.text == "y"),
    })
}

/// A settled result: the reservation's fields without its argv, plus the outcome.
fn with(mut base: Value, extra: Value) -> Value {
    if let (Some(fields), Value::Object(extra)) = (base.as_object_mut(), extra) {
        fields.remove("argv");
        fields.extend(extra);
    }
    base
}

fn effect(action: &Action) -> Option<ReceiptEffect> {
    Some(ReceiptEffect {
        kind: "owner_action".into(),
        reference: format!("{} {}", action.kind, action.target),
    })
}

/// Refuses before any effect when autopilot, another approval or a landing holds the task's
/// action lock; the lock is only probed, so the command takes it itself.
fn in_flight(ctx: &Ctx, action: &Action) -> Result<()> {
    let Some(task) = &action.task else {
        return Ok(());
    };
    if let Err(error) = ctx.store.get(task) {
        if format!("{error}").ends_with("not found") {
            return Err(reject(RejectCode::BadRequest, format!("{task} not found")));
        }
        return Err(error);
    }
    let lock = format!("autopilot-{task}");
    if ctx.store.try_lock(&lock)?.is_none() {
        return Err(Rejection::new(
            RejectCode::ConflictInFlight,
            format!(
                "autopilot or another approval holds the lock {lock} on {task}; report it to \
                 the owner and do not retry or override it"
            ),
        )
        .with_detail(json!({"holder": lock, "task": task}))
        .into());
    }
    Ok(())
}

fn excerpt(bytes: &[u8], tail: bool) -> String {
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim();
    if text.len() <= EXCERPT_MAX {
        return text.to_owned();
    }
    let mut cut = if tail {
        text.len() - EXCERPT_MAX
    } else {
        EXCERPT_MAX
    };
    while !text.is_char_boundary(cut) {
        cut += 1;
    }
    if tail {
        format!("...{}", &text[cut..])
    } else {
        format!("{}...", &text[..cut])
    }
}

fn refusal_code(reason: &str) -> &'static str {
    if reason.contains("approval already in progress") {
        "conflict_in_flight"
    } else if reason.contains("stale_revision") || reason.contains("stale revision") {
        "stale_revision"
    } else {
        "refused"
    }
}

/// A started command whose process ended without a result: its events decide.
fn from_events(ctx: &Ctx, request: &str, base: Value) -> Result<(ReceiptState, Value)> {
    let events = ctx.store.events_shared()?;
    let kinds: Vec<&str> = events
        .iter()
        .filter(|event| event.detail["owner_authority"]["request"].as_str() == Some(request))
        .map(|event| event.kind.as_str())
        .take(16)
        .collect();
    Ok(if kinds.is_empty() {
        let reason = "the command started, but its process ended without a result and no event \
                      names this request; check the target before asking the owner again";
        (
            ReceiptState::Uncertain,
            with(
                base,
                json!({"outcome": "outcome_unknown", "reason": reason}),
            ),
        )
    } else {
        (
            ReceiptState::Committed,
            with(
                base,
                json!({"outcome": "executed", "reconciled": true, "events": kinds}),
            ),
        )
    })
}

fn finished(
    ctx: &Ctx,
    request: &str,
    base: Value,
    output: Result<Output>,
) -> Result<(ReceiptState, Value)> {
    let output = match output {
        Ok(output) => output,
        Err(error) => {
            let reason = format!("{error:#}");
            return Ok((
                ReceiptState::Rejected,
                with(
                    base,
                    json!({"outcome": "not_started", "code": "spawn_failed", "reason": reason}),
                ),
            ));
        }
    };
    Ok(match output.status.code() {
        Some(0) => (
            ReceiptState::Committed,
            with(
                base,
                json!({"outcome": "executed", "exit_code": 0,
                    "output": excerpt(&output.stdout, false)}),
            ),
        ),
        Some(code) => {
            let reason = excerpt(&output.stderr, true);
            (
                ReceiptState::Rejected,
                with(
                    base,
                    json!({"outcome": "refused", "code": refusal_code(&reason), "exit_code": code,
                        "reason": reason}),
                ),
            )
        }
        None => from_events(ctx, request, base)?,
    })
}

fn reserve(
    ctx: &Ctx,
    admitted: &Admitted,
    payload: &Payload,
    action: &Action,
    me: &Executor,
) -> Result<(RequestReceipt, bool)> {
    let origin = &admitted.origin;
    let session = origin
        .binding
        .container
        .assistant()
        .context("owner_action names an assistant session")?;
    ctx.store
        .settle_assistant_container(session, |assistant, ended| {
            if let Some(stored) = assistant.state.receipt(&origin.request_id) {
                admission::same_request(stored, &origin.op, &origin.payload_sha256)?;
                return Ok((stored.clone(), true));
            }
            admission::recheck_assistant(assistant, ended, origin.binding.generation)?;
            let cited = cite(assistant, payload, action)?;
            if assistant.state.receipts.len() >= RECEIPTS_MAX {
                return Err(reject(
                    RejectCode::ReceiptLimit,
                    format!("the container already holds {RECEIPTS_MAX} receipts"),
                ));
            }
            let mut result = base(action, &cited);
            result["outcome"] = json!("reserved");
            let receipt = RequestReceipt {
                version: AGENT_RECORD_VERSION,
                request_id: origin.request_id.clone(),
                op: origin.op.clone(),
                payload_sha256: origin.payload_sha256.clone(),
                identity: origin.identity.clone(),
                state: ReceiptState::Reserved,
                result,
                executor: Some(me.clone()),
                effect: effect(action),
                evidence: Vec::new(),
                post_pending: Vec::new(),
                at: Timestamp::now(),
                committed_at: None,
            };
            assistant.state.receipts.push(receipt.clone());
            Ok((receipt, false))
        })
}

/// Moves a reservation still held by `seen` to its final state. `started` (when given) must
/// still match whether a command child claimed it, so a claim racing a reconcile wins or loses
/// whole; a changed reservation is returned as stored.
fn settle(
    ctx: &Ctx,
    session: &str,
    request: &str,
    seen: Option<&Executor>,
    started: Option<bool>,
    to: (ReceiptState, Value),
    source: &str,
) -> Result<RequestReceipt> {
    ctx.store
        .settle_assistant_container(session, |assistant, _| {
            let stored = assistant
                .state
                .receipts
                .iter_mut()
                .find(|receipt| receipt.request_id == request)
                .context("the owner-action reservation disappeared")?;
            let claimed = child::started(stored).is_some();
            if stored.state != ReceiptState::Reserved
                || stored.executor.as_ref() != seen
                || started.is_some_and(|expected| expected != claimed)
            {
                return Ok(stored.clone());
            }
            if stored.evidence.len() >= EVIDENCE_MAX {
                stored.evidence.remove(0);
            }
            let now = Timestamp::now();
            stored.evidence.push(ReceiptEvidence {
                state: to.1["outcome"].as_str().unwrap_or_default().to_owned(),
                source: source.into(),
                detail: None,
                at: now,
            });
            (stored.state, stored.result) = to;
            stored.executor = None;
            stored.committed_at = (stored.state == ReceiptState::Committed).then_some(now);
            Ok(stored.clone())
        })
}

/// A replayed reservation: unchanged while its executor or command runs; once both are gone, not
/// executed when no command claimed it, else executed or uncertain by the event log.
fn reconcile(ctx: &Ctx, session: &str, stored: RequestReceipt) -> Result<RequestReceipt> {
    if stored.state != ReceiptState::Reserved {
        return Ok(stored);
    }
    if let Some(running) = &stored.executor
        && !effects::executor_gone(running)?
    {
        return Ok(stored);
    }
    let claimed = child::started(&stored);
    let to = match &claimed {
        None => (
            ReceiptState::Abandoned,
            with(
                stored.result.clone(),
                json!({"outcome": "not_executed",
                    "reason": "the executor ended before the command started"}),
            ),
        ),
        Some(command) if !effects::executor_gone(command)? => return Ok(stored),
        Some(_) => from_events(ctx, &stored.request_id, stored.result.clone())?,
    };
    let seen = stored.executor.clone();
    settle(
        ctx,
        session,
        &stored.request_id,
        seen.as_ref(),
        Some(claimed.is_some()),
        to,
        "reconcile",
    )
}

/// The `owner_action` op. Refusals before any effect store nothing: `unsupported_action`,
/// `not_owner_input`, `conflict_in_flight`, `confirmation_required` (with the command to show)
/// and `bad_request`.
pub(crate) fn call(ctx: &Ctx, envelope: &Envelope) -> Result<CallResponse> {
    let payload: Payload = serde_json::from_value(envelope.payload.clone()).map_err(|error| {
        reject(
            RejectCode::BadRequest,
            format!(
                "malformed owner_action payload ({} error); expected fields: {FIELDS}",
                error_category(&error)
            ),
        )
    })?;
    let container = admission::container_of(&envelope.identity);
    let session = envelope.identity.assistant.clone().unwrap_or_default();
    let admitted = match admission::admit(ctx, envelope, Mode::Mutation { settles_turn: None })? {
        Admission::Replay(stored) => {
            let receipt = reconcile(ctx, &session, *stored)?;
            return receipts::respond(ctx, &container, receipt, true);
        }
        Admission::New(admitted) => admitted,
    };
    let action = kinds::resolve(
        &payload.kind,
        payload.target.as_deref(),
        &payload.args,
        payload.expected_revision,
    )?;
    let Resolved::Assistant(assistant, _) = &admitted.resolved else {
        anyhow::bail!("owner_action resolved no assistant container");
    };
    let cited = cite(assistant, &payload, &action)?;
    if cited
        .confirmation
        .as_ref()
        .is_some_and(|answer| answer.text == "n")
    {
        return receipts::commit_assistant(ctx, &admitted, |assistant| {
            let cited = cite(assistant, &payload, &action)?;
            Ok(Commit {
                result: with(base(&action, &cited), json!({"outcome": "declined"})),
                effect: effect(&action),
                post_pending: Vec::new(),
            })
        });
    }
    in_flight(ctx, &action)?;
    if action.class == Class::Confirm && cited.confirmation.is_none() {
        return Err(Rejection::new(
            RejectCode::ConfirmationRequired,
            format!(
                "`{}` waits for the owner's confirmation in the pane",
                action.command
            ),
        )
        .with_detail(json!({"command": action.command, "kind": action.kind}))
        .into());
    }
    if serde_json::to_vec(&base(&action, &cited))?.len() > BASE_MAX {
        return Err(reject(
            RejectCode::BadRequest,
            "the command is too long for one receipt; write long notes to a file and pass it \
             with the command's file flag",
        ));
    }
    let me = effects::executor()?;
    let (reserved, replayed) = reserve(ctx, &admitted, &payload, &action, &me)?;
    if replayed {
        let receipt = reconcile(ctx, &session, reserved)?;
        return receipts::respond(ctx, &container, receipt, true);
    }
    effects::failpoint("owner_action_reserved_before_run");
    let output = child::run(ctx, &session, &envelope.request_id, &action.argv);
    effects::failpoint("owner_action_ran_before_settle");
    let to = finished(ctx, &envelope.request_id, reserved.result.clone(), output)?;
    let receipt = settle(
        ctx,
        &session,
        &envelope.request_id,
        Some(&me),
        None,
        to,
        "executor",
    )?;
    receipts::respond(ctx, &container, receipt, false)
}
