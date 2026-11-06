//! PM and assistant turns (T-679): the request disposition a turn ended with and corral's
//! bounded continuation decision, both recorded in the transaction that ends the turn. Only the
//! session's own todo plan decides; the board, its events and other sessions never do.
use corral_core::agent::request::{Continuation, Disposition, RequestLedger, TurnEnd};
use corral_core::agent::{AgentRole, AgentRunState, TurnClass};
use jiff::Timestamp;
use serde_json::{Value, json};

/// The `request` member of a PM or assistant `turn_ended` receipt, and whether continuation
/// stopped at this turn. None for other roles and for a generation whose todo is off.
pub(crate) fn turn_ended(
    state: &mut AgentRunState,
    index: usize,
    aborted: bool,
    pending_commands: bool,
    now: Timestamp,
) -> Option<(Value, bool)> {
    if !matches!(state.binding.role, AgentRole::Pm | AgentRole::Assistant) {
        return None;
    }
    let turn = state.turns.get(index)?;
    if !state
        .generation(turn.generation)
        .is_some_and(|g| g.capability("todo"))
    {
        return None;
    }
    let end = if aborted || turn.class == Some(TurnClass::Cancelled) {
        TurnEnd::Cancelled
    } else if turn.class == Some(TurnClass::BudgetExhausted) {
        TurnEnd::BudgetExhausted
    } else if turn.provider_error.is_some() {
        TurnEnd::ProviderFailed
    } else if pending_commands {
        TurnEnd::WaitingCommand
    } else {
        TurnEnd::Normal
    };
    let (origin, at) = (turn.origin.clone(), (turn.generation, turn.seq));
    let mut requests: RequestLedger = state.todo_requests.clone().unwrap_or_default();
    let before = requests.continued.clone();
    let record = requests.turn_ended(state.todo.as_ref(), end, &origin, at, now);
    let stopped =
        matches!(record.continuation, Continuation::Stopped { .. }) && requests.continued != before;
    let next = match &record.disposition {
        Disposition::Actionable { next } => state
            .todo
            .iter()
            .flat_map(|p| p.steps())
            .find(|s| s.id == *next)
            .map(|s| json!({"id": s.id, "label": s.label})),
        _ => None,
    };
    let value = json!({
        "plan": record.plan, "revision": record.revision, "end": record.end,
        "disposition": record.disposition, "continuation": record.continuation, "next": next,
    });
    // A session that never kept a plan gets no ledger written for its turns.
    if state.todo.is_some() || state.todo_requests.is_some() {
        state.todo_requests = Some(requests);
    }
    Some((value, stopped))
}

#[cfg(all(test, unix))]
#[path = "requests_tests.rs"]
mod tests;
