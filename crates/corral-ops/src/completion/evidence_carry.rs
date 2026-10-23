//! Evidence carry: criterion records bound to an earlier identity stand for a carried gate or
//! verdict. The ledger is never rewritten; one relation beside it records the carry.
use anyhow::Result;
use corral_core::{CandidateIdentity, CarryMethod, EvidenceCarry, EvidenceRecord, Run, Task};
use jiff::Timestamp;

use super::{Admission, obligation_failures};
use crate::{Ctx, verification};

pub const EVIDENCE_CARRY_VERSION: u32 = 1;

/// The identity the carried records are bound to and the start of their admission window.
pub struct CarrySource<'a> {
    pub identity: &'a CandidateIdentity,
    pub window_start: Timestamp,
}

/// An evidence obligation failed against the source; nothing may be carried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CarryRefused {
    pub failures: Vec<String>,
}

impl std::fmt::Display for CarryRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "evidence_carry_refused: {}", self.failures.join(", "))
    }
}

impl std::error::Error for CarryRefused {}

/// SHA-256 of the canonical JSON of the records bound to `from`, in ledger order.
pub(crate) fn records_sha256(run: &Run, from: &CandidateIdentity) -> Result<String> {
    let records: Vec<&EvidenceRecord> = run
        .completion
        .evidence
        .iter()
        .filter(|e| e.identity == *from)
        .collect();
    Ok(verification::digest(&serde_json::to_vec(&records)?))
}

/// The stored relation while it stands for `to`: same task and run, records unchanged.
pub(crate) fn live<'r>(
    task: &Task,
    run: &'r Run,
    to: &CandidateIdentity,
) -> Result<Option<&'r EvidenceCarry>> {
    let Some(carry) = run.completion.evidence_carry.as_deref() else {
        return Ok(None);
    };
    if carry.version != EVIDENCE_CARRY_VERSION
        || carry.to != *to
        || carry.from.task != task.id
        || carry.from.run != run.id
    {
        return Ok(None);
    }
    Ok((records_sha256(run, &carry.from)? == carry.records_sha256).then_some(carry))
}

/// Validate every evidence obligation against `source` before relating its records to `to`.
/// A live relation whose `to` is the source (a chained carry) keeps its `from` and window.
/// Returns `None` when there is no manifest, or no obligation and no record bound to the
/// source; refuses with [`CarryRefused`] and today's failure names otherwise.
pub fn establish(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    source: CarrySource<'_>,
    to: &CandidateIdentity,
    method: CarryMethod,
) -> Result<Option<EvidenceCarry>> {
    if task.completion.is_none() {
        return Ok(None);
    }
    let (from, window_start) = match live(task, run, source.identity)? {
        Some(chained) => (chained.from.clone(), chained.window_start),
        None => (source.identity.clone(), source.window_start),
    };
    let at = corral_core::clock::now();
    let window = Admission {
        identity: &from,
        start: window_start,
        end: Some(at),
    };
    let failures = obligation_failures(ctx, task, run, &window, None)?;
    if !failures.is_empty() {
        return Err(CarryRefused { failures }.into());
    }
    if !run.completion.evidence.iter().any(|e| e.identity == from) {
        return Ok(None);
    }
    Ok(Some(EvidenceCarry {
        version: EVIDENCE_CARRY_VERSION,
        method,
        records_sha256: records_sha256(run, &from)?,
        from,
        window_start,
        to: to.clone(),
        at,
    }))
}

#[cfg(test)]
#[path = "evidence_carry_tests.rs"]
mod tests;
