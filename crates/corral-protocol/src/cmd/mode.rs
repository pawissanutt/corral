//! Typed mode commands; see the family rules in the parent module.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "verb", rename_all = "kebab-case")]
pub enum ModeCommand {
    Assist {},
    Pause {},
    Resume {},
    Turbo {
        off: bool,
    },
    Autopilot {
        projects: Vec<String>,
        include_new: bool,
        expires_after_days: Option<u32>,
        max_new_tasks: Option<u32>,
        tasks: Vec<String>,
        exclusions: Vec<String>,
        plan_scopes: Vec<String>,
        dispatch: bool,
        accept: bool,
        plan_accept: bool,
        aggregate_accept: bool,
        confirm: String,
        json: bool,
    },
    Off {
        json: bool,
    },
    #[serde(other)]
    Other,
}

impl ModeCommand {
    pub fn verb(&self) -> &'static str {
        match self {
            Self::Assist {} => "assist",
            Self::Pause {} => "pause",
            Self::Resume {} => "resume",
            Self::Turbo { .. } => "turbo",
            Self::Autopilot { .. } => "autopilot",
            Self::Off { .. } => "off",
            Self::Other => "other",
        }
    }
}
