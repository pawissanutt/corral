//! Direct submission of an engine operation.
use corral_types::Op;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const OP_SUBMIT: &str = "op.submit";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OpSubmit {
    #[schemars(with = "crate::schema_types::Op")]
    pub op: Op,
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum OpReply {
    Done { seq: u64, task: Option<String> },
    Running { seq: u64 },
}

#[cfg(test)]
#[path = "op_tests.rs"]
mod tests;
