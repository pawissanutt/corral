//! The stored diff and run summary from `done`, with the applicable checks.
use corral_types::{Artifact, Check, Run};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Immutable identity of the coding candidate whose patch is being displayed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReviewCandidate {
    pub run: String,
    pub base: String,
    pub head: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReviewDetail {
    pub task: String,
    /// The candidate run contains the summary and commit recorded at `done`.
    #[schemars(with = "Option<crate::schema_types::Run>")]
    pub run: Option<Run>,
    #[schemars(with = "Option<crate::schema_types::Artifact>")]
    pub diff: Option<Artifact>,
    /// The gate check for HEAD and the review check for the current round.
    #[schemars(with = "Vec<crate::schema_types::Check>")]
    pub checks: Vec<Check>,
    pub max_rounds: u32,
    pub files: Vec<ReviewFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReviewFile {
    pub path: String,
    pub added: u32,
    pub deleted: u32,
}
