use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use corral_types::{ChangeKind, DependentFact, TestCandidate};

use super::{ProviderCoverage, Reply, Row, enum_name};
use crate::{ProjectDelta, TreeFacts};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "fact", rename_all = "snake_case", try_from = "WireChangedFact")]
pub enum ChangedFact {
    Definition,
    Reference { to_path: String, to_name: String },
}

#[derive(Deserialize)]
struct WireChangedFact {
    fact: String,
    // These fields are required strings for reference changes and ignored
    // altogether for definitions, just like other fields outside their shape.
    to_path: Option<Value>,
    to_name: Option<Value>,
}

impl TryFrom<WireChangedFact> for ChangedFact {
    type Error = &'static str;

    fn try_from(value: WireChangedFact) -> Result<Self, Self::Error> {
        match value.fact.as_str() {
            "definition" => Ok(Self::Definition),
            "reference" => match (value.to_path, value.to_name) {
                (Some(Value::String(to_path)), Some(Value::String(to_name))) => {
                    Ok(Self::Reference { to_path, to_name })
                }
                _ => Err("reference change requires string fields to_path and to_name"),
            },
            _ => Err("change fact must be definition or reference"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeFact {
    #[serde(deserialize_with = "enum_name")]
    pub change: ChangeKind,
    #[serde(flatten)]
    pub fact: ChangedFact,
    pub path: String,
    pub name: String,
    pub line: u64,
}

impl Reply {
    /// Populate the provider-only lists without changing built-in delta facts.
    /// `None` means not provided; `Some([])` means the provider reported no rows
    /// for a category (or none survived filtering against the commits).
    pub fn merge_delta(
        self,
        delta: &mut ProjectDelta,
        before: &TreeFacts,
        after: &TreeFacts,
    ) -> ProviderCoverage {
        let Self { rows, mut coverage } = self;
        delta.changes = coverage.provides("changes").then(Vec::new);
        delta.dependents = coverage.provides("dependents").then(Vec::new);
        delta.tests = coverage.provides("tests").then(Vec::new);

        let base = file_paths(before);
        let head = file_paths(after);
        for row in rows {
            let paths = match &row {
                Row::Change { change } if change.change == ChangeKind::Removed => &base,
                _ => &head,
            };
            if !row.has_known_paths(|path| paths.contains(path)) {
                coverage.dropped_unknown_files += 1;
                continue;
            }
            match row {
                Row::Change { change } => {
                    if let Some(changes) = &mut delta.changes {
                        changes.push(change);
                    }
                }
                Row::Dependent { dependent } => {
                    if let Some(dependents) = &mut delta.dependents {
                        dependents.push(dependent);
                    }
                }
                Row::Test { candidate } => {
                    if let Some(tests) = &mut delta.tests {
                        tests.push(candidate);
                    }
                }
                _ => {}
            }
        }
        coverage
    }
}

fn file_paths(tree: &TreeFacts) -> HashSet<&str> {
    tree.files.iter().map(|file| file.path.as_str()).collect()
}
