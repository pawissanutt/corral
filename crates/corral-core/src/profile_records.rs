//! Store-owned version 1 records. Receipts alone never apply config or grant authority.
use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::{
    AutoProfileSelection, PROFILE_RECORD_VERSION, ProfileSetupRecord, ProfileSetupState,
    RepairHistory, ReviewSelectionContext, Store,
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectionRecord {
    sequence: u64,
    receipt: AutoProfileSelection,
}

impl Store {
    pub fn profile_setup_history(&self) -> Result<Vec<ProfileSetupRecord>> {
        let rows = self.record_rows("profile-setup-history")?;
        let records: Vec<ProfileSetupRecord> = rows
            .into_iter()
            .map(|row| serde_json::from_value(row.doc).context("decoding setup history"))
            .collect::<Result<_>>()?;
        let mut previous = ProfileSetupRecord::default();
        for (index, record) in records.iter().enumerate() {
            record.validate_integrity()?;
            previous.check_transition(record)?;
            check_setup_identities(&records[..index], record)?;
            previous = record.clone();
        }
        Ok(records)
    }

    pub fn profile_setup(&self) -> Result<ProfileSetupRecord> {
        let record: ProfileSetupRecord =
            self.record("profile-setup", "current")?.unwrap_or_default();
        record.validate_integrity()?;
        Ok(record)
    }

    /// Revision compare-and-append under one lock; identical redelivery is idempotent.
    /// Applied transitions belong exclusively to the transactional apply path.
    pub fn save_profile_setup(&self, record: &ProfileSetupRecord) -> Result<()> {
        self.check_runtime()?;
        let _lock = self.lock("profile-records")?;
        if self.profile_setup()? == *record {
            return Ok(());
        }
        ensure!(
            self.pending_profile_setup_intent()?.is_none(),
            "pending setup apply must be resumed before changing the proposal"
        );
        ensure!(
            record.state != ProfileSetupState::Applied,
            "Applied setup requires transactional apply"
        );
        self.append_profile_setup_locked(record)
    }

    /// Caller owns profile-records for the entire transaction.
    pub(crate) fn append_profile_setup_locked(&self, record: &ProfileSetupRecord) -> Result<()> {
        self.records_transaction(|| {
            let previous = self.profile_setup()?;
            if &previous == record {
                return Ok(());
            }
            previous.check_transition(record)?;
            if let Some(proposal) = &record.proposal
                && let Some(old) = self.record::<crate::ProfileSetupProposal>(
                    "profile-setup-proposals",
                    &proposal.id,
                )?
            {
                ensure!(old == *proposal, "conflicting setup proposal identity");
            }
            if let Some(approval) = &record.approval
                && let Some(old) = self.record::<crate::ProfileSetupApproval>(
                    "profile-setup-approvals",
                    &approval.id,
                )?
            {
                ensure!(old == *approval, "conflicting setup approval identity");
            }
            match record.state {
                ProfileSetupState::Applied | ProfileSetupState::Cancelled => {
                    record.validate_integrity()?
                }
                ProfileSetupState::NotStarted
                | ProfileSetupState::Gathering
                | ProfileSetupState::Proposed => {
                    record.validate(&self.config_layers()?.config()?)?
                }
            }
            self.save_record(
                "profile-setup-history",
                &format!("{:020}", record.revision),
                record,
            )?;
            if let Some(proposal) = &record.proposal {
                self.save_record("profile-setup-proposals", &proposal.id, proposal)?;
                if record.state == ProfileSetupState::Applied {
                    self.save_record("profile-setup-applied", &proposal.id, record)?;
                }
            }
            if let Some(approval) = &record.approval {
                self.save_record("profile-setup-approvals", &approval.id, approval)?;
            }
            self.save_record("profile-setup", "current", record)
        })
    }

    /// Historical stale receipts remain inspectable; callers validate before future routing.
    pub fn auto_profile_selections(&self) -> Result<Vec<AutoProfileSelection>> {
        let mut records: Vec<SelectionRecord> = self
            .record_rows("profile-selections")?
            .into_iter()
            .map(|row| serde_json::from_value(row.doc).context("decoding Auto selection"))
            .collect::<Result<_>>()?;
        ensure!(
            records
                .iter()
                .all(|r| r.receipt.version == PROFILE_RECORD_VERSION),
            "unsupported Auto selection version"
        );
        records.sort_by_key(|record| record.sequence);
        Ok(records.into_iter().map(|record| record.receipt).collect())
    }

    pub fn save_auto_profile_selection(
        &self,
        receipt: &AutoProfileSelection,
        review: Option<&ReviewSelectionContext<'_>>,
    ) -> Result<()> {
        self.check_runtime()?;
        let _lock = self.lock("profile-records")?;
        self.records_transaction(|| {
            if let Some(previous) =
                self.record::<SelectionRecord>("profile-selections", &receipt.id)?
            {
                ensure!(
                    previous.receipt == *receipt,
                    "conflicting Auto selection identity"
                );
                return Ok(());
            }
            let config = self.config_layers()?.config()?;
            let task = self.get(&receipt.task)?;
            receipt.validate(&config, &task)?;
            match (&receipt.review, review) {
                (Some(selection), Some(context)) => {
                    ensure!(context.task == &task, "stale review selection task");
                    selection.validate(&config, context)?;
                }
                (None, None) => {}
                (Some(_), None) | (None, Some(_)) => {
                    anyhow::bail!("review selection requires exact review context")
                }
            }
            let sequence = self
                .record::<u64>("profile-selection-sequence", "current")?
                .unwrap_or_default()
                .checked_add(1)
                .context("Auto selection sequence exhausted")?;
            self.save_record(
                "profile-selections",
                &receipt.id,
                &SelectionRecord {
                    sequence,
                    receipt: receipt.clone(),
                },
            )?;
            self.save_record("profile-selection-sequence", "current", &sequence)
        })
    }

    pub fn profile_repair_histories(&self) -> Result<BTreeMap<String, RepairHistory>> {
        self.record_rows("profile-repairs")?
            .into_iter()
            .map(|row| {
                Ok((
                    row.key,
                    serde_json::from_value(row.doc).context("decoding repair history")?,
                ))
            })
            .collect()
    }

    /// Histories are append-only, including across switches/restarts. No counter reset API.
    pub fn save_profile_repair_history(&self, task: &str, history: &RepairHistory) -> Result<()> {
        self.check_runtime()?;
        let _lock = self.lock("profile-records")?;
        self.records_transaction(|| {
            self.get(task)?;
            if let Some(previous) = self.record::<RepairHistory>("profile-repairs", task)? {
                ensure!(
                    history.policy() == previous.policy()
                        && history.current_profile() == previous.current_profile()
                        && history.stronger_profile() == previous.stronger_profile()
                        && history.events().starts_with(previous.events()),
                    "repair history cannot be rewritten"
                );
            }
            let names = self.config_layers()?.config()?.profile_names();
            ensure!(
                names.iter().any(|name| name == history.current_profile()),
                "unknown current repair profile"
            );
            if let Some(stronger) = history.stronger_profile() {
                ensure!(
                    names.iter().any(|name| name == stronger),
                    "unknown stronger repair profile"
                );
            }
            for event in history.events() {
                if let crate::RepairEvent::Started { receipt } = event {
                    ensure!(
                        names.contains(&receipt.profile),
                        "repair attempt references unknown profile"
                    );
                }
            }
            self.save_record("profile-repairs", task, history)
        })
    }
}

fn check_setup_identities(records: &[ProfileSetupRecord], next: &ProfileSetupRecord) -> Result<()> {
    for previous in records {
        if let (Some(old), Some(new)) = (&previous.proposal, &next.proposal) {
            ensure!(
                old.id != new.id || old == new,
                "conflicting setup proposal identity"
            );
        }
        if let (Some(old), Some(new)) = (&previous.approval, &next.approval) {
            ensure!(
                old.id != new.id || old == new,
                "conflicting setup approval identity"
            );
        }
    }
    Ok(())
}
