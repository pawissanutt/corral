use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::{ArtifactTier, parse_age};

/// What retention does after an artifact crosses the configured thresholds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RetentionAction {
    #[default]
    Compress,
    Purge,
    Keep,
}

/// `[artifacts.retention]` or `[projects.<p>.artifacts.retention]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArtifactRetention {
    pub action: RetentionAction,
    pub older_than: String,
    pub larger_than: Option<String>,
    pub tiers: Vec<ArtifactTier>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub per_tier: BTreeMap<ArtifactTier, RetentionOverride>,
}

/// Optional action and thresholds for one selected tier.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RetentionOverride {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<RetentionAction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub older_than: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub larger_than: Option<String>,
}

impl Default for ArtifactRetention {
    fn default() -> Self {
        Self {
            action: RetentionAction::Compress,
            older_than: "14d".into(),
            larger_than: Some("1MB".into()),
            tiers: vec![ArtifactTier::Transient, ArtifactTier::Normal],
            per_tier: BTreeMap::new(),
        }
    }
}

impl ArtifactRetention {
    /// Applies one tier's overrides; the parent tier allowlist still governs eligibility.
    pub fn for_tier(&self, tier: ArtifactTier) -> Self {
        let mut rule = self.clone();
        if let Some(overrides) = self.per_tier.get(&tier) {
            if let Some(action) = overrides.action {
                rule.action = action;
            }
            if let Some(age) = &overrides.older_than {
                rule.older_than = age.clone();
            }
            if let Some(size) = &overrides.larger_than {
                rule.larger_than = Some(size.clone());
            }
        }
        rule.per_tier.clear();
        rule
    }

    pub(crate) fn problems(&self, key: &str) -> Vec<String> {
        let mut out = Vec::new();
        if let Err(e) = parse_age(&self.older_than) {
            out.push(format!("{key}.older_than: {e:#}"));
        }
        if let Some(size) = &self.larger_than
            && let Err(e) = parse_bytes(size)
        {
            out.push(format!("{key}.larger_than: {e:#}"));
        }
        for tier in self.per_tier.keys() {
            out.extend(
                self.for_tier(*tier)
                    .problems(&format!("{key}.per_tier.{}", tier.as_str())),
            );
        }
        out
    }
}

/// Parses positive whole byte counts with decimal B/KB/MB/GB or binary KiB/MiB/GiB units.
///
/// # Errors
/// Rejects unknown units, fractional/nonpositive counts, and values exceeding `u64`.
pub fn parse_bytes(s: &str) -> Result<u64> {
    let s = s.trim();
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (number, unit) = s.split_at(split);
    let multiplier: u64 = match unit.trim() {
        "B" => 1,
        "KB" => 1_000,
        "MB" => 1_000_000,
        "GB" => 1_000_000_000,
        "KiB" => 1_024,
        "MiB" => 1_048_576,
        "GiB" => 1_073_741_824,
        _ => bail!("{s:?} is not a byte size: use B, KB, MB, GB, KiB, MiB or GiB"),
    };
    let count: u64 = number
        .parse()
        .with_context(|| format!("{s:?} is not a byte size: count must be a whole number"))?;
    if count == 0 {
        bail!("{s:?} is not a byte size: count must be positive");
    }
    count
        .checked_mul(multiplier)
        .with_context(|| format!("{s:?} is too large"))
}

/// Keep the direct Rust field while nesting its TOML under the project's artifacts table.
pub(crate) mod project_retention {
    use super::*;
    use serde::{Deserializer, Serializer};

    #[derive(Serialize, Deserialize)]
    struct ProjectArtifacts<T> {
        retention: T,
    }

    pub fn serialize<S: Serializer>(
        retention: &Option<ArtifactRetention>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        ProjectArtifacts { retention }.serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<ArtifactRetention>, D::Error> {
        let artifacts = ProjectArtifacts::<Option<ArtifactRetention>>::deserialize(deserializer)?;
        Ok(artifacts.retention)
    }
}
