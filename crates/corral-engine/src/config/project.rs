use std::{collections::BTreeMap, path::PathBuf};

use serde::{Deserialize, Deserializer, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Project {
    pub path: PathBuf,
    pub base: Option<String>,
    #[serde(deserialize_with = "deserialize_gate")]
    pub gate: Vec<String>,
    pub reviewer: Option<String>,
    pub profile: Option<String>,
    pub wrap: Option<String>,
    pub running: Option<usize>,
    pub env: BTreeMap<String, String>,
    pub auto_dispatch: Option<bool>,
    pub auto_land: Option<bool>,
    pub max_rounds: Option<u32>,
    pub branch_prefix: Option<String>,
    /// Rust enum names whose variants are system-map operations.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub operation_enums: Vec<String>,
    /// Repository-relative directory for committed system-map lens files.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lenses: Option<String>,
}

impl Project {
    pub fn lens_dir(&self) -> &str {
        self.lenses.as_deref().unwrap_or(".corral/lenses")
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct EffectiveProject {
    pub path: PathBuf,
    pub base: Option<String>,
    pub gate: Vec<String>,
    pub reviewer: Option<String>,
    pub profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wrap: Option<String>,
    pub running: Option<usize>,
    pub env: BTreeMap<String, String>,
    pub auto_dispatch: bool,
    pub auto_land: bool,
    pub max_rounds: u32,
    pub branch_prefix: String,
}

fn deserialize_gate<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Gate {
        One(String),
        Many(Vec<String>),
    }
    Ok(match Gate::deserialize(d)? {
        Gate::One(line) => vec![line],
        Gate::Many(lines) => lines,
    })
}
