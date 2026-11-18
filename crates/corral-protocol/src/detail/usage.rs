//! Reported usage and run coverage in local calendar-day buckets.
pub use corral_types::UsageDay;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct UsageSeries {
    /// Ascending days, including days without any reports or runs.
    #[schemars(with = "Vec<UsageDaySchema>")]
    pub days: Vec<UsageDay>,
}

#[derive(JsonSchema)]
#[schemars(rename = "UsageDay")]
#[allow(dead_code, reason = "schema projection for the shared serde type")]
struct UsageDaySchema {
    day: String,
    #[schemars(with = "crate::schema_types::UsageSummary")]
    summary: corral_types::UsageSummary,
}
