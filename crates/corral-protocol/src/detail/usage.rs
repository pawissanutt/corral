//! Recorded accounting and quota evidence. Missing completeness is unknown, not complete.
use crate::board::CostBasis;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct UsageSeries {
    pub providers: Vec<ProviderSeries>,
    pub days: Vec<UsageDay>,
    #[serde(default)]
    pub completeness: UsageCompleteness,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderSeries {
    pub id: String,
    pub points: Vec<UsagePoint>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct UsagePoint {
    pub at: String,
    pub used: Option<f64>,
    pub limit: Option<f64>,
    pub cost_usd: Option<f64>,
    pub cost_basis: Option<CostBasis>,
    #[serde(default)]
    pub completeness: UsageCompleteness,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct UsageDay {
    pub day: String,
    pub by_profile: Vec<ProfileUsage>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProfileUsage {
    pub profile: String,
    /// Empty only for legacy senders or an unknown connection.
    #[serde(default)]
    pub connection: String,
    pub cost_usd: Option<f64>,
    pub cost_basis: Option<CostBasis>,
    pub tokens: Option<u64>,
    #[serde(default)]
    pub completeness: UsageCompleteness,
}

/// Scan coverage accompanies every monetary/token total, including known subtotals.
/// The scanned instants are the earliest/latest dated facts examined, not proof
/// that every record between them was examined. `partial` covers omitted records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UsageCompleteness {
    pub partial: bool,
    pub reason: Option<String>,
    pub scanned_rows: u64,
    pub scanned_from: Option<String>,
    pub scanned_to: Option<String>,
    pub unpriced_entries: u64,
    pub undated_entries: u64,
}

impl Default for UsageCompleteness {
    fn default() -> Self {
        Self {
            partial: true,
            reason: Some("coverage_unknown".into()),
            scanned_rows: 0,
            scanned_from: None,
            scanned_to: None,
            unpriced_entries: 0,
            undated_entries: 0,
        }
    }
}

/// Provider evidence is not a numerical allowance. An exhaustion observation can
/// name a reset while its limit, consumption and headroom remain unknown.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct UsageQuota {
    pub source: String,
    pub scope: Option<String>,
    pub window: Option<String>,
    pub used: Option<f64>,
    pub limit: Option<f64>,
    pub headroom: Option<f64>,
    pub resets_at: Option<String>,
    pub observed_at: Option<String>,
    pub exhausted: Option<bool>,
    #[serde(default)]
    pub completeness: UsageCompleteness,
}
