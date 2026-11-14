//! Recorded cost and quota series. Unknown quota facts are absent.
use crate::board::CostBasis;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct UsageSeries {
    pub providers: Vec<ProviderSeries>,
    pub days: Vec<UsageDay>,
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
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct UsageDay {
    pub day: String,
    pub by_profile: Vec<ProfileUsage>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProfileUsage {
    pub profile: String,
    pub cost_usd: Option<f64>,
    pub cost_basis: Option<CostBasis>,
    pub tokens: Option<u64>,
}
