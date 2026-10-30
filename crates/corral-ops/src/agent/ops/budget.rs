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
use corral_core::agent::{BudgetLimits, BudgetRequest};
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
    {
        return Err(bad("invalid budget lifecycle event"));
    }
    let container = admission::container_of(&envelope.identity);
    let admitted = match admission::admit(ctx, envelope, Mode::Mutation { settles_turn: None })? {
        Admission::Replay(stored) => return receipts::respond(ctx, &container, *stored, true),
        Admission::New(admitted) => admitted,
    };
    let generation = envelope.identity.generation;
    receipts::commit(ctx, &admitted, move |state, _| {
        let feature = state
            .current_generation()
            .and_then(|g| g.capabilities.as_ref())
            .and_then(|s| s.features.get("budget"));
        // Accounting runs even without limits so later explicit resume cannot erase prior spend.
        let limits = feature
            .filter(|f| f.enabled)
            .map(|f| BudgetLimits::from_settings(&f.settings))
            .unwrap_or_default();
        let legacy_usage_gap = state
            .generations
            .iter()
            .any(|g| g.bound.is_some() && g.capabilities.as_ref().is_none_or(|s| s.catalog < 7));
        let admitted = match p.action.as_str() {
            "reserve" => {
                if p.response.is_some() || p.tokens == 0 {
                    return Err(bad("reserve needs positive tokens and no response"));
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
                state
                    .budget
                    .settle(&p.request, generation, response, p.tokens, p.usd_nanos)
                    .map_err(|e| bad(&e))?;
                true
            }
            "state" => true,
            _ => return Err(bad("unknown budget action")),
        };
        let (tokens, usd_nanos) = state.budget.totals();
        Ok(Commit {
            result: json!({"admitted":admitted, "exhausted":state.budget.exhausted(limits),
            "tokens":tokens, "usd_nanos":usd_nanos, "limits":limits,
            "uncertain_requests":state.budget.requests.iter().filter(|r| r.response.is_none()).count(),
            "pricing_gaps":state.budget.requests.iter().filter(|r| !r.priced).count(),
            "legacy_usage_gap":legacy_usage_gap,
            "unbounded_requests":state.budget.requests.iter().filter(|r| r.unbounded && r.response.is_none()).count()}),
            effect: None,
            post_pending: Vec::new(),
        })
    })
}
