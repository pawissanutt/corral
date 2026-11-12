//! A host-only lifecycle event, committed under the same container transaction as its receipt.
//! Concurrent providers cannot admit against separate in-memory balances.
use crate::{
    Ctx,
    agent::{
        admission::{self, Admission, Mode},
        call::{CallResponse, RejectCode, Rejection},
        envelope::{Envelope, event_id_parts},
        receipts::{self, Commit},
    },
};
use anyhow::Result;
use corral_core::agent::{AgentRole, BudgetLimits, BudgetRequest};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    kind: String,
    action: String,
    request: String,
    #[serde(default)]
    response: Option<String>,
    #[serde(default)]
    tokens: u64,
    #[serde(default)]
    usd_nanos: u64,
    #[serde(default)]
    priced: bool,
    #[serde(default)]
    unbounded: bool,
    /// Absent on older hosts; never infer a cache miss from missing telemetry.
    #[serde(default)]
    usage: Option<crate::pm_usage_guard::RequestUsage>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
}
fn bad(message: &str) -> anyhow::Error {
    Rejection::new(RejectCode::BadRequest, message).into()
}
pub(super) fn budget(ctx: &Ctx, envelope: &Envelope) -> Result<CallResponse> {
    let p: Payload = super::payload(
        envelope,
        "kind=budget, action, request, response?, tokens?, usd_nanos?",
    )?;
    if p.kind != "budget"
        || event_id_parts(&envelope.request_id).is_none()
        || p.request.is_empty()
        || p.request.len() > 128
        || p.response
            .as_ref()
            .is_some_and(|r| r.is_empty() || r.len() > 256)
        || p.tokens > 9_007_199_254_740_991
        || p.usd_nanos > 9_007_199_254_740_991
        || p.usage.as_ref().is_some_and(|usage| !usage.valid(p.tokens))
        || [&p.provider, &p.model]
            .into_iter()
            .flatten()
            .any(|name| name.is_empty() || name.len() > 256)
    {
        return Err(bad("invalid budget lifecycle event"));
    }
    let container = admission::container_of(&envelope.identity);
    let admitted = match admission::admit(ctx, envelope, Mode::Mutation { settles_turn: None })? {
        Admission::Replay(stored) => return receipts::respond(ctx, &container, *stored, true),
        Admission::New(admitted) => admitted,
    };
    let generation = envelope.identity.generation;
    let is_pm = envelope.identity.role == AgentRole::Pm;
    receipts::commit_selected(
        ctx,
        &admitted,
        // Hard caps fail closed if any bound generation predates accounting. The
        // current generation alone cannot establish that historical guarantee.
        &receipts::pm_selection(&admitted.origin)
            .family("budget_requests")
            .family("generations"),
        move |state, _| {
            let binding = state
                .current_generation()
                .and_then(|g| g.connection.clone());
            let connection = binding.as_ref().map(|b| b.connection);
            if let Some(connection) = connection {
                connection.require_admission()?;
            }
            let feature = state
                .current_generation()
                .and_then(|g| g.capabilities.as_ref())
                .and_then(|s| s.features.get("budget"));
            // Accounting runs even without limits so later explicit resume cannot erase prior spend.
            let limits = feature
                .filter(|f| f.enabled)
                .map(|f| BudgetLimits::from_settings(&f.settings))
                .unwrap_or_default();
            // Retired generations carry their gap in the container's archive.
            let legacy_usage_gap = state.legacy_usage_gap();
            let mut guard_paused = is_pm && crate::pm_usage_guard::paused(ctx, generation)?;
            let admitted = match p.action.as_str() {
                "reserve" => {
                    if p.response.is_some() || p.tokens == 0 {
                        return Err(bad("reserve needs positive tokens and no response"));
                    }
                    if guard_paused {
                        return Ok(Commit {
                            result: json!({
                                "admitted":false, "guard_paused":true,
                                "reason":crate::pm_usage_guard::PauseReason::CacheGuardPaused,
                                "exhausted":false,
                            }),
                            effect: None,
                            post_pending: Vec::new(),
                        });
                    }
                    if legacy_usage_gap
                        && (limits.max_tokens.is_some() || limits.max_usd_nanos.is_some())
                    {
                        return Ok(Commit {
                            result: json!({"admitted":false, "legacy_usage_gap":true,
                        "reason":"historical generation predates budget accounting; hard cap cannot be established"}),
                            effect: None,
                            post_pending: Vec::new(),
                        });
                    }
                    state.budget.reserve(
                        BudgetRequest {
                            connection,
                            request: p.request,
                            generation,
                            tokens: p.tokens,
                            usd_nanos: p.usd_nanos,
                            priced: p.priced,
                            unbounded: p.unbounded,
                            response: None,
                        },
                        limits,
                    )
                }
                "settle" => {
                    let response = p
                        .response
                        .ok_or_else(|| bad("settle needs a canonical response identity"))?;
                    let duplicate = state
                        .budget
                        .requests
                        .iter()
                        .any(|r| r.response.as_ref() == Some(&response));
                    let guard_response =
                        (is_pm && !duplicate && p.usage.is_some()).then(|| response.clone());
                    state
                        .budget
                        .settle(&p.request, generation, response, p.tokens, p.usd_nanos)
                        .map_err(|e| bad(&e))?;
                    if let (Some(response), Some(usage)) = (guard_response, p.usage) {
                        guard_paused = crate::pm_usage_guard::observe(
                            ctx,
                            crate::pm_usage_guard::Observation {
                                generation,
                                request: p.request,
                                response,
                                model: p.model,
                                provider: p.provider,
                                at: corral_core::clock::now(),
                                usage,
                            },
                        )?;
                    }
                    true
                }
                "state" => true,
                _ => return Err(bad("unknown budget action")),
            };
            let (tokens, usd_nanos) = state.budget.totals();
            Ok(Commit {
                result: json!({"admitted":admitted, "exhausted":state.budget.exhausted(limits),
            "guard_paused":guard_paused,
            "connection":connection, "billing":connection.map(|c| c.billing()),
            "binding_digest":binding.as_ref().map(|b| &b.digest),
            "connection_identity":binding.as_ref().map(|b| &b.identity),
            "candidate":binding.as_ref().and_then(|b| b.candidate.as_ref()),
            "tokens":tokens, "usd_nanos":usd_nanos, "limits":limits,
            "uncertain_requests":state.budget.requests.iter().filter(|r| r.response.is_none()).count(),
            "pricing_gaps":state.budget.requests.iter().filter(|r| !r.priced).count(),
            "legacy_usage_gap":legacy_usage_gap,
            "unbounded_requests":state.budget.requests.iter().filter(|r| r.unbounded && r.response.is_none()).count()}),
                effect: None,
                post_pending: Vec::new(),
            })
        },
    )
}
