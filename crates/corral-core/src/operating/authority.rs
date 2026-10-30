//! Operating mode and delegated authority flags of a workspace grant.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatingMode {
    #[default]
    Assist,
    Autopilot,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OperatingAuthority {
    pub dispatch: bool,
    pub accept: bool,
    pub plan_accept: bool,
    /// Final tracker Plan acceptance, never implied by `accept` or `plan_accept`.
    /// Omitted when off so grants confirmed before this field keep their digest.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub aggregate_accept: bool,
}
