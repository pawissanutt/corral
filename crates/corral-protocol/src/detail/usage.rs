//! Accounted usage, its run coverage, and the runs used for durations.
use corral_types::{Run, Usage, UsageSummary};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct UsageSeries {
    /// Coverage and totals for the whole query, independent of the detail page.
    #[schemars(with = "crate::schema_types::UsageSummary")]
    pub summary: UsageSummary,
    #[schemars(with = "Vec<crate::schema_types::Usage>")]
    pub rows: Vec<Usage>,
    #[schemars(with = "Vec<crate::schema_types::Run>")]
    pub runs: Vec<Run>,
}
