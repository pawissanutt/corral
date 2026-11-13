//! Typed meta commands; see the family rules in the parent module.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "verb", rename_all = "kebab-case")]
pub enum MetaCommand {
    Pin {
        target: String,
        label: Option<String>,
        follow_latest: bool,
    },
    Unpin {
        query: String,
    },
    DashboardSet {
        id: String,
        interval_secs: u32,
        next_due: String,
        authorize_dispatch: bool,
    },
    DashboardPause {
        id: String,
    },
    DashboardRemove {
        id: String,
    },
    ArtifactSetTier {
        reference: String,
        tier: String,
    },
    #[serde(other)]
    Other,
}

impl MetaCommand {
    pub fn verb(&self) -> &'static str {
        match self {
            Self::Pin { .. } => "pin",
            Self::Unpin { .. } => "unpin",
            Self::DashboardSet { .. } => "dashboard-set",
            Self::DashboardPause { .. } => "dashboard-pause",
            Self::DashboardRemove { .. } => "dashboard-remove",
            Self::ArtifactSetTier { .. } => "artifact-set-tier",
            Self::Other => "other",
        }
    }
}
