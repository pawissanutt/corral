//! Structural facts shared by the provider, daemon and protocol clients.
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

mod data;
pub use data::*;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeUnitKind {
    Binary,
    Script,
    Service,
    Image,
    Unit,
    Job,
}

/// Stable across edits that move a declaration within the same file.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RuntimeUnitId {
    pub kind: RuntimeUnitKind,
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeUnit {
    pub id: RuntimeUnitId,
    pub line: u64,
    /// Owning repository-relative container, empty for root.
    pub container: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry_point: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build_context: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dockerfile: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<String>,
    /// Literal compose names, including any not declared in the same file.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RuntimeSite {
    pub path: String,
    pub line: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeEdgeKind {
    Starts,
    Runs,
    DependsOn,
    Builds,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeBasis {
    Declared,
    Syntax,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RuntimeSource {
    Container { path: String },
    Unit { id: RuntimeUnitId },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RuntimeEdge {
    pub kind: RuntimeEdgeKind,
    pub from: RuntimeSource,
    pub to: RuntimeUnitId,
    pub basis: RuntimeBasis,
    pub site: RuntimeSite,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeFacts {
    pub units: Vec<RuntimeUnit>,
    pub edges: Vec<RuntimeEdge>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Command,
    Route,
    Operation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryDetector {
    Clap,
    Axum,
    Express,
    NpmBin,
    OperationEnum,
    Extension,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EntryPoint {
    pub kind: EntryKind,
    pub name: String,
    pub path: String,
    pub line: u64,
    pub owner: RuntimeSource,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryPoints {
    pub entries: Vec<EntryPoint>,
    /// One row per container. An empty set means no detector applied, not zero entry points.
    pub coverage: BTreeMap<String, BTreeSet<EntryDetector>>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryPointDelta {
    pub added: Vec<EntryPoint>,
    pub removed: Vec<EntryPoint>,
    /// Containers covered by extension entry rows in either snapshot.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub extension_containers: BTreeSet<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeDelta {
    pub units: Vec<RuntimeUnitChange>,
    pub edges: Vec<RuntimeEdgeChange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeUnitChange {
    pub before: Option<RuntimeUnit>,
    pub after: Option<RuntimeUnit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeEdgeChange {
    pub before: Option<RuntimeEdge>,
    pub after: Option<RuntimeEdge>,
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
