use std::{collections::BTreeSet, fmt, str::FromStr};

use serde::{Deserialize, Serialize};

pub const TASK_SIZES: [&str; 5] = ["xs", "s", "m", "l", "xl"];

/// Suggested labels; custom labels use the same field and validation.
pub const BUILTIN_TAGS: [&str; 6] = [
    "accessibility",
    "bug",
    "dependencies",
    "documentation",
    "performance",
    "testing",
];

/// Technical complexity or uncertainty, independent of work volume and priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskDifficulty {
    Routine,
    Standard,
    Hard,
}

impl TaskDifficulty {
    pub const ALL: [Self; 3] = [Self::Routine, Self::Standard, Self::Hard];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Routine => "routine",
            Self::Standard => "standard",
            Self::Hard => "hard",
        }
    }
}

impl fmt::Display for TaskDifficulty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for TaskDifficulty {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|difficulty| difficulty.as_str() == value)
            .ok_or_else(|| format!("unknown difficulty {value:?} (routine, standard, hard)"))
    }
}

pub fn validate_tag(tag: &str) -> Result<(), String> {
    if tag.is_empty() || tag.trim() != tag || tag.chars().any(|ch| ch.is_control()) {
        return Err(format!(
            "invalid tag {tag:?}: use a nonempty label without surrounding whitespace or control characters"
        ));
    }
    Ok(())
}

/// Keep exact spelling and insertion order; only identical labels are duplicates.
pub fn normalize_tags(mut tags: Vec<String>) -> Result<Vec<String>, String> {
    for tag in &tags {
        validate_tag(tag)?;
    }
    let mut seen = BTreeSet::new();
    tags.retain(|tag| seen.insert(tag.clone()));
    Ok(tags)
}
