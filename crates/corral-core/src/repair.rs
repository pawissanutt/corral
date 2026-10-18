//! Opt-in repair contracts. Persist a successful mutation before launching its attempt.
//! Absence of `Option<RepairPolicy>` means legacy `routing.escalate_after` and
//! `review.max_rounds` still apply; this module does not reinterpret those fields.

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};

/// No `Default`: adopting these bounds requires an explicit approved policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "version", deny_unknown_fields)]
pub enum RepairPolicy {
    #[serde(rename = "1")]
    V1 {
        current_profile_limit: u32,
        stronger_profile_limit: u32,
        overall_limit: u32,
        pm_diagnosis_allowance: u32,
    },
}

impl RepairPolicy {
    /// Suggested setup proposal only, never a legacy-config default.
    pub const fn approved_starter_policy() -> Self {
        Self::V1 {
            current_profile_limit: 2,
            stronger_profile_limit: 2,
            overall_limit: 4,
            pm_diagnosis_allowance: 1,
        }
    }

    pub fn validate(&self) -> Result<()> {
        let Self::V1 {
            current_profile_limit,
            stronger_profile_limit,
            overall_limit,
            pm_diagnosis_allowance,
        } = self;
        let stages = u64::from(*current_profile_limit) + u64::from(*stronger_profile_limit);
        ensure!(
            u64::from(*overall_limit) <= stages,
            "overall limit exceeds stage allowances"
        );
        ensure!(
            *pm_diagnosis_allowance <= 1,
            "only one PM diagnosis is supported"
        );
        Ok(())
    }
}

/// Stable identities are caller-supplied, not regenerated on each sync.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FindingId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RepairAttemptId(pub String);

/// Exact machine identity, independent of explanatory failure prose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FailureIdentity {
    pub criterion_or_test: String,
    pub affected_contract_or_location: String,
    /// Stable machine code/signature, not a model's paraphrase of the error.
    pub failure_code: String,
}

impl FailureIdentity {
    pub fn validate(&self) -> Result<()> {
        nonempty(&self.criterion_or_test)?;
        nonempty(&self.affected_contract_or_location)?;
        nonempty(&self.failure_code)
    }

    /// SHA-256 of length-delimited exact UTF-8 fields, with a versioned domain.
    /// No fuzzy normalization: semantic equivalence must be recorded separately.
    pub fn fingerprint(&self) -> String {
        let mut hash = Sha256::new();
        hash.update(b"corral-repair-failure-v1\0");
        for field in [
            &self.criterion_or_test,
            &self.affected_contract_or_location,
            &self.failure_code,
        ] {
            hash.update(field.len().to_string().as_bytes());
            hash.update(b":");
            hash.update(field.as_bytes());
        }
        format!("{:x}", hash.finalize())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FindingMatch {
    New,
    Exact {
        finding: FindingId,
    },
    Semantic {
        finding: FindingId,
        reason: String,
    },
    Uncertain {
        candidates: Vec<FindingId>,
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum FindingResolution {
    Unresolved,
    Resolved {
        evidence: Vec<String>,
    },
    Uncertain {
        reason: String,
        evidence: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptedFix {
    pub attempt: RepairAttemptId,
    pub change: String,
    pub evidence: Vec<String>,
}

/// Immutable observation; a later observation may retain the same stable finding ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepairFinding {
    pub observation_id: String,
    pub finding_id: FindingId,
    pub identity: FailureIdentity,
    pub fingerprint: String,
    pub observed_failure: String,
    pub matching: FindingMatch,
    pub attempted_fixes: Vec<AttemptedFix>,
    pub resolution: FindingResolution,
}

impl RepairFinding {
    pub fn validate(&self) -> Result<()> {
        nonempty(&self.observation_id)?;
        nonempty(&self.finding_id.0)?;
        self.identity.validate()?;
        ensure!(
            self.fingerprint == self.identity.fingerprint(),
            "invalid failure fingerprint"
        );
        nonempty(&self.observed_failure)?;
        for fix in &self.attempted_fixes {
            nonempty(&fix.attempt.0)?;
            nonempty(&fix.change)?;
            evidence(&fix.evidence, false)?;
        }
        match &self.matching {
            FindingMatch::New => {}
            FindingMatch::Exact { finding } => nonempty(&finding.0)?,
            FindingMatch::Semantic { finding, reason } => {
                nonempty(&finding.0)?;
                nonempty(reason)?;
            }
            FindingMatch::Uncertain { candidates, reason } => {
                ensure!(!candidates.is_empty(), "uncertain match needs candidates");
                for candidate in candidates {
                    nonempty(&candidate.0)?;
                }
                nonempty(reason)?;
            }
        }
        match &self.resolution {
            FindingResolution::Unresolved => {}
            FindingResolution::Resolved { evidence: items } => evidence(items, true)?,
            FindingResolution::Uncertain {
                reason,
                evidence: items,
            } => {
                nonempty(reason)?;
                evidence(items, false)?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RepairAttemptKind {
    InitialImplementation,
    CurrentProfileRepair,
    StrongerProfileRepair,
    /// Recovery continues an existing attempt, never creates another coding allowance.
    AvailabilityRecovery {
        interrupted_attempt: RepairAttemptId,
    },
    PmDiagnosis,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepairStartReceipt {
    pub attempt_id: RepairAttemptId,
    pub profile: String,
    pub kind: RepairAttemptKind,
    pub findings: Vec<FindingId>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepairCounters {
    pub initial_implementations: u32,
    pub availability_recoveries: u32,
    pub current_profile_repairs: u32,
    pub stronger_profile_repairs: u32,
    pub coding_repairs: u32,
    pub pm_diagnoses: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiptApplication {
    Recorded,
    Duplicate,
}

/// Ordered history is the durable source of truth; counters are derived, never reset.
/// A history snapshots one approved policy and profile pair. New owner allowances
/// require a separate recorded policy action in the caller, not mutation of this record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepairHistory {
    version: u32,
    policy: RepairPolicy,
    current_profile: String,
    stronger_profile: Option<String>,
    events: Vec<RepairEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub enum RepairEvent {
    Finding { finding: RepairFinding },
    Started { receipt: RepairStartReceipt },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryWire {
    version: u32,
    policy: RepairPolicy,
    current_profile: String,
    stronger_profile: Option<String>,
    events: Vec<RepairEvent>,
}

impl<'de> Deserialize<'de> for RepairHistory {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let wire = HistoryWire::deserialize(deserializer)?;
        let restore = || -> Result<Self> {
            ensure!(wire.version == 1, "unsupported repair history version");
            let mut history = Self::new(wire.policy, &wire.current_profile, wire.stronger_profile)?;
            for event in wire.events {
                history.apply(event)?;
            }
            Ok(history)
        };
        restore().map_err(serde::de::Error::custom)
    }
}

impl RepairHistory {
    pub fn new(
        policy: RepairPolicy,
        current_profile: &str,
        stronger_profile: Option<String>,
    ) -> Result<Self> {
        policy.validate()?;
        nonempty(current_profile)?;
        if let Some(stronger) = &stronger_profile {
            nonempty(stronger)?;
            ensure!(
                stronger != current_profile,
                "stronger profile must be distinct"
            );
        }
        Ok(Self {
            version: 1,
            policy,
            current_profile: current_profile.to_owned(),
            stronger_profile,
            events: Vec::new(),
        })
    }

    pub fn policy(&self) -> &RepairPolicy {
        &self.policy
    }

    pub fn current_profile(&self) -> &str {
        &self.current_profile
    }

    pub fn stronger_profile(&self) -> Option<&str> {
        self.stronger_profile.as_deref()
    }

    pub fn events(&self) -> &[RepairEvent] {
        &self.events
    }

    pub fn counters(&self) -> Result<RepairCounters> {
        let mut counts = RepairCounters::default();
        for event in &self.events {
            if let RepairEvent::Started { receipt } = event {
                match receipt.kind {
                    RepairAttemptKind::InitialImplementation => {
                        increment(&mut counts.initial_implementations)?
                    }
                    RepairAttemptKind::CurrentProfileRepair => {
                        increment(&mut counts.current_profile_repairs)?;
                        increment(&mut counts.coding_repairs)?;
                    }
                    RepairAttemptKind::StrongerProfileRepair => {
                        increment(&mut counts.stronger_profile_repairs)?;
                        increment(&mut counts.coding_repairs)?;
                    }
                    RepairAttemptKind::AvailabilityRecovery { .. } => {
                        increment(&mut counts.availability_recoveries)?
                    }
                    RepairAttemptKind::PmDiagnosis => increment(&mut counts.pm_diagnoses)?,
                }
            }
        }
        Ok(counts)
    }

    /// Append once or accept an identical redelivery. Conflicting IDs fail atomically.
    /// The caller must durably save the result before sending a launch command.
    pub fn apply(&mut self, event: RepairEvent) -> Result<ReceiptApplication> {
        match &event {
            RepairEvent::Finding { finding } => {
                if let Some(previous) = self.events.iter().find_map(|event| match event {
                    RepairEvent::Finding { finding: old }
                        if old.observation_id == finding.observation_id =>
                    {
                        Some(old)
                    }
                    RepairEvent::Finding { .. } | RepairEvent::Started { .. } => None,
                }) {
                    ensure!(previous == finding, "conflicting finding observation ID");
                    return Ok(ReceiptApplication::Duplicate);
                }
                self.validate_finding(finding)?;
            }
            RepairEvent::Started { receipt } => {
                if let Some(previous) = self.receipt(&receipt.attempt_id) {
                    ensure!(previous == receipt, "conflicting attempt ID");
                    return Ok(ReceiptApplication::Duplicate);
                }
                self.validate_start(receipt)?;
            }
        }
        self.events.push(event);
        Ok(ReceiptApplication::Recorded)
    }

    fn receipt(&self, id: &RepairAttemptId) -> Option<&RepairStartReceipt> {
        self.events.iter().find_map(|event| match event {
            RepairEvent::Started { receipt } if &receipt.attempt_id == id => Some(receipt),
            RepairEvent::Finding { .. } | RepairEvent::Started { .. } => None,
        })
    }

    fn latest_finding(&self, id: &FindingId) -> Option<&RepairFinding> {
        self.events.iter().rev().find_map(|event| match event {
            RepairEvent::Finding { finding } if &finding.finding_id == id => Some(finding),
            RepairEvent::Finding { .. } | RepairEvent::Started { .. } => None,
        })
    }

    fn validate_finding(&self, finding: &RepairFinding) -> Result<()> {
        finding.validate()?;
        let target = match &finding.matching {
            FindingMatch::New => {
                ensure!(
                    self.latest_finding(&finding.finding_id).is_none(),
                    "finding ID already exists"
                );
                None
            }
            FindingMatch::Exact { finding: id } => {
                let previous = self.latest_finding(id).context("unknown exact finding")?;
                ensure!(
                    previous.identity == finding.identity,
                    "exact finding identity differs"
                );
                Some(id)
            }
            FindingMatch::Semantic { finding: id, .. } => {
                ensure!(
                    self.latest_finding(id).is_some(),
                    "unknown semantic finding"
                );
                Some(id)
            }
            FindingMatch::Uncertain { candidates, .. } => {
                ensure!(
                    self.latest_finding(&finding.finding_id).is_none(),
                    "uncertain match cannot claim an existing identity"
                );
                for id in candidates {
                    ensure!(
                        self.latest_finding(id).is_some(),
                        "unknown uncertain candidate"
                    );
                }
                None
            }
        };
        if let Some(id) = target {
            ensure!(
                *id == finding.finding_id,
                "confirmed repeat must retain stable finding ID"
            );
        }
        for fix in &finding.attempted_fixes {
            let receipt = self
                .receipt(&fix.attempt)
                .context("fix references unknown attempt")?;
            ensure!(
                matches!(
                    receipt.kind,
                    RepairAttemptKind::CurrentProfileRepair
                        | RepairAttemptKind::StrongerProfileRepair
                ),
                "fix must reference a coding repair"
            );
            ensure!(
                receipt.findings.contains(&finding.finding_id),
                "fix attempt did not address finding"
            );
        }
        Ok(())
    }

    fn validate_start(&self, receipt: &RepairStartReceipt) -> Result<()> {
        nonempty(&receipt.attempt_id.0)?;
        nonempty(&receipt.profile)?;
        let counts = self.counters()?;
        let RepairPolicy::V1 {
            current_profile_limit,
            stronger_profile_limit,
            overall_limit,
            pm_diagnosis_allowance,
        } = &self.policy;
        for (index, id) in receipt.findings.iter().enumerate() {
            ensure!(
                !receipt.findings[..index].contains(id),
                "duplicate finding reference"
            );
            let finding = self
                .latest_finding(id)
                .context("attempt references unknown finding")?;
            ensure!(
                !matches!(finding.resolution, FindingResolution::Resolved { .. }),
                "attempt references resolved finding"
            );
        }
        let repairs_exhausted = counts.coding_repairs >= *overall_limit
            || (counts.current_profile_repairs >= *current_profile_limit
                && (self.stronger_profile.is_none()
                    || counts.stronger_profile_repairs >= *stronger_profile_limit));
        if matches!(
            receipt.kind,
            RepairAttemptKind::CurrentProfileRepair | RepairAttemptKind::StrongerProfileRepair
        ) {
            ensure!(
                counts.initial_implementations == 1,
                "repair requires initial implementation"
            );
            ensure!(
                counts.pm_diagnoses == 0,
                "diagnosis cannot grant coding allowance"
            );
            ensure!(!receipt.findings.is_empty(), "repair requires a finding");
            ensure!(
                counts.coding_repairs < *overall_limit,
                "overall repair limit exhausted"
            );
        }
        match &receipt.kind {
            RepairAttemptKind::InitialImplementation => {
                ensure!(
                    counts == RepairCounters::default(),
                    "initial implementation already started"
                );
                ensure!(
                    receipt.profile == self.current_profile,
                    "incorrect initial profile"
                );
                ensure!(
                    receipt.findings.is_empty(),
                    "initial implementation is not a repair"
                );
            }
            RepairAttemptKind::CurrentProfileRepair => {
                ensure!(
                    receipt.profile == self.current_profile,
                    "incorrect current profile"
                );
                ensure!(
                    counts.stronger_profile_repairs == 0,
                    "cannot return to current stage"
                );
                ensure!(
                    counts.current_profile_repairs < *current_profile_limit,
                    "current stage exhausted"
                );
            }
            RepairAttemptKind::StrongerProfileRepair => {
                ensure!(
                    self.stronger_profile.as_ref() == Some(&receipt.profile),
                    "no matching configured stronger profile"
                );
                ensure!(
                    counts.current_profile_repairs >= *current_profile_limit,
                    "current stage not exhausted"
                );
                ensure!(
                    counts.stronger_profile_repairs < *stronger_profile_limit,
                    "stronger stage exhausted"
                );
            }
            RepairAttemptKind::AvailabilityRecovery {
                interrupted_attempt,
            } => {
                ensure!(
                    counts.pm_diagnoses == 0,
                    "cannot recover coding after diagnosis"
                );
                let original = self
                    .receipt(interrupted_attempt)
                    .context("unknown interrupted attempt")?;
                let latest_coding_attempt =
                    self.events.iter().rev().find_map(|event| match event {
                        RepairEvent::Started { receipt }
                            if matches!(
                                receipt.kind,
                                RepairAttemptKind::InitialImplementation
                                    | RepairAttemptKind::CurrentProfileRepair
                                    | RepairAttemptKind::StrongerProfileRepair
                            ) =>
                        {
                            Some(&receipt.attempt_id)
                        }
                        RepairEvent::Finding { .. } | RepairEvent::Started { .. } => None,
                    });
                ensure!(
                    latest_coding_attempt == Some(interrupted_attempt),
                    "availability recovery references a superseded coding attempt"
                );
                ensure!(
                    matches!(
                        original.kind,
                        RepairAttemptKind::InitialImplementation
                            | RepairAttemptKind::CurrentProfileRepair
                            | RepairAttemptKind::StrongerProfileRepair
                    ),
                    "availability recovery must reference implementation or repair"
                );
                ensure!(
                    receipt.profile == original.profile && receipt.findings == original.findings,
                    "recovery must retain original profile and findings"
                );
            }
            RepairAttemptKind::PmDiagnosis => {
                ensure!(
                    counts.initial_implementations == 1,
                    "diagnosis requires initial implementation"
                );
                ensure!(repairs_exhausted, "coding repair allowance remains");
                ensure!(
                    counts.pm_diagnoses < *pm_diagnosis_allowance,
                    "PM diagnosis allowance exhausted"
                );
                ensure!(
                    !receipt.findings.is_empty(),
                    "diagnosis requires unresolved findings"
                );
            }
        }
        Ok(())
    }
}

fn increment(value: &mut u32) -> Result<()> {
    *value = value.checked_add(1).context("repair counter overflow")?;
    Ok(())
}

fn nonempty(value: &str) -> Result<()> {
    ensure!(
        !value.trim().is_empty(),
        "repair contract field must not be empty"
    );
    Ok(())
}

fn evidence(items: &[String], required: bool) -> Result<()> {
    ensure!(
        !required || !items.is_empty(),
        "resolution requires evidence"
    );
    for item in items {
        nonempty(item)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "repair_tests.rs"]
mod tests;
