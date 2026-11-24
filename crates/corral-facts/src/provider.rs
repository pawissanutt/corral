//! Validation and merge of facts returned by an extension. The provider cannot
//! create files, modules, or containers: it attaches facts to immutable trees.

mod delta;
mod snapshot;

use std::collections::{BTreeMap, BTreeSet};

pub use corral_types::{DefinitionFact, ProviderCoverage};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Deserializer, Serialize, de::value::StringDeserializer};

pub use self::delta::{ChangeFact, ChangeKind, ChangedFact, DependentFact, TestCandidate};
use crate::{DataField, DataKind, EntryKind, ReadStatus, ReferenceKind};

/// An entire validated reply, stored separately from the built-in facts.
#[derive(Debug, Serialize, Deserialize)]
pub struct Reply {
    rows: Vec<Row>,
    coverage: ProviderCoverage,
}

impl Reply {
    /// Parse all JSON Lines before any rows can be merged. Only categories both
    /// requested and declared in the final `done` row can contribute facts.
    pub fn parse(output: &[u8], want: &[&str]) -> Result<Self> {
        let output = std::str::from_utf8(output).context("facts reply is not UTF-8")?;
        let mut lines = output.lines().enumerate();
        let mut rows = Vec::new();
        let mut type_ids = BTreeSet::new();
        let mut remaining_type_budget = crate::data::REPLY_BUDGET;
        for (index, line) in lines.by_ref() {
            let number = index + 1;
            // Serde's internally tagged enum also supports sequence input.
            ensure!(
                line.trim_start().starts_with('{'),
                "malformed facts row at line {number}: expected a JSON object"
            );
            let row: Row = serde_json::from_str(line)
                .with_context(|| format!("malformed facts row at line {number}"))?;
            if let Row::Done {
                provided,
                skipped,
                omitted,
            } = row
            {
                ensure!(lines.next().is_none(), "facts reply has rows after done");
                let (provided, not_provided) = want
                    .iter()
                    .map(|&category| category.to_owned())
                    .partition(|category| provided.contains(category));
                return Ok(Self {
                    rows,
                    coverage: ProviderCoverage {
                        provided,
                        not_provided,
                        skipped,
                        omitted,
                        dropped_unknown_files: 0,
                    },
                });
            }
            row.validate()
                .with_context(|| format!("malformed facts row at line {number}"))?;
            if let Row::Type {
                path, name, fields, ..
            } = &row
            {
                remaining_type_budget = crate::data::extension_shape_cost(path, name, fields)
                    .and_then(|cost| remaining_type_budget.checked_sub(cost))
                    .with_context(|| {
                        format!("malformed facts row at line {number}: data shape budget exceeded")
                    })?;
                ensure!(
                    type_ids.insert((path.clone(), name.clone())),
                    "malformed facts row at line {number}: duplicate data type"
                );
            }
            rows.push(row);
        }
        anyhow::bail!("facts reply is missing a final done row")
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Row {
    File {
        path: String,
        #[serde(deserialize_with = "enum_name")]
        read: ReadStatus,
    },
    Definition {
        path: String,
        #[serde(flatten)]
        definition: DefinitionFact,
    },
    Reference {
        from: String,
        to: String,
        #[serde(deserialize_with = "enum_name")]
        kind: ReferenceKind,
        count: u64,
    },
    Entry {
        path: String,
        line: u64,
        #[serde(deserialize_with = "enum_name")]
        kind: EntryKind,
        name: String,
    },
    Type {
        path: String,
        name: String,
        line: u64,
        #[serde(deserialize_with = "enum_name")]
        kind: DataKind,
        fields: Vec<DataField>,
    },
    Change {
        #[serde(flatten)]
        change: ChangeFact,
    },
    Dependent {
        #[serde(flatten)]
        dependent: DependentFact,
    },
    Test {
        #[serde(flatten)]
        candidate: TestCandidate,
    },
    Done {
        provided: Vec<String>,
        skipped: BTreeMap<String, u64>,
        omitted: BTreeMap<String, u64>,
    },
}

impl Row {
    fn validate(&self) -> Result<()> {
        match self {
            Self::Definition { definition, .. } => ensure!(
                definition.line > 0 && definition.end_line >= definition.line,
                "invalid definition line range"
            ),
            Self::Reference { count, .. } => ensure!(*count > 0, "zero reference count"),
            Self::Entry { line, .. } => ensure!(*line > 0, "zero entry line"),
            Self::Type {
                name, line, fields, ..
            } => {
                ensure!(
                    *line > 0 && !name.trim().is_empty(),
                    "invalid data declaration"
                );
                let mut names = BTreeSet::new();
                for field in fields {
                    ensure!(
                        field.line > 0
                            && !field.name.trim().is_empty()
                            && !field.ty.trim().is_empty(),
                        "invalid data field"
                    );
                    ensure!(names.insert(&field.name), "duplicate data field name");
                }
            }
            Self::Change { change } => ensure!(change.line > 0, "zero change line"),
            Self::Dependent { dependent } => {
                ensure!(dependent.line > 0, "zero dependent line");
                ensure!(dependent.depth != Some(0), "zero dependent depth");
            }
            _ => {}
        }
        Ok(())
    }

    fn has_known_paths(&self, contains: impl Fn(&str) -> bool) -> bool {
        match self {
            Self::File { path, .. }
            | Self::Definition { path, .. }
            | Self::Entry { path, .. }
            | Self::Type { path, .. } => contains(path),
            Self::Reference { from, to, .. } => contains(from) && contains(to),
            Self::Change { change } => {
                contains(&change.path)
                    && match &change.fact {
                        ChangedFact::Definition => true,
                        ChangedFact::Reference { to_path, .. } => contains(to_path),
                    }
            }
            Self::Dependent { dependent } => contains(&dependent.path),
            Self::Test { candidate } => contains(&candidate.path),
            Self::Done { .. } => true,
        }
    }
}

// Unit enum fields must use their documented string, not an externally tagged
// object that Serde would otherwise accept (for example, {"complete": null}).
fn enum_name<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(StringDeserializer::new(String::deserialize(deserializer)?))
}

#[cfg(test)]
mod tests;
