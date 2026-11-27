//! Workspace Pi skill catalog. IDs are opaque source identities, never client-supplied paths.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const PI_SKILLS_V1: &str = "config.pi_skills.v1";
pub const PI_SKILLS: &str = "config.pi_skills";
pub const PI_SKILL_GET: &str = "config.pi_skill_get";
pub const PI_SKILL_PUT: &str = "config.pi_skill_put";
pub const PI_SKILL_TOGGLE: &str = "config.pi_skill_toggle";

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PiSkillSource {
    Builtin,
    Role,
    User,
    Project,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PiSkillsRequest {
    pub project: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PiSkillGet {
    pub project: Option<String>,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PiSkillPut {
    pub project: Option<String>,
    pub id: String,
    pub text: String,
    pub revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PiSkillToggle {
    pub project: Option<String>,
    pub name: String,
    pub enabled: bool,
    pub revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PiSkillsCatalog {
    pub skills: Vec<PiSkillRow>,
    pub revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PiSkillDetail {
    pub id: String,
    pub text: String,
    pub revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PiSkillRow {
    pub id: String,
    pub name: String,
    pub source: PiSkillSource,
    pub path: String,
    pub enabled: bool,
    pub editable: bool,
    pub required: bool,
    pub shadowed: bool,
}
