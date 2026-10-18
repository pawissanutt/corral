//! Store-owned version 1 journals. Receipts alone never apply config or grant authority.
use std::{collections::BTreeMap, fs, path::Path};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{
    AutoProfileSelection, PROFILE_RECORD_VERSION, ProfileSetupRecord, ProfileSetupState,
    RepairHistory, ReviewSelectionContext, Store,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SetupJournal {
    version: u32,
    records: Vec<ProfileSetupRecord>,
}

impl Default for SetupJournal {
    fn default() -> Self {
        Self {
            version: PROFILE_RECORD_VERSION,
            records: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectionJournal {
    version: u32,
    receipts: Vec<AutoProfileSelection>,
}

impl Default for SelectionJournal {
    fn default() -> Self {
        Self {
            version: PROFILE_RECORD_VERSION,
            receipts: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RepairJournal {
    version: u32,
    histories: BTreeMap<String, RepairHistory>,
}

impl Default for RepairJournal {
    fn default() -> Self {
        Self {
            version: PROFILE_RECORD_VERSION,
            histories: BTreeMap::new(),
        }
    }
}

impl Store {
    pub fn profile_setup_history(&self) -> Result<Vec<ProfileSetupRecord>> {
        let journal: SetupJournal = read_journal(&self.state_dir().join("profile-setup.json"))?;
        ensure!(
            journal.version == PROFILE_RECORD_VERSION,
            "unsupported setup journal version"
        );
        let mut previous = ProfileSetupRecord::default();
        for (index, record) in journal.records.iter().enumerate() {
            record.validate_integrity()?;
            previous.check_transition(record)?;
            check_setup_identities(&journal.records[..index], record)?;
            previous = record.clone();
        }
        Ok(journal.records)
    }

    pub fn profile_setup(&self) -> Result<ProfileSetupRecord> {
        Ok(self.profile_setup_history()?.pop().unwrap_or_default())
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
        let fresh = Store::open_unobserved(self.root())?;
        let mut records = fresh.profile_setup_history()?;
        let previous = records.last().cloned().unwrap_or_default();
        if &previous == record {
            return Ok(());
        }
        previous.check_transition(record)?;
        check_setup_identities(&records, record)?;
        match record.state {
            ProfileSetupState::Applied | ProfileSetupState::Cancelled => {
                record.validate_integrity()?
            }
            ProfileSetupState::NotStarted
            | ProfileSetupState::Gathering
            | ProfileSetupState::Proposed => record.validate(&fresh.config)?,
        }
        records.push(record.clone());
        write_journal(
            &fresh.state_dir().join("profile-setup.json"),
            &SetupJournal {
                version: PROFILE_RECORD_VERSION,
                records,
            },
        )
    }

    /// Historical stale receipts remain inspectable; callers validate before future routing.
    pub fn auto_profile_selections(&self) -> Result<Vec<AutoProfileSelection>> {
        let journal: SelectionJournal =
            read_journal(&self.state_dir().join("profile-selections.json"))?;
        ensure!(
            journal.version == PROFILE_RECORD_VERSION,
            "unsupported selection journal version"
        );
        ensure!(
            journal
                .receipts
                .iter()
                .all(|r| r.version == PROFILE_RECORD_VERSION),
            "unsupported Auto selection version"
        );
        Ok(journal.receipts)
    }

    pub fn save_auto_profile_selection(
        &self,
        receipt: &AutoProfileSelection,
        review: Option<&ReviewSelectionContext<'_>>,
    ) -> Result<()> {
        self.check_runtime()?;
        let _lock = self.lock("profile-records")?;
        let fresh = Store::open_unobserved(self.root())?;
        let task = fresh.get(&receipt.task)?;
        let mut receipts = fresh.auto_profile_selections()?;
        if let Some(previous) = receipts.iter().find(|old| old.id == receipt.id) {
            ensure!(previous == receipt, "conflicting Auto selection identity");
            return Ok(());
        }
        receipt.validate(&fresh.config, &task)?;
        match (&receipt.review, review) {
            (Some(selection), Some(context)) => {
                ensure!(context.task == &task, "stale review selection task");
                selection.validate(&fresh.config, context)?;
            }
            (None, None) => {}
            (Some(_), None) | (None, Some(_)) => {
                anyhow::bail!("review selection requires exact review context")
            }
        }
        receipts.push(receipt.clone());
        write_journal(
            &fresh.state_dir().join("profile-selections.json"),
            &SelectionJournal {
                version: PROFILE_RECORD_VERSION,
                receipts,
            },
        )
    }

    pub fn profile_repair_histories(&self) -> Result<BTreeMap<String, RepairHistory>> {
        let journal: RepairJournal = read_journal(&self.state_dir().join("profile-repairs.json"))?;
        ensure!(
            journal.version == PROFILE_RECORD_VERSION,
            "unsupported repair journal version"
        );
        Ok(journal.histories)
    }

    /// Histories are append-only, including across switches/restarts. No counter reset API.
    pub fn save_profile_repair_history(&self, task: &str, history: &RepairHistory) -> Result<()> {
        self.check_runtime()?;
        let _lock = self.lock("profile-records")?;
        let fresh = Store::open_unobserved(self.root())?;
        fresh.get(task)?;
        let mut histories = fresh.profile_repair_histories()?;
        if let Some(previous) = histories.get(task) {
            ensure!(
                history.policy() == previous.policy()
                    && history.current_profile() == previous.current_profile()
                    && history.stronger_profile() == previous.stronger_profile()
                    && history.events().starts_with(previous.events()),
                "repair history cannot be rewritten"
            );
        }
        let names = fresh.config.profile_names();
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
        histories.insert(task.to_owned(), history.clone());
        write_journal(
            &fresh.state_dir().join("profile-repairs.json"),
            &RepairJournal {
                version: PROFILE_RECORD_VERSION,
                histories,
            },
        )
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

pub(crate) fn read_journal<T: DeserializeOwned + Default>(path: &Path) -> Result<T> {
    match fs::read_to_string(path) {
        Ok(text) => {
            serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

pub(crate) fn write_journal(path: &Path, value: &impl Serialize) -> Result<()> {
    fs::create_dir_all(path.parent().context("journal has no parent")?)
        .context("creating profile journal directory")?;
    crate::store::atomic_write(
        path,
        &serde_json::to_string(value).context("encoding profile journal")?,
    )
}
