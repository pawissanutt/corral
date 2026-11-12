//! Typed one-item PM actions (Part 2 B): `act` executes one executable workset option, `park`
//! parks one task and `attend` records handling of one attention item. Authority is decided at
//! the mutation: `act` revalidates its ID against fresh policy, scope, hold and task identity
//! (owner-required options never receive an ID), and every action refuses a task outside the
//! bound PM's approved projects before any effect. The reservation commits before the effect,
//! so a replay answers from the stored receipt and nothing executes twice.
#[cfg(test)]
#[path = "pm_actions_tests.rs"]
mod tests;

use anyhow::{Context, Result};
use corral_core::agent::{AgentRole, ReceiptState, RequestOrigin, RequestReceipt};
use corral_core::attention::ConditionKey;
use corral_core::{Responsible, ResumeCheck, Store, TaskHead};
use jiff::Timestamp;
use serde::Deserialize;
use serde_json::{Value, json};

use super::super::admission::{self, Admission, Mode, Resolved};
use super::super::call::{CallResponse, RejectCode, Rejection};
use super::super::effects;
use super::super::envelope::Envelope;
use crate::{Ctx, PmActionOutcome, PmWorkset};

const FIELDS: &str =
    "action, id?, task?, reason?, resume_when?, resume_action?, review_by?, key?, note?";
/// Headroom under the 8 KiB wire cap for the receipt around the result.
const RESULT_BUDGET: usize = 7000;
const REASON_MAX: usize = 1024;
/// The `corral attention attend` default re-check interval.
const ATTEND_NEXT_CHECK_SECS: u64 = 1800;
/// The `corral attention park` default review interval.
const PARK_REVIEW_SECS: u64 = 86_400;

/// The closed action set. No free-form command, owner flag or task text reaches an executor.
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    Act {
        id: String,
    },
    Park {
        task: String,
        reason: String,
        #[serde(default)]
        resume_when: Vec<String>,
        #[serde(default)]
        resume_action: Option<String>,
        #[serde(default)]
        review_by: Option<String>,
    },
    Attend {
        key: String,
        note: String,
    },
}

fn reject(code: RejectCode, reason: impl Into<String>) -> anyhow::Error {
    Rejection::new(code, reason).into()
}

fn clip(text: &str, max: usize) -> String {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

pub(crate) fn call(ctx: &Ctx, envelope: &Envelope) -> Result<CallResponse> {
    if envelope.identity.role != AgentRole::Pm {
        return Err(reject(
            RejectCode::OpNotAllowed,
            "pm_action belongs to the PM",
        ));
    }
    let action: Action = super::payload(envelope, FIELDS)?;
    let admitted = match admission::admit(ctx, envelope, Mode::Mutation { settles_turn: None })? {
        Admission::Replay(stored) => return replay(ctx, *stored),
        Admission::New(admitted) => admitted,
    };
    let Resolved::Pm(pm) = &admitted.resolved else {
        return Err(reject(
            RejectCode::ContainerNotFound,
            "no corral-pi PM is bound",
        ));
    };
    let scope = pm.state.binding.project.as_str();
    let origin = &admitted.origin;
    match action {
        Action::Act { id } => reserve(ctx, origin, &id, || act(ctx, scope, &id)),
        Action::Park {
            task,
            reason,
            resume_when,
            resume_action,
            review_by,
        } => {
            let checks = resume_when
                .iter()
                .map(String::as_str)
                .map(corral_core::parse_park_check)
                .collect::<Result<Vec<_>>>()
                .map_err(|error| {
                    reject(
                        RejectCode::BadRequest,
                        clip(&format!("{error:#}"), REASON_MAX),
                    )
                })?;
            let review = review_by
                .as_deref()
                .map_or(Ok(PARK_REVIEW_SECS), review_secs)?;
            let head = in_scope(ctx, scope, &task)?;
            let resume_action = resume_action.unwrap_or_default();
            reserve(ctx, origin, &head.id, || {
                park(ctx, &head.id, &reason, &checks, &resume_action, review)
            })
        }
        Action::Attend { key, note } => {
            let head = in_scope(ctx, scope, key.split('/').next().unwrap_or_default())?;
            reserve(ctx, origin, &key, || {
                attend(ctx, &head.id, &key, &note, &origin.request_id)
            })
        }
    }
}

fn replay(ctx: &Ctx, stored: RequestReceipt) -> Result<CallResponse> {
    let receipt = if stored.state == ReceiptState::Reserved {
        effects::reconcile_pm(ctx, &stored.request_id)?
    } else {
        stored
    };
    Ok(CallResponse::Receipt {
        receipt,
        replayed: true,
    })
}

/// The PM two-phase journal: the reservation commits before the effect. A refusal settles as
/// a stored rejection; any other failure leaves the outcome unknown and is never retried.
fn reserve(
    ctx: &Ctx,
    origin: &RequestOrigin,
    target: &str,
    apply: impl FnOnce() -> Result<Value>,
) -> Result<CallResponse> {
    let reserved = effects::pm_reserve(&ctx.store, origin, "pm_action", target)?;
    if reserved.replayed {
        return replay(ctx, reserved.receipt);
    }
    effects::failpoint("pm_reserved_before_effect");
    let (state, result) = match apply() {
        Ok(result) => (ReceiptState::Committed, result),
        Err(error) => match error.downcast_ref::<Rejection>() {
            Some(rejection) => (ReceiptState::Rejected, refusal(rejection)),
            None => (
                ReceiptState::Uncertain,
                json!({
                    "outcome": "outcome_unknown",
                    "reason": clip(&format!("{error:#}"), REASON_MAX),
                }),
            ),
        },
    };
    let receipt = effects::pm_settle(&ctx.store, &reserved.receipt, state, result)?;
    Ok(CallResponse::Receipt {
        receipt,
        replayed: false,
    })
}

fn refusal(rejection: &Rejection) -> Value {
    let mut result = json!({
        "outcome": "refused", "code": rejection.code.as_str(), "reason": rejection.reason,
    });
    if let Some(Value::Object(fields)) = &rejection.detail {
        for (key, value) in fields {
            result[key.as_str()] = value.clone();
        }
    }
    result
}

/// The task must exist and lie inside the bound PM's approved projects.
fn in_scope(ctx: &Ctx, scope: &str, task: &str) -> Result<TaskHead> {
    let head = Store::normalize_id(task)
        .and_then(|id| ctx.store.head(&id))
        .map_err(|error| {
            reject(
                RejectCode::BadRequest,
                clip(&format!("unknown task: {error:#}"), REASON_MAX),
            )
        })?;
    if !super::super::hooks::pm::contains(&ctx.store, scope, head.project.as_deref())? {
        return Err(reject(
            RejectCode::OutOfScope,
            format!("{} lies outside the PM's project scope", head.id),
        ));
    }
    Ok(head)
}

/// `pm act`'s revalidation and scoped executor, with the PM's scope checked on the freshly
/// selected action before anything runs.
fn act(ctx: &Ctx, scope: &str, id: &str) -> Result<Value> {
    let mut selected = None;
    let acted = crate::pm_workset::pm_act_checked(ctx, id, |executable| {
        let task = executable
            .action
            .task
            .as_deref()
            .context("automatic action has no task")?;
        in_scope(ctx, scope, task)?;
        selected = Some((
            task.to_owned(),
            executable.action.kind,
            executable.action.recommended.clone(),
            executable.authority,
        ));
        Ok(())
    })?;
    let outcome = match acted.outcome {
        PmActionOutcome::Stale => return Err(stale(&acted.workset)),
        PmActionOutcome::Progressed => "progressed",
        PmActionOutcome::Waiting => "waiting",
    };
    let (task, kind, option, authority) =
        selected.context("pm act executed an action it did not check")?;
    let status = ctx.store.head(&task)?.status;
    let mut result = json!({
        "action": "act", "outcome": outcome, "task": &task, "kind": kind, "option": option,
        "authority": authority, "status": status.as_str(),
        "policy_revision": acted.workset.policy_revision, "workset_status": acted.workset.status,
    });
    if let Some(diagnosis) = &acted.diagnosis {
        result["diagnosis"] = serde_json::to_value(diagnosis)?;
        if serde_json::to_vec(&result)?.len() > RESULT_BUDGET {
            result["diagnosis"] = json!({
                "omitted": true,
                "follow_up": {"op": "read_show", "payload": {"task": &task, "section": "automation"}},
            });
        }
    }
    Ok(result)
}

/// The ID is not executable in the fresh workset; the refusal carries the fresh state.
fn stale(workset: &PmWorkset) -> anyhow::Error {
    let blockers: Vec<String> = workset
        .blockers
        .iter()
        .take(4)
        .map(|blocker| clip(blocker, 256))
        .collect();
    Rejection::new(
        RejectCode::StaleRevision,
        format!(
            "stale_action: not executable under policy revision {}; policy, scope, hold or task \
             identity changed, or the option needs the owner. Re-read corral_next",
            workset.policy_revision
        ),
    )
    .with_detail(json!({
        "policy_revision": workset.policy_revision, "paused": workset.paused,
        "workset_status": workset.status, "executable": workset.actions.len(),
        "blockers": blockers,
    }))
    .into()
}

/// Park and attend refuse with their own machine prefixes; the refusal keeps them.
fn service_refusal(error: &anyhow::Error) -> anyhow::Error {
    let reason = clip(&format!("{error:#}"), REASON_MAX);
    let code = if reason.contains("owner_park:") {
        RejectCode::OpNotAllowed
    } else if reason.contains("stale_") {
        RejectCode::StaleRevision
    } else if reason.contains("attention_busy") {
        RejectCode::ConflictInFlight
    } else {
        RejectCode::BadRequest
    };
    reject(code, reason)
}

fn review_secs(value: &str) -> Result<u64> {
    [('s', 1_u64), ('m', 60), ('h', 3600), ('d', 86_400)]
        .into_iter()
        .find_map(|(unit, scale)| value.strip_suffix(unit).map(|number| (number, scale)))
        .and_then(|(number, scale)| number.parse::<u64>().ok()?.checked_mul(scale))
        .filter(|secs| (3600..=604_800).contains(secs))
        .ok_or_else(|| {
            reject(
                RejectCode::BadRequest,
                "review_by must be 1h..7d, written <n>s, <n>m, <n>h or <n>d",
            )
        })
}

/// The `corral attention park` service as the PM: it never replaces an owner park.
fn park(
    ctx: &Ctx,
    task: &str,
    reason: &str,
    checks: &[ResumeCheck],
    resume_action: &str,
    review_secs: u64,
) -> Result<Value> {
    let now = Timestamp::now();
    let parked =
        match crate::park::park_by_pm(ctx, task, reason, checks, resume_action, review_secs, now) {
            Ok(park) => park,
            // The park and its event are one task write; a PM park stamped with this request's
            // time is the evidence it committed before the error.
            Err(error) => ctx
                .store
                .head(task)?
                .park
                .clone()
                .filter(|park| park.by == Responsible::Pm && park.at == now)
                .ok_or_else(|| service_refusal(&error))?,
        };
    Ok(json!({
        "action": "park", "outcome": "parked", "task": task, "revision": parked.revision,
        "by": parked.by, "review_by": parked.review_by,
        "resume_when": parked.resume_when.iter().map(|check| &check.description).collect::<Vec<_>>(),
    }))
}

/// Attention handling as the PM, after the service rereads the item; stale keys are refused.
fn attend(ctx: &Ctx, task: &str, key: &str, note: &str, request: &str) -> Result<Value> {
    let handling = match crate::attention::attend_by_pm(
        ctx,
        key,
        note,
        ATTEND_NEXT_CHECK_SECS,
        request,
        Timestamp::now(),
    ) {
        Ok(handling) => handling,
        // Handling commits before its wake publication; the request names the record.
        Err(error) => ctx
            .store
            .get(task)?
            .attention
            .records
            .get(&ConditionKey(key.into()))
            .and_then(|record| record.handling.clone())
            .filter(|handling| handling.request == request)
            .ok_or_else(|| service_refusal(&error))?,
    };
    Ok(json!({"action": "attend", "outcome": "handled", "key": key, "handling": handling}))
}
