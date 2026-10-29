//! The migration journal `C/migration.json` and the fence `C/state/schema-version`.

use std::fs;
use std::io;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use super::{FENCE_PATH, JOURNAL, durable};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalState {
    Fenced,
    Importing,
    Verified,
    Published,
    RollingBack,
    Finalizing,
    Finalized,
}

impl JournalState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fenced => "fenced",
            Self::Importing => "importing",
            Self::Verified => "verified",
            Self::Published => "published",
            Self::RollingBack => "rolling_back",
            Self::Finalizing => "finalizing",
            Self::Finalized => "finalized",
        }
    }

    pub fn is_pre_publish(self) -> bool {
        matches!(self, Self::Fenced | Self::Importing | Self::Verified)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RollbackStep {
    Intent,
    Archived,
    Quarantined,
    Cleaned,
    FenceRestored,
}

impl RollbackStep {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Intent => "intent",
            Self::Archived => "archived",
            Self::Quarantined => "quarantined",
            Self::Cleaned => "cleaned",
            Self::FenceRestored => "fence_restored",
        }
    }
}

/// Where a pre-publish rollback found its candidate database.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Candidate {
    #[serde(rename = "db")]
    Db,
    #[serde(rename = "db.migrating")]
    Migrating,
    #[serde(rename = "none")]
    None,
}

impl Candidate {
    pub fn dir(self) -> Option<&'static str> {
        match self {
            Self::Db => Some(super::DB),
            Self::Migrating => Some(super::MIGRATING),
            Self::None => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        self.dir().unwrap_or("none")
    }
}

/// The rollback in progress: from `published` (the archive and the discarded change count) or
/// from a pre-publish state (the candidate's location).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RollbackRecord {
    pub from: JournalState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discard: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_seq_observed: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate: Option<Candidate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Journal {
    pub state: JournalState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<RollbackStep>,
    /// The exact fence bytes before the migration; `None` when the fence was absent.
    pub original_fence: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub m0_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub migration_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub import_seq: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_seq_at_publish: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollback: Option<RollbackRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub legacy_paths: Vec<String>,
    pub started: jiff::Timestamp,
}

impl Journal {
    pub fn read(corral: &Path) -> Result<Option<Journal>> {
        let path = corral.join(JOURNAL);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
        };
        serde_json::from_str(&text)
            .map(Some)
            .with_context(|| format!("parsing {}", path.display()))
    }

    /// Writes the journal durably (tmp -> fsync -> rename -> fsync `C`).
    pub fn write(&self, corral: &Path) -> Result<()> {
        let mut text = serde_json::to_string_pretty(self).context("serializing the journal")?;
        text.push('\n');
        durable::write(&corral.join(JOURNAL), text.as_bytes(), || Ok(()))
    }

    pub fn remove(corral: &Path) -> Result<()> {
        durable::unlink(&corral.join(JOURNAL))
    }

    pub fn rollback(&self) -> Result<&RollbackRecord> {
        self.rollback
            .as_ref()
            .context("the rolling_back journal has no rollback record")
    }
}

/// The exact fence bytes, `None` when absent; refuses a fence that is not text.
pub fn read_fence(corral: &Path) -> Result<Option<String>> {
    let path = corral.join(FENCE_PATH);
    match fs::read(&path) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => Ok(Some(text)),
            Err(_) => bail!("{} is not a text schema marker", path.display()),
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

/// The fence value with surrounding whitespace removed, as every reader compares it.
pub fn fence_value(fence: Option<&str>) -> Option<&str> {
    fence.map(str::trim)
}

/// Writes `bytes` as the fence, or unlinks it for `None`; `between` runs in the tmp/rename
/// window (B14).
pub fn write_fence(
    corral: &Path,
    bytes: Option<&str>,
    between: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let path = corral.join(FENCE_PATH);
    match bytes {
        Some(bytes) => {
            let state = path.parent().context("fence has a parent")?;
            fs::create_dir_all(state).with_context(|| format!("creating {}", state.display()))?;
            durable::write(&path, bytes.as_bytes(), between)
        }
        None => {
            between()?;
            durable::unlink(&path)
        }
    }
}
