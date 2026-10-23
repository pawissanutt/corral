//! The owner's revision-guarded member-gate amendment on a tracker. `member_gates` stays outside
//! the operating contract revision and the approved scope, so frozen target bindings still hold.
use anyhow::{Context, Result, ensure};

use super::{ScopeWrite, Store};
use crate::{MemberGateRecord, ScopeGate, Task};

/// One amendment of `member`'s gate, guarded by the member's current record revision (0 when
/// it has none).
#[derive(Debug, Clone)]
pub struct MemberGateAmendment {
    pub member: String,
    pub expected_revision: u64,
    pub gate: ScopeGate,
    pub reason: String,
    pub by: String,
}

#[derive(Debug, Clone)]
pub struct MemberGateAmended {
    pub tracker: Task,
    pub record: MemberGateRecord,
    /// The member run whose completion command snapshot names the new review-phase commands.
    pub reconciled_run: Option<String>,
}

impl Store {
    /// Amend one member's gate on `tracker`. Under the board lock, `validate` checks the fresh
    /// tracker and must not write; `reconcile` then adjusts the member's current run under the
    /// member's lock and must be idempotent, because a crash before the tracker save leaves its
    /// change behind and the owner's retry runs it again. The tracker save re-checks the revision.
    ///
    /// # Errors
    /// Rejects non-trackers, stale or exhausted revisions, a blank reason, an invalid gate, a
    /// failed validation or reconciliation, and persistence failures.
    pub fn amend_member_gate(
        &self,
        tracker: &str,
        amendment: &MemberGateAmendment,
        validate: impl FnOnce(&Store, &Task) -> Result<()>,
        reconcile: impl FnOnce(&Store, &mut Task) -> Result<Option<String>>,
    ) -> Result<MemberGateAmended> {
        let _board = self.lock("dispatch")?;
        let tracker = Self::normalize_id(tracker)?;
        let member = Self::normalize_id(&amendment.member)?;
        let fresh = self.reopen_unobserved()?;
        let current = fresh.get(&tracker)?;
        ensure!(current.tracker, "{} is not a tracker Plan", current.id);
        check_revision(&current, &member, amendment.expected_revision)?;
        ensure!(
            !amendment.reason.trim().is_empty(),
            "member-gate amendment needs a reason"
        );
        amendment.gate.validate()?;
        validate(&fresh, &current)?;
        let reconciled_run = {
            let _member = fresh.lock(&member)?;
            let before = fresh.get(&member)?;
            let mut task = before.clone();
            let run = reconcile(&fresh, &mut task)?;
            if task != before {
                fresh.save_inner(&task)?;
            }
            run
        };
        let _tracker = fresh.lock(&tracker)?;
        let mut task = fresh.get(&tracker)?;
        check_revision(&task, &member, amendment.expected_revision)?;
        let revision = amendment
            .expected_revision
            .checked_add(1)
            .context("member-gate revision exhausted")?;
        let mut history = Vec::new();
        if let Some(previous) = task.member_gates.get(&member) {
            history.extend(previous.history.iter().cloned());
            history.push(MemberGateRecord {
                history: Vec::new(),
                ..previous.clone()
            });
        }
        let record = MemberGateRecord {
            revision,
            gate: amendment.gate.clone(),
            reason: amendment.reason.clone(),
            by: amendment.by.clone(),
            at: crate::clock::now(),
            history,
        };
        task.member_gates.insert(member.clone(), record.clone());
        let saved = fresh.save_task(&task, ScopeWrite::MemberGate)?;
        fresh.log_event(
            Some(&saved.id),
            "plan_member_gate_amended",
            serde_json::json!({
                "tracker": saved.id, "member": member,
                "from_revision": amendment.expected_revision, "revision": revision,
                "gate": record.gate, "reason": record.reason, "actor": record.by,
                "reconciled_run": reconciled_run,
            }),
        )?;
        Ok(MemberGateAmended {
            tracker: saved,
            record,
            reconciled_run,
        })
    }
}

fn check_revision(tracker: &Task, member: &str, expected: u64) -> Result<()> {
    let current = tracker.member_gates.get(member).map_or(0, |r| r.revision);
    ensure!(
        current == expected,
        "stale member-gate amendment: expected revision {expected}, current revision {current} of {member} on {}",
        tracker.id
    );
    Ok(())
}
