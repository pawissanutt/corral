use serde::{Deserialize, Serialize};

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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadStatus {
    Complete,
    Unsupported,
    Failed,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Coverage {
    pub complete: u64,
    pub unsupported: u64,
    pub failed: u64,
    pub skipped_non_utf8_paths: u64,
    /// Resolved and unresolved counts refer to detected syntax occurrences, not all uses of symbols.
    pub resolved_references: u64,
    pub unresolved_references: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ContainerFact {
    /// Repository-relative directory; empty for a root container.
    pub path: String,
    pub name: String,
    pub kind: ContainerKind,
    pub workspace_status: WorkspaceStatus,
    /// Other manifests in this directory share its map node and source files.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub other_packages: Vec<PackageIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PackageIdentity {
    pub name: String,
    pub kind: ContainerKind,
    pub workspace_status: WorkspaceStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContainerKind {
    Cargo,
    Npm,
    Python,
    Folder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceStatus {
    Member,
    Excluded,
    SeparateWorkspace,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ModuleId {
    pub container: String,
    /// First segment under the container's source root (or the container itself).
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DependencyFact {
    pub container: String,
    pub manifest: String,
    pub section: String,
    pub alias: String,
    pub package: String,
    pub requirement: String,
    /// Structured manifest options (e.g. Cargo features), in normalized text form.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<String>,
    /// A local container path, if this declaration could be associated with one.
    pub target: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceKind {
    Use,
    Import,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceBasis {
    Syntax,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reference<T> {
    pub from: T,
    pub to: T,
    pub kind: ReferenceKind,
    pub basis: ReferenceBasis,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageEdge {
    pub from: String,
    pub to: String,
    pub kind: PackageEdgeKind,
    pub references: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageEdgeKind {
    DeclaredAndReferenced,
    DeclaredOnly,
    ReferencedOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpawnFact {
    pub path: String,
    pub process: String,
    pub count: u64,
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    pub added: u64,
    pub removed: u64,
    pub binary: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependencyChange {
    pub before: Option<DependencyFact>,
    pub after: Option<DependencyFact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleReferenceChange {
    pub from: ModuleId,
    pub to: ModuleId,
    pub before: u64,
    pub after: u64,
}
