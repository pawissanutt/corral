//! Shared projections of immutable repository facts. The daemon owns their assembly.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::facts::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemRow {
    pub project: String,
    pub head: Option<String>,
    pub status: SystemStatus,
    /// Name of the configured extension, if any.
    pub extension: Option<String>,
    /// Categories supplied by the current head's `facts` snapshot answer.
    /// `None` means no successful answer; `Some([])` is an empty answer.
    pub snapshot_provided: Option<Vec<String>>,
    /// Categories supplied by the project's latest `facts` review answer.
    /// `None` means no successful answer; `Some([])` is an empty answer.
    pub review_provided: Option<Vec<String>>,
    pub last_extension_error: Option<String>,
    pub error: Option<String>,
    /// Changes when a detail for this project changes; use conditional detail reads to refresh.
    pub revision: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemStatus {
    Building,
    Ready,
}

/// Repository-relative node identities. The empty string names a root container.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SystemAt {
    Container { path: String },
    Module { module: ModuleId },
    File { path: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemMap {
    pub project: String,
    pub head: Option<String>,
    pub status: SystemStatus,
    pub error: Option<String>,
    /// None when no extension has answered this detail.
    pub provider: Option<SystemProvider>,
    pub coverage: Option<Coverage>,
    /// Rows omitted to stay within the wire budget.
    pub omitted_rows: usize,
    pub rows: Vec<SystemMapRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SystemMapRow {
    Container {
        container: ContainerFact,
        stats: SystemNodeStats,
    },
    PackageEdge {
        edge: PackageEdge,
    },
    Module {
        module: ModuleId,
        stats: SystemNodeStats,
    },
    Neighbor {
        container: ContainerFact,
    },
    ModuleReference {
        reference: Reference<ModuleId>,
    },
    Spawn {
        container: String,
        spawn: SpawnFact,
    },
    File {
        path: String,
        language: String,
        lines: u64,
        test: bool,
        read_status: ReadStatus,
    },
    Definition {
        definition: DefinitionFact,
    },
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemNodeStats {
    pub lines: u64,
    /// Physical lines per language.
    pub languages: BTreeMap<String, u64>,
    pub complete: u64,
    pub unsupported: u64,
    pub failed: u64,
    pub days_7: SystemActivity,
    pub days_30: SystemActivity,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemActivity {
    pub commits: u64,
    pub added: u64,
    pub removed: u64,
    pub last_commit: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemProvider {
    pub extension: String,
    pub coverage: ProviderCoverage,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemReview {
    pub task: String,
    pub project: String,
    pub base: String,
    /// The submission predates persisted base coordinates; the base was inferred.
    pub base_estimated: bool,
    pub head: String,
    pub status: SystemStatus,
    pub error: Option<String>,
    pub provider: Option<SystemProvider>,
    pub skipped_non_utf8_paths: u64,
    /// Rows omitted to stay within the wire budget.
    pub omitted_rows: usize,
    pub categories: Vec<SystemCategorySummary>,
    pub rows: Vec<SystemReviewRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemCategorySummary {
    pub category: SystemImpactCategory,
    pub provided: bool,
    /// Total known rows before wire omissions. Provider omissions are in `provider.coverage`.
    pub total: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemImpactCategory {
    PackageDependencies,
    ModuleReferences,
    Definitions,
    CallEdges,
    ExternalProcesses,
    Dependents,
    Tests,
    NotRead,
}

impl SystemImpactCategory {
    pub const ALL: [Self; 8] = [
        Self::PackageDependencies,
        Self::ModuleReferences,
        Self::Definitions,
        Self::CallEdges,
        Self::ExternalProcesses,
        Self::Dependents,
        Self::Tests,
        Self::NotRead,
    ];
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SystemReviewRow {
    /// Project-level geometry at the submitted head, including untouched nodes.
    /// Activity windows are not computed for a review.
    Map {
        row: SystemMapRow,
    },
    Container {
        path: String,
        added: u64,
        removed: u64,
    },
    Module {
        module: ModuleId,
        added: u64,
        removed: u64,
    },
    File {
        path: String,
        container: String,
        module: ModuleId,
        added: u64,
        removed: u64,
        binary: bool,
        /// The change deleted the file: it exists at the base only.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        deleted: bool,
    },
    /// A changed package or workspace manifest, independent of dependency deltas.
    Manifest {
        path: String,
    },
    Dependency {
        before: Option<DependencyFact>,
        after: Option<DependencyFact>,
    },
    PackageEdge {
        before: Option<PackageEdge>,
        after: Option<PackageEdge>,
    },
    ModuleReference {
        from: ModuleId,
        to: ModuleId,
        before: u64,
        after: u64,
    },
    Definition {
        change: ChangeKind,
        path: String,
        name: String,
        line: u64,
    },
    CallEdge {
        from: ModuleId,
        to: ModuleId,
        added: u64,
        removed: u64,
        changed: u64,
        path: String,
        line: u64,
    },
    Spawn {
        path: String,
        process: String,
        before: u64,
        after: u64,
    },
    Dependent {
        dependent: DependentFact,
        /// The module holding the dependent at the head, when the head tree has its file.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        module: Option<ModuleId>,
    },
    TestFile {
        path: String,
    },
    TestCandidate {
        candidate: TestCandidate,
    },
    NotRead {
        path: String,
        read_status: ReadStatus,
    },
}
