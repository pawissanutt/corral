//! Versioned task-wide context for a new local allowance. The ledger verifies this
//! projection against its immutable predecessor; it is not another initial start.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::{
    RepairAttemptKind, RepairCounters, RepairEvent, RepairFinding, RepairLedger, RepairStartReceipt,
};

/// Marks accounting reconstructed from canonical evidence, not live launch receipts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepairProjection {
    pub version: u32,
    pub evidence: crate::blob_ref::BlobRef,
    pub lineage: String,
}

impl RepairProjection {
    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            self.version == 1,
            "unsupported historical repair projection"
        );
        crate::blob_ref::BlobHash::parse(&self.lineage)?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepairContext {
    pub version: u32,
    pub predecessor_version: u32,
    pub predecessor_digest: String,
    pub initial: RepairStartReceipt,
    pub findings: Vec<RepairFinding>,
    pub receipts: Vec<RepairStartReceipt>,
}

impl RepairContext {
    pub(crate) fn from_ledger(ledger: &RepairLedger) -> Result<Self> {
        let mut findings = Vec::new();
        let mut receipts = Vec::new();
        for allowance in &ledger.allowances {
            for event in allowance.history.events() {
                match event {
                    RepairEvent::Finding { finding } => {
                        findings.push(finding.clone());
                    }
                    RepairEvent::Started { receipt } => receipts.push(receipt.clone()),
                    _ => {}
                }
            }
        }
        let initials = receipts
            .iter()
            .filter(|r| r.kind == RepairAttemptKind::InitialImplementation)
            .collect::<Vec<_>>();
        ensure!(
            initials.len() == 1,
            "continuation requires exactly one task initial implementation"
        );
        let context = Self {
            version: 1,
            predecessor_version: ledger.version,
            predecessor_digest: ledger.digest()?,
            initial: initials[0].clone(),
            findings,
            receipts,
        };
        context.validate()?;
        Ok(context)
    }

    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            self.version == 1 && matches!(self.predecessor_version, 1 | 2),
            "unsupported repair context version"
        );
        crate::blob_ref::BlobHash::parse(&self.predecessor_digest)?;
        ensure!(
            self.initial.kind == RepairAttemptKind::InitialImplementation
                && self.initial.findings.is_empty(),
            "invalid task initial anchor"
        );
        ensure!(
            !self.initial.attempt_id.0.trim().is_empty() && !self.initial.profile.trim().is_empty(),
            "empty task initial anchor"
        );
        let mut ids = std::collections::BTreeSet::new();
        for receipt in &self.receipts {
            ensure!(
                ids.insert(&receipt.attempt_id),
                "duplicate predecessor attempt"
            );
        }
        ensure!(
            self.receipts
                .iter()
                .filter(|r| r.kind == RepairAttemptKind::InitialImplementation)
                .collect::<Vec<_>>()
                == vec![&self.initial],
            "inconsistent task initial anchor"
        );
        let mut findings = std::collections::BTreeSet::new();
        for finding in &self.findings {
            finding.validate()?;
            ensure!(
                findings.insert(&finding.observation_id),
                "duplicate inherited observation"
            );
            for fix in &finding.attempted_fixes {
                ensure!(
                    self.receipts.iter().any(|r| r.attempt_id == fix.attempt),
                    "inherited fix has no receipt"
                );
            }
        }
        Ok(())
    }
}

impl RepairLedger {
    /// Sum actual starts only; continuation anchors are not events and add no credit.
    pub fn task_counters(&self) -> Result<RepairCounters> {
        self.validate()?;
        let mut total = RepairCounters::default();
        for allowance in &self.allowances {
            let c = allowance.history.counters()?;
            for (sum, count) in [
                (
                    &mut total.initial_implementations,
                    c.initial_implementations,
                ),
                (
                    &mut total.availability_recoveries,
                    c.availability_recoveries,
                ),
                (
                    &mut total.current_profile_repairs,
                    c.current_profile_repairs,
                ),
                (
                    &mut total.stronger_profile_repairs,
                    c.stronger_profile_repairs,
                ),
                (&mut total.coding_repairs, c.coding_repairs),
                (&mut total.pm_diagnoses, c.pm_diagnoses),
            ] {
                *sum = sum
                    .checked_add(count)
                    .context("task repair counter overflow")?;
            }
        }
        Ok(total)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            matches!(self.version, 1 | 2) && !self.allowances.is_empty(),
            "unsupported or empty repair ledger"
        );
        ensure!(
            self.version == 1
                || self.allowances.len() > 1
                || self.allowances[0].history.projection().is_some(),
            "v2 ledger has no versioned origin"
        );
        let mut attempts = std::collections::BTreeSet::new();
        let mut observations = std::collections::BTreeSet::new();
        for (index, allowance) in self.allowances.iter().enumerate() {
            ensure!(
                self.version == 2
                    || (allowance.history.context().is_none()
                        && allowance.history.projection().is_none()),
                "v1 ledger cannot carry new repair context"
            );
            if let Some(projection) = allowance.history.projection() {
                projection.validate()?;
                ensure!(
                    index == 0,
                    "historical projection cannot replace a continuation"
                );
            }
            if let Some(context) = allowance.history.context() {
                ensure!(
                    self.version == 2 && index > 0,
                    "repair context has no predecessor"
                );
                let predecessor_version =
                    if index == 1 && self.allowances[0].history.projection().is_none() {
                        1
                    } else {
                        2
                    };
                ensure!(
                    context.predecessor_version == predecessor_version,
                    "predecessor ledger version contradicts archive"
                );
                let predecessor = Self {
                    version: context.predecessor_version,
                    allowances: self.allowances[..index].to_vec(),
                };
                ensure!(
                    *context == RepairContext::from_ledger(&predecessor)?,
                    "repair predecessor context differs from archive"
                );
                ensure!(
                    allowance
                        .renewal
                        .as_ref()
                        .is_some_and(|r| r.previous_digest == context.predecessor_digest),
                    "renewal does not bind predecessor"
                );
            } else if self.version == 2 {
                ensure!(index == 0, "continuation context missing");
            }
            for event in allowance.history.events() {
                match event {
                    RepairEvent::Started { receipt } => ensure!(
                        attempts.insert(&receipt.attempt_id) || self.version == 1,
                        "task attempt identity reused"
                    ),
                    RepairEvent::Finding { finding } => ensure!(
                        observations.insert(&finding.observation_id) || self.version == 1,
                        "task observation identity reused"
                    ),
                    _ => {}
                }
            }
        }
        if self.version == 2 {
            let initial_count = self
                .allowances
                .iter()
                .map(|a| a.history.counters().map(|c| c.initial_implementations))
                .collect::<Result<Vec<_>>>()?
                .into_iter()
                .sum::<u32>();
            ensure!(
                initial_count == 1,
                "task initial implementation is not unique"
            );
        }
        Ok(())
    }
}
