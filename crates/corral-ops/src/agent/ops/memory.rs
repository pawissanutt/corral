//! Single-phase memory operations. File IO runs inside the fenced container transaction;
//! a crash before its receipt remains unknown, never automatically reconciled.
use super::super::{
    admission::{self, Admission, Mode},
    call::{AgentOp, CallResponse, RejectCode, Rejection},
    envelope::Envelope,
    receipts::{self, Commit},
};
use crate::{
    Ctx,
    memory::{MemoryActor, authorize},
};
use anyhow::Result;
use corral_core::{
    agent::AgentRole,
    memory::{MemoryEntry, MemoryScope, MemorySelector},
};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct List {
    scope: MemoryScope,
    #[serde(default)]
    offset: usize,
    #[serde(default = "list_limit")]
    limit: usize,
}
fn list_limit() -> usize {
    corral_core::memory::PAGE_ENTRIES
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Read {
    selector: MemorySelector,
    #[serde(default)]
    offset: usize,
    #[serde(default = "read_limit")]
    limit: usize,
}
fn read_limit() -> usize {
    corral_core::memory::READ_BYTES
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Put {
    selector: MemorySelector,
    entry: MemoryEntry,
    expected_hash: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Remove {
    selector: MemorySelector,
    expected_hash: String,
}

enum Input {
    List(List),
    Read(Read),
    Put(Put),
    Remove(Remove),
}
impl Input {
    fn scope(&self) -> &MemoryScope {
        match self {
            Self::List(p) => &p.scope,
            Self::Read(p) => &p.selector.scope,
            Self::Put(p) => &p.selector.scope,
            Self::Remove(p) => &p.selector.scope,
        }
    }
}
fn bad(error: impl std::fmt::Display) -> anyhow::Error {
    Rejection::new(RejectCode::BadRequest, error.to_string()).into()
}
pub(crate) fn call(ctx: &Ctx, envelope: &Envelope, op: AgentOp) -> Result<CallResponse> {
    let input = match op {
        AgentOp::MemoryList => Input::List(super::payload(envelope, "scope, offset?, limit?")?),
        AgentOp::MemoryRead => Input::Read(super::payload(envelope, "selector, offset?, limit?")?),
        AgentOp::MemoryPut => {
            // Creation requires an explicit null, not an accidentally omitted precondition.
            if envelope.payload.get("expected_hash").is_none() {
                return Err(bad("expected_hash is required (null for explicit absence)"));
            }
            Input::Put(super::payload(envelope, "selector, entry, expected_hash")?)
        }
        AgentOp::MemoryRemove => {
            Input::Remove(super::payload(envelope, "selector, expected_hash")?)
        }
        _ => return Err(bad("not a memory operation")),
    };
    let container = admission::container_of(&envelope.identity);
    let admitted = match admission::admit(ctx, envelope, Mode::Mutation { settles_turn: None })? {
        Admission::Replay(stored) => return receipts::respond(ctx, &container, *stored, true),
        Admission::New(admitted) => admitted,
    };
    receipts::commit(ctx, &admitted, |state, _| {
        // append() also checks this, but memory file effects cannot be rolled back if
        // the container's receipt capacity is already exhausted.
        if state.receipts.len() >= corral_core::agent::RECEIPTS_MAX {
            return Err(Rejection::new(
                RejectCode::ReceiptLimit,
                "memory requires room for its receipt before file IO",
            )
            .into());
        }
        let actor = match state.binding.role {
            AgentRole::Pm => MemoryActor::Pm(&state.binding.project),
            AgentRole::Assistant => MemoryActor::Assistant,
            AgentRole::Worker => MemoryActor::Worker(&state.binding.project),
            _ => {
                return Err(Rejection::new(
                    RejectCode::OpNotAllowed,
                    "memory is unavailable for this role",
                )
                .into());
            }
        };
        let write = matches!(input, Input::Put(_) | Input::Remove(_));
        authorize(&ctx.store, &actor, input.scope(), write).map_err(|_| {
            Rejection::new(
                RejectCode::OutOfScope,
                "memory scope is unavailable to this caller",
            )
        })?;
        let author = format!(
            "{}:{}:generation:{}",
            state.binding.role.as_str(),
            container.key(),
            envelope.identity.generation
        );
        receipts::failpoint("memory_before_io");
        let result = match input {
            Input::List(p) => serde_json::to_value(
                ctx.store
                    .memory_list(&p.scope, p.offset, p.limit)
                    .map_err(bad)?,
            )?,
            Input::Read(p) => serde_json::to_value(
                ctx.store
                    .memory_read(&p.selector, p.offset, p.limit)
                    .map_err(bad)?,
            )?,
            Input::Put(p) => serde_json::to_value(
                ctx.store
                    .memory_put(
                        &p.selector,
                        p.entry,
                        p.expected_hash.as_deref(),
                        &author,
                        jiff::Timestamp::now(),
                    )
                    .map_err(bad)?,
            )?,
            Input::Remove(p) => {
                ctx.store
                    .memory_remove(&p.selector, &p.expected_hash)
                    .map_err(bad)?;
                json!({"removed": true})
            }
        };
        receipts::failpoint("memory_after_io_before_receipt");
        Ok(Commit {
            result,
            effect: None,
            post_pending: vec![],
        })
    })
}

#[cfg(test)]
#[path = "memory_tests.rs"]
mod tests;
