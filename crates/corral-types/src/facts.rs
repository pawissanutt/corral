//! Structural facts shared by the provider, daemon and protocol clients.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

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
    Call,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceBasis {
    Syntax,
    Resolved,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefinitionFact {
    pub name: String,
    pub line: u64,
    pub end_line: u64,
    pub kind: String,
}

/// Which requested categories the provider answered, including empty results.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCoverage {
    pub provided: Vec<String>,
    pub not_provided: Vec<String>,
    pub skipped: BTreeMap<String, u64>,
    pub omitted: BTreeMap<String, u64>,
    pub dropped_unknown_files: u64,
}

impl ProviderCoverage {
    /// Describe a missing or unusable reply without reporting a zero result.
    pub fn unavailable(want: &[&str]) -> Self {
        Self {
            provided: Vec::new(),
            not_provided: want.iter().map(|&category| category.to_owned()).collect(),
            skipped: BTreeMap::new(),
            omitted: BTreeMap::new(),
            dropped_unknown_files: 0,
        }
    }

    pub fn provides(&self, category: &str) -> bool {
        self.provided.iter().any(|entry| entry == category)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Added,
    Removed,
    Changed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependentFact {
    pub path: String,
    pub name: String,
    pub line: u64,
    /// Full name of the changed definition that this definition depends on.
    pub of: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestCandidate {
    pub path: String,
    pub name: String,
}
