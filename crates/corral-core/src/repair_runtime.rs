//! Runtime projection of the opt-in repair history. Task mutations and record writes share
//! one version-checked unit; renewal archives, rather than erases, the old allowance.
use anyhow::{Context, Result, ensure};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::{FindingResolution, RepairHistory, RepairPolicy, Store};

pub const REPAIR_RUNTIME: &str = "repair-runtime";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub enum RepairTransition {
    Current { profile: String },
    Stronger { profile: String },
    Diagnose,
    Owner { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepairRenewal {
    pub id: String,
    pub previous_digest: String,
    pub reason: String,
    pub approved_by: String,
    pub at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepairDiagnosis {
    pub id: String,
    pub started: Timestamp,
    /// A proposal is advice, never a coding allowance or an acceptance decision.
    pub proposal: Option<String>,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepairAllowance {
    pub history: RepairHistory,
    pub diagnosis: Option<RepairDiagnosis>,
    pub renewal: Option<RepairRenewal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepairLedger {
    pub version: u32,
    pub allowances: Vec<RepairAllowance>,
}

impl RepairLedger {
    pub fn new(history: RepairHistory) -> Self {
        Self {
            version: 1,
            allowances: vec![RepairAllowance {
                history,
                diagnosis: None,
                renewal: None,
            }],
        }
    }

    pub fn active(&self) -> Result<&RepairAllowance> {
        ensure!(self.version == 1, "unsupported repair ledger version");
        self.allowances.last().context("repair allowance missing")
    }

    pub fn active_mut(&mut self) -> Result<&mut RepairAllowance> {
        ensure!(self.version == 1, "unsupported repair ledger version");
        self.allowances
            .last_mut()
            .context("repair allowance missing")
    }

    pub fn digest(&self) -> Result<String> {
        use sha2::{Digest, Sha256};
        Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(self)?)))
    }

    pub fn transition(&self) -> Result<RepairTransition> {
        let active = self.active()?;
        let h = &active.history;
        let c = h.counters()?;
        let RepairPolicy::V1 {
            current_profile_limit,
            stronger_profile_limit,
            overall_limit,
            pm_diagnosis_allowance,
        } = h.policy();
        if c.pm_diagnoses > 0 {
            return Ok(RepairTransition::Owner {
                reason: active
                    .diagnosis
                    .as_ref()
                    .and_then(|d| d.proposal.clone())
                    .unwrap_or_else(|| "PM diagnosis in progress; no coding retries remain".into()),
            });
        }
        if h.coding_blocker().is_none() && c.coding_repairs < *overall_limit {
            if c.current_profile_repairs < *current_profile_limit {
                return Ok(RepairTransition::Current {
                    profile: h.current_profile().into(),
                });
            }
            if c.stronger_profile_repairs < *stronger_profile_limit
                && let Some(profile) = h.stronger_profile()
            {
                return Ok(RepairTransition::Stronger {
                    profile: profile.into(),
                });
            }
        }
        if c.pm_diagnoses < *pm_diagnosis_allowance {
            Ok(RepairTransition::Diagnose)
        } else {
            Ok(RepairTransition::Owner { reason: h.coding_blocker().unwrap_or(
                "repair and PM diagnosis budgets exhausted; propose a bounded replan, deferral or explicit owner renewal").into() })
        }
    }

    pub fn unresolved(&self) -> Result<Vec<crate::FindingId>> {
        Ok(self
            .active()?
            .history
            .findings()
            .into_iter()
            .filter(|f| !matches!(f.resolution, FindingResolution::Resolved { .. }))
            .map(|f| f.finding_id.clone())
            .collect())
    }
}

impl Store {
    pub fn repair_ledger(&self, task: &str) -> Result<Option<RepairLedger>> {
        let ledger: Option<RepairLedger> = self.record(REPAIR_RUNTIME, task)?;
        if let Some(ledger) = &ledger {
            ledger.active()?;
        }
        Ok(ledger)
    }

    /// Joins the task transaction. Record version preconditions reject a concurrent renewal
    /// or observation; a duplicate start is accepted by RepairHistory, not counted again.
    pub fn save_repair_ledger(&self, task: &str, ledger: &RepairLedger) -> Result<()> {
        ledger.active()?;
        if let Some(previous) = self.repair_ledger(task)? {
            ensure!(
                ledger.allowances.len() == previous.allowances.len(),
                "new allowance requires explicit owner renewal"
            );
            let last = previous.allowances.len() - 1;
            ensure!(
                ledger.allowances[..last] == previous.allowances[..last],
                "repair archive changed"
            );
            let old = previous.active()?;
            let new = ledger.active()?;
            ensure!(
                old.renewal == new.renewal
                    && old.history.policy() == new.history.policy()
                    && old.history.current_profile() == new.history.current_profile()
                    && old.history.stronger_profile() == new.history.stronger_profile()
                    && new.history.events().starts_with(old.history.events()),
                "repair history cannot be rewritten"
            );
            if let Some(diagnosis) = &old.diagnosis {
                let next = new.diagnosis.as_ref().context("diagnosis removed")?;
                ensure!(
                    diagnosis.id == next.id
                        && diagnosis.started == next.started
                        && (diagnosis.proposal.is_none() || diagnosis == next),
                    "diagnosis cannot be rewritten"
                );
            }
        }
        self.save_record(REPAIR_RUNTIME, task, ledger)
    }

    /// Low-level persistence only: the ops owner-entry guard must run before this call.
    /// Approval binds the complete prior ledger; redelivery of the exact action is idempotent.
    pub fn renew_repair_allowance(
        &self,
        task: &str,
        renewal: RepairRenewal,
        policy: RepairPolicy,
        profile: &str,
        stronger: Option<String>,
    ) -> Result<RepairLedger> {
        let _lock = self.lock(&format!("repair-renew-{task}"))?;
        self.records_transaction(|| {
            let mut ledger = self
                .repair_ledger(task)?
                .context("no repair allowance to renew")?;
            if let Some(old) = ledger
                .allowances
                .iter()
                .find(|a| a.renewal.as_ref().is_some_and(|r| r.id == renewal.id))
            {
                ensure!(
                    old.renewal.as_ref() == Some(&renewal)
                        && old.history.policy() == &policy
                        && old.history.current_profile() == profile
                        && old.history.stronger_profile() == stronger.as_deref(),
                    "conflicting renewal identity"
                );
                return Ok(ledger);
            }
            ensure!(
                renewal.approved_by == "owner"
                    && !renewal.id.trim().is_empty()
                    && !renewal.reason.trim().is_empty(),
                "explicit owner renewal required"
            );
            ensure!(
                renewal.previous_digest == ledger.digest()?,
                "stale repair renewal"
            );
            ledger.allowances.push(RepairAllowance {
                history: RepairHistory::new(policy, profile, stronger)?,
                diagnosis: None,
                renewal: Some(renewal),
            });
            self.save_record(REPAIR_RUNTIME, task, &ledger)?;
            self.log_event(
                Some(task),
                "repair_allowance_renewed",
                serde_json::json!({
                    "renewal": ledger.active()?.renewal, "allowance": ledger.allowances.len()
                }),
            )?;
            Ok(ledger)
        })
    }
}
