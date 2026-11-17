//! Canonical owner actions, citing journaled owner instructions. Interactive pane behavior
//! is unchanged. Daemon-hosted assistants additionally require server-authenticated, fresh,
//! untainted device provenance and current assistant_act authority at citation/reservation.
//! High-risk actions use the same journaled confirmation mechanism for panes and apps;
//! app confirmation is device-bound and can only be written by the daemon prompt API.
//! Commands run once as the existing reserved child; replay and uncertain recovery never
//! re-execute a command.
#[cfg(all(test, target_os = "linux"))]
mod crash_tests;
#[cfg(all(test, target_os = "linux"))]
mod tests;

use std::process::Output;

use anyhow::{Context, Result};
use corral_core::Status;
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
use crate::assistant::scratch::AttachedReview;
use kinds::{Action, Class};

const FIELDS: &str = "kind, target?, args?, expected_revision?, input, review?, slices?, messages?";
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
    /// Host fields, deliberately absent from the model's tool schema and command flags.
    #[serde(default)]
    review: Option<AttachedReview>,
    #[serde(default, deserialize_with = "deserialize_slices")]
    slices: Vec<corral_core::build_record::RecordSlice>,
    #[serde(default)]
    messages: Vec<corral_core::build_record::RecordMessage>,
}

/// Decimal USD travels as text through the integer-only authenticated envelope. Keep the
/// build-record schema numeric and continue accepting older numeric slice costs.
fn deserialize_slices<'de, D>(
    deserializer: D,
) -> std::result::Result<Vec<corral_core::build_record::RecordSlice>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;

    let mut slices = Vec::<Value>::deserialize(deserializer)?;
    for slice in &mut slices {
        if let Some(cost) = slice.pointer_mut("/usage/cost")
            && let Some(decimal) = cost.as_str()
        {
            *cost = Value::Number(decimal.parse().map_err(D::Error::custom)?);
        }
    }
    slices
        .into_iter()
        .map(|slice| serde_json::from_value(slice).map_err(D::Error::custom))
        .collect()
}

fn reject(code: RejectCode, reason: impl Into<String>) -> anyhow::Error {
    Rejection::new(code, reason).into()
}

/// The cited owner input and, for the confirm class, the owner's journaled answer.
struct Cited {
    input: OwnerInput,
    confirmation: Option<OwnerInput>,
    confirm_class: bool,
}

fn cite(
    store: &corral_core::Store,
    assistant: &AssistantContainer,
    payload: &Payload,
    action: &Action,
    request: &str,
) -> Result<Cited> {
    let input = assistant
        .owner_input(&payload.input)
        .filter(|entry| {
            matches!(
                entry.source,
                OwnerInputSource::Pane | OwnerInputSource::Dialog
            )
        })
        .cloned()
        .ok_or_else(|| {
            reject(
                RejectCode::NotOwnerInput,
                format!(
                    "{} is not a pane-typed or dialog-selected owner input of this session; only the owner's own \
                     messages authorize an owner action, never a corral wake, PM message or \
                     recovery notice",
                    payload.input
                ),
            )
        })?;
    let origin = crate::agent::app_runtime::input::recheck(store, assistant, &input)
        .map_err(|error| reject(RejectCode::NotOwnerInput, error.to_string()))?;
    let confirm_class =
        action.class == Class::Confirm || origin.as_ref().is_some_and(|p| p.confirm_class());
    let confirmation = if confirm_class {
        assistant.confirmation(request, &action.command).cloned()
    } else {
        None
    };
    if let Some(answer) = &confirmation
        && let Some(origin) = origin
    {
        let confirmed = crate::agent::app_runtime::input::recheck(store, assistant, answer)
            .map_err(|error| reject(RejectCode::NotOwnerInput, error.to_string()))?
            .ok_or_else(|| {
                reject(
                    RejectCode::NotOwnerInput,
                    "app confirmation has no device provenance",
                )
            })?;
        if confirmed.device != origin.device
            || answer.generation != assistant.state.binding.generation
        {
            return Err(reject(
                RejectCode::NotOwnerInput,
                "confirmation belongs to a different device or generation",
            ));
        }
        if origin.confirm_class() {
            super::owner_prompt::check_attachment_confirmation(
                store,
                &input,
                answer,
                request,
                &action.command,
            )
            .map_err(|error| reject(RejectCode::NotOwnerInput, error.to_string()))?;
        }
    }
    Ok(Cited {
        input,
        confirmation,
        confirm_class,
    })
}

fn base(action: &Action, cited: &Cited) -> Value {
    let confirmation = cited.confirmation.as_ref();
    json!({
        "kind": action.kind, "target": action.target, "command": action.command, "argv": action.argv,
        "class": if cited.confirm_class { Class::Confirm.as_str() } else { action.class.as_str() },
        "input": cited.input.id, "sha256": cited.input.sha256,
        "confirmation": confirmation.map(|entry| &entry.id),
        "confirmed": matches!(action.kind, "finish" | "finish_here") || confirmation.is_some_and(|entry| entry.text == "y"),
    })
}

fn reservation_base(action: &Action, cited: &Cited, payload: &Payload) -> Value {
    let mut result = base(action, cited);
    if action.kind == "finish_here" {
        result["finish_here_evidence"] = json!({
            "review": payload.review, "slices": payload.slices, "messages": payload.messages,
        });
    }
    result
}

/// A settled result: the reservation's fields without its argv, plus the outcome.
fn with(mut base: Value, extra: Value) -> Value {
    if let (Some(fields), Value::Object(extra)) = (base.as_object_mut(), extra) {
        fields.remove("argv");
        fields.remove("finish_here_evidence");
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
    } else if reason.contains("stale_candidate:") {
        "stale_candidate"
    } else if reason.contains("landing_stale:") {
        "landing_stale"
    } else if reason.contains("in_session_finish_disabled:") {
        "in_session_finish_disabled"
    } else if reason.contains("review_required:") {
        "review_required"
    } else {
        "refused"
    }
}

/// An approval is the one kind with an unambiguous terminal state: its task is Done and the
/// task's `approved` event carries this request's authority.
fn approved(ctx: &Ctx, receipt: &RequestReceipt) -> Result<bool> {
    let (Some("approve"), Some(task)) = (
        receipt.result["kind"].as_str(),
        receipt.result["target"].as_str(),
    ) else {
        return Ok(false);
    };
    if ctx.store.get(task)?.status != Status::Done {
        return Ok(false);
    }
    Ok(ctx.store.events_shared()?.iter().any(|event| {
        event.kind == "approved"
            && event.task.as_deref() == Some(task)
            && event.detail["owner_authority"]["request"].as_str()
                == Some(receipt.request_id.as_str())
    }))
}

/// A command child that claimed its reservation and ended without the executor seeing its
/// exit: the child's own completion record decides; without one, only a verified terminal
/// state does. Events written before or during the effect (a rebase before landing, a claim
/// before a launch, a failed landing) prove nothing, so everything else stays uncertain.
fn after_claim(ctx: &Ctx, receipt: &RequestReceipt) -> Result<(ReceiptState, Value)> {
    let base = receipt.result.clone();
    if let Some(error) = child::completion(receipt) {
        return Ok(match error {
            None => (
                ReceiptState::Committed,
                with(
                    base,
                    json!({"outcome": "executed", "reconciled": "completion_record"}),
                ),
            ),
            Some(reason) => (
                ReceiptState::Rejected,
                with(
                    base,
                    json!({"outcome": "refused", "code": refusal_code(&reason),
                        "reason": reason, "reconciled": "completion_record"}),
                ),
            ),
        });
    }
    if approved(ctx, receipt)? {
        return Ok((
            ReceiptState::Committed,
            with(
                base,
                json!({"outcome": "executed", "reconciled": "target_state"}),
            ),
        ));
    }
    let reason = "the command started, but its process ended without recording a result and the \
                  target shows no verified completion; check it before asking the owner again";
    Ok((
        ReceiptState::Uncertain,
        with(
            base,
            json!({"outcome": "outcome_unknown", "reason": reason}),
        ),
    ))
}

fn finished(
    ctx: &Ctx,
    session: &str,
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
        Some(0) if base["kind"] == "finish_here" => {
            let result: Value = serde_json::from_slice(&output.stdout)
                .context("reading structured finish_here action result")?;
            let mut result = with(base, result);
            result["outcome"] = json!("executed");
            (ReceiptState::Committed, result)
        }
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
        None => {
            let container = ctx.store.get_assistant_container(session)?;
            let stored =
                receipts::assistant_receipt(&ctx.store, session, &container.state, request)?
                    .context("the owner-action reservation disappeared")?;
            after_claim(ctx, &stored)?
        }
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
            if let Some(stored) = receipts::assistant_receipt(
                &ctx.store,
                session,
                &assistant.state,
                &origin.request_id,
            )? {
                admission::same_request(&stored, &origin.op, &origin.payload_sha256)?;
                return Ok((stored, true));
            }
            admission::recheck_assistant(assistant, ended, origin.binding.generation)?;
            crate::assistant::workflow::boundary::require_ordinary(&ctx.store, session, assistant)?;
            let cited = cite(&ctx.store, assistant, payload, action, &origin.request_id)?;
            receipts::assistant_capacity(&ctx.store, session, &mut assistant.state)?;
            if assistant.state.receipts.len() >= RECEIPTS_MAX {
                return Err(reject(
                    RejectCode::ReceiptLimit,
                    format!("the container already holds {RECEIPTS_MAX} receipts"),
                ));
            }
            let mut result = reservation_base(action, &cited, payload);
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
            if !assistant
                .state
                .receipts
                .iter()
                .any(|receipt| receipt.request_id == request)
            {
                return receipts::assistant_receipt(
                    &ctx.store,
                    session,
                    &assistant.state,
                    request,
                )?
                .context("the owner-action reservation disappeared");
            }
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
/// executed when no command claimed it, else settled by `after_claim`.
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
        Some(_) => after_claim(ctx, &stored)?,
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
    if payload.kind != "finish_here"
        && (payload.review.is_some() || !payload.slices.is_empty() || !payload.messages.is_empty())
    {
        return Err(reject(
            RejectCode::BadRequest,
            "host build evidence is only valid for finish_here",
        ));
    }
    let Resolved::Assistant(assistant, _) = &admitted.resolved else {
        anyhow::bail!("owner_action resolved no assistant container");
    };
    crate::assistant::workflow::boundary::require_ordinary(&ctx.store, &session, assistant)?;
    let request = envelope.request_id.as_str();
    let cited = cite(&ctx.store, assistant, &payload, &action, request)?;
    if cited
        .confirmation
        .as_ref()
        .is_some_and(|answer| answer.text == "n")
    {
        return receipts::commit_assistant(ctx, &admitted, |assistant| {
            let cited = cite(&ctx.store, assistant, &payload, &action, request)?;
            Ok(Commit {
                result: with(base(&action, &cited), json!({"outcome": "declined"})),
                effect: effect(&action),
                post_pending: Vec::new(),
            })
        });
    }
    in_flight(ctx, &action)?;
    if cited.confirm_class && cited.confirmation.is_none() {
        let prompt = if assistant
            .state
            .current_generation()
            .is_some_and(|g| g.daemon_hosted)
        {
            Some(super::owner_prompt::confirmation(
                ctx,
                &session,
                request,
                &action.command,
                &payload.input,
            )?)
        } else {
            None
        };
        return Err(Rejection::new(
            RejectCode::ConfirmationRequired,
            format!(
                "`{}` waits for the owner's fresh confirmation",
                action.command
            ),
        )
        .with_detail(json!({"command": action.command, "kind": action.kind, "prompt": prompt}))
        .into());
    }
    if serde_json::to_vec(&reservation_base(&action, &cited, &payload))?.len() > BASE_MAX {
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
    let to = finished(ctx, &session, request, reserved.result.clone(), output)?;
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
