use serde::{Deserialize, Serialize};

pub use corral_types::facts::*;

use crate::provider::ChangeFact;

/// An immutable resolved revision. Obtain it with [`crate::resolve_revision`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Revision {
    pub(crate) commit: String,
    pub(crate) tree: String,
    pub(crate) committed_at: i64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) shallow_boundary: Vec<String>,
}

impl Revision {
    pub fn commit(&self) -> &str {
        &self.commit
    }
    pub fn tree(&self) -> &str {
        &self.tree
    }
    /// Committer timestamp in Unix seconds.
    pub fn committed_at(&self) -> i64 {
        self.committed_at
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectFacts {
    pub revision: Revision,
    pub structure: TreeFacts,
    pub activity: Activity,
}

/// Facts derived exclusively from the tree, independent of the checkout and commit history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeFacts {
    pub files: Vec<FileFact>,
    pub containers: Vec<ContainerFact>,
    pub modules: Vec<ModuleId>,
    pub dependencies: Vec<DependencyFact>,
    pub file_references: Vec<Reference<String>>,
    /// Includes same-module references; renderers may choose not to draw those edges.
    pub module_references: Vec<Reference<ModuleId>>,
    pub package_edges: Vec<PackageEdge>,
    pub spawns: Vec<SpawnFact>,
    pub coverage: Coverage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileFact {
    pub path: String,
    pub language: String,
    /// Physical lines including an unterminated final line; zero for binary/non-file entries.
    pub lines: u64,
    pub test: bool,
    pub read_status: ReadStatus,
    pub container: String,
    pub module: ModuleId,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub definitions: Vec<DefinitionFact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Activity {
    pub days_7: Vec<ActivityEntry>,
    pub days_30: Vec<ActivityEntry>,
    /// Commit-level evidence for the 30-day window. Grouping by commit lets
    /// callers count unique changes and attribute older touches to tasks.
    pub commits: Vec<ActivityCommit>,
    pub shallow: bool,
    pub skipped_non_utf8_paths: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityCommit {
    pub commit: String,
    pub committed_at: i64,
    pub subject: String,
    pub files: Vec<FileChange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityEntry {
    pub path: String,
    pub commits: u64,
    pub added: u64,
    pub removed: u64,
    pub last_commit: String,
    pub last_committed_at: i64,
    pub last_subject: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectDelta {
    pub files: Vec<FileChange>,
    pub skipped_non_utf8_paths: u64,
    pub dependencies: Vec<DependencyChange>,
    pub module_references: Vec<ModuleReferenceChange>,
    /// None means not provided; Some(empty) means the provider found no rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changes: Option<Vec<ChangeFact>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependents: Option<Vec<DependentFact>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tests: Option<Vec<TestCandidate>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    pub added: u64,
    pub removed: u64,
    pub binary: bool,
}
