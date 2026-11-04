//! Storage classes and roots (plan T-253 section 8): the `[storage]` config section, the fixed
//! class roots under `.corral`, and the containment matrix that validates the configurable
//! cache and scratch roots against the containers they must never overlap.

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Project, parse_bytes};

/// Fixed class roots under `.corral` that every v2 open requires as real directories.
pub const FIXED_CLASS_DIRS: &[&str] = &["db", "blobs", "agent/sessions", "lock", "rollback"];

/// Every fixed path under `.corral` (section 8 table), compared by the containment matrix.
const FIXED_PATHS: &[&str] = &[
    "db",
    "db.migrating",
    "blobs",
    "agent/sessions",
    "lock",
    "rollback",
    "migration.json",
    "tasks",
    "state",
    "artifacts",
    "prompts",
    "events.jsonl",
    "lessons",
    "corral.toml",
    "pins.toml",
];

/// `[storage]`. Defaults: `cache_root` = `$XDG_CACHE_HOME/corral/<workspace-id>/cache` and
/// `scratch_root` = `.../scratch` (`$HOME/.cache` without `XDG_CACHE_HOME`); no
/// `transcript_retention_days` keeps transcripts forever; `review_scratch_warn` = `"2GiB"`;
/// `review_evidence_max` = `"64MiB"`; `blob_grace_hours` = `24`; `view_max_age` = `30`
/// seconds. Relative roots are resolved against the workspace root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StorageCfg {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_root: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scratch_root: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript_retention_days: Option<u32>,
    pub review_scratch_warn: String,
    pub review_evidence_max: String,
    pub blob_grace_hours: u64,
    pub view_max_age: u64,
}

impl Default for StorageCfg {
    fn default() -> Self {
        Self {
            cache_root: None,
            scratch_root: None,
            transcript_retention_days: None,
            review_scratch_warn: "2GiB".into(),
            review_evidence_max: "64MiB".into(),
            blob_grace_hours: 24,
            view_max_age: 30,
        }
    }
}

impl StorageCfg {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    pub fn review_scratch_warn_bytes(&self) -> Result<u64> {
        parse_bytes(&self.review_scratch_warn)
    }

    pub fn review_evidence_max_bytes(&self) -> Result<u64> {
        parse_bytes(&self.review_evidence_max)
    }

    pub fn blob_grace(&self) -> Duration {
        Duration::from_secs(self.blob_grace_hours.saturating_mul(3600))
    }

    pub fn view_max_age(&self) -> Duration {
        Duration::from_secs(self.view_max_age)
    }

    /// `None` keeps transcripts forever.
    pub fn transcript_retention(&self) -> Option<Duration> {
        self.transcript_retention_days
            .map(|days| Duration::from_secs(u64::from(days) * 86_400))
    }

    /// Config-load problems. Explicitly configured roots go through the containment matrix;
    /// derived defaults are checked when the layout is resolved. Nothing is created.
    pub(crate) fn problems(
        &self,
        workspace: &Path,
        projects: &BTreeMap<String, Project>,
    ) -> Vec<String> {
        let mut out = Vec::new();
        for (key, value) in [
            ("review_scratch_warn", &self.review_scratch_warn),
            ("review_evidence_max", &self.review_evidence_max),
        ] {
            if let Err(error) = parse_bytes(value) {
                out.push(format!("storage.{key}: {error:#}"));
            }
        }
        if self.transcript_retention_days == Some(0) {
            out.push(
                "storage.transcript_retention_days must be > 0 (omit it to keep transcripts forever)"
                    .into(),
            );
        }
        if self.blob_grace_hours == 0 {
            out.push("storage.blob_grace_hours must be > 0".into());
        }
        if self.view_max_age == 0 {
            out.push("storage.view_max_age must be > 0".into());
        }
        match Containers::of(workspace, projects, &StorageEnv::from_process()) {
            Ok(containers) => out.extend(
                self.check_roots(&containers, false)
                    .err()
                    .into_iter()
                    .flatten()
                    .map(|violation| violation.to_string()),
            ),
            Err(error) => out.push(format!("storage: {error:#}")),
        }
        out
    }

    /// Resolves both roots and checks them against the matrix. With `defaults` false, only
    /// explicitly configured roots are checked.
    fn check_roots(
        &self,
        containers: &Containers,
        defaults: bool,
    ) -> std::result::Result<(PathBuf, PathBuf), Vec<RootViolation>> {
        let mut resolved = Vec::new();
        let mut violations = Vec::new();
        for (key, explicit, leaf) in [
            ("cache_root", &self.cache_root, "cache"),
            ("scratch_root", &self.scratch_root, "scratch"),
        ] {
            let given = match explicit {
                Some(path) => Some(containers.workspace.join(path)),
                None => containers.default_base.as_ref().map(|base| base.join(leaf)),
            };
            let Some(given) = given else {
                if defaults {
                    violations.push(RootViolation::underivable(key));
                }
                resolved.push(None);
                continue;
            };
            match canonical(&given) {
                Ok(path) => resolved.push(Some((key, given, path, explicit.is_some()))),
                Err(error) => {
                    if explicit.is_some() || defaults {
                        violations.push(RootViolation::syntax(key, &given, &error));
                    }
                    resolved.push(None);
                }
            }
        }
        for (index, root) in resolved.iter().enumerate() {
            let Some((key, given, path, explicit)) = root else {
                continue;
            };
            if !explicit && !defaults {
                continue;
            }
            let other = resolved[1 - index]
                .as_ref()
                .map(|(other_key, _, other_path, _)| (*other_key, other_path.as_path()));
            if let Some(violation) = containers.violation(key, given, path, other) {
                violations.push(violation);
            }
        }
        match (violations.is_empty(), &resolved[..]) {
            (true, [Some(cache), Some(scratch)]) => Ok((cache.2.clone(), scratch.2.clone())),
            _ => Err(violations),
        }
    }
}

/// The environment a default root derives from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StorageEnv {
    pub xdg_cache_home: Option<PathBuf>,
    pub home: Option<PathBuf>,
}

impl StorageEnv {
    /// Absolute `XDG_CACHE_HOME` and `HOME` of this process; relative values are ignored.
    pub fn from_process() -> Self {
        let absolute = |name| {
            std::env::var_os(name)
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
        };
        Self {
            xdg_cache_home: absolute("XDG_CACHE_HOME"),
            home: absolute("HOME"),
        }
    }

    fn cache_home(&self) -> Option<PathBuf> {
        self.xdg_cache_home
            .clone()
            .or_else(|| self.home.as_ref().map(|home| home.join(".cache")))
    }
}

/// The first 16 hex digits of the SHA-256 of the canonical workspace root.
pub fn workspace_id(workspace: &Path) -> String {
    let root = std::fs::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf());
    Sha256::digest(root.as_os_str().as_encoded_bytes())
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// `path` with its longest existing ancestor resolved by `realpath` and the remaining
/// components appended. Relative paths, `..` and dangling symlinks are refused.
pub fn canonical(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() {
        bail!("{} is not absolute", path.display());
    }
    if path.components().any(|c| c == Component::ParentDir) {
        bail!("{} contains `..`", path.display());
    }
    let mut existing = path;
    let mut rest = Vec::new();
    loop {
        match std::fs::canonicalize(existing) {
            Ok(resolved) => {
                return Ok(rest.iter().rev().fold(resolved, |acc, name| acc.join(name)));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if std::fs::symlink_metadata(existing).is_ok() {
                    bail!("{} is a dangling symlink", existing.display());
                }
                let (Some(parent), Some(name)) = (existing.parent(), existing.file_name()) else {
                    bail!("no ancestor of {} exists", path.display());
                };
                rest.push(name.to_owned());
                existing = parent;
            }
            Err(error) => {
                return Err(error).with_context(|| format!("resolve {}", existing.display()));
            }
        }
    }
}

/// A section 8 matrix row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatrixRow {
    /// `C` and every fixed class root: refuse equal, inside and containing.
    CorralAndFixedRoots,
    /// `P`, `P/.worktrees`: refuse equal, inside and containing.
    Project,
    /// `W`: refuse equal and containing; inside is allowed.
    Workspace,
    /// `$HOME`, `/`: refuse equal and containing; inside is allowed.
    HomeAndFilesystemRoot,
    /// The other configurable root: refuse equal, inside and containing.
    OtherConfigurableRoot,
}

impl MatrixRow {
    pub fn label(self) -> &'static str {
        match self {
            Self::CorralAndFixedRoots => "`C` and every fixed class root",
            Self::Project => "`P`, `P/.worktrees`",
            Self::Workspace => "`W`",
            Self::HomeAndFilesystemRoot => "`$HOME`, `/`",
            Self::OtherConfigurableRoot => "the other configurable root",
        }
    }

    fn refuses(self, relation: Relation) -> bool {
        match self {
            Self::Workspace | Self::HomeAndFilesystemRoot => relation != Relation::Inside,
            _ => true,
        }
    }
}

/// How a configurable root R relates to another path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relation {
    Equal,
    /// R is inside it.
    Inside,
    /// It is inside R.
    Contains,
}

impl Relation {
    fn of(root: &Path, other: &Path) -> Option<Self> {
        if root == other {
            Some(Self::Equal)
        } else if root.starts_with(other) {
            Some(Self::Inside)
        } else if other.starts_with(root) {
            Some(Self::Contains)
        } else {
            None
        }
    }

    fn column(self) -> &'static str {
        match self {
            Self::Equal => "equal",
            Self::Inside => "R inside it",
            Self::Contains => "it inside R",
        }
    }
}

/// One refused configurable root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootViolation {
    pub key: &'static str,
    pub root: PathBuf,
    /// The container or root it collides with, and how; `None` for a path that cannot be
    /// resolved at all.
    pub collision: Option<(String, PathBuf, MatrixRow, Relation)>,
    pub detail: String,
}

impl RootViolation {
    fn syntax(key: &'static str, given: &Path, error: &anyhow::Error) -> Self {
        Self {
            key,
            root: given.to_path_buf(),
            collision: None,
            detail: format!("{error:#}"),
        }
    }

    fn underivable(key: &'static str) -> Self {
        Self {
            key,
            root: PathBuf::new(),
            collision: None,
            detail: "no default: neither XDG_CACHE_HOME nor HOME is an absolute path".into(),
        }
    }
}

impl fmt::Display for RootViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.collision {
            None => write!(
                f,
                "storage.{} {} refused: {}",
                self.key,
                self.root.display(),
                self.detail
            ),
            Some((name, path, row, relation)) => write!(
                f,
                "storage.{} {} ({}) {} {name} {}; refused by the plan section 8 containment matrix, row {}, column \"{}\"",
                self.key,
                self.root.display(),
                self.detail,
                match relation {
                    Relation::Equal => "equals",
                    Relation::Inside => "is inside",
                    Relation::Contains => "contains",
                },
                path.display(),
                row.label(),
                relation.column()
            ),
        }
    }
}

/// The section 8 containers and fixed roots, canonicalized.
#[derive(Debug, Clone)]
struct Containers {
    workspace: PathBuf,
    corral: PathBuf,
    default_base: Option<PathBuf>,
    /// In matrix order: fixed roots and `C`, projects, `W`, `$HOME` and `/`.
    entries: Vec<(String, PathBuf, MatrixRow)>,
}

impl Containers {
    fn of(
        workspace: &Path,
        projects: &BTreeMap<String, Project>,
        env: &StorageEnv,
    ) -> Result<Self> {
        let workspace = canonical(&std::path::absolute(workspace)?)
            .with_context(|| format!("resolve workspace root {}", workspace.display()))?;
        let corral = canonical(&workspace.join(".corral"))?;
        let mut entries = Vec::new();
        for fixed in FIXED_PATHS {
            entries.push((
                format!("fixed class root .corral/{fixed}"),
                canonical(&corral.join(fixed))?,
                MatrixRow::CorralAndFixedRoots,
            ));
        }
        entries.push((
            ".corral".into(),
            corral.clone(),
            MatrixRow::CorralAndFixedRoots,
        ));
        for (key, project) in projects {
            let root = canonical(&workspace.join(&project.path))?;
            let worktrees = canonical(&root.join(".worktrees"))?;
            entries.push((format!("project {key}"), root, MatrixRow::Project));
            entries.push((
                format!("project {key}'s .worktrees"),
                worktrees,
                MatrixRow::Project,
            ));
        }
        entries.push((
            "the workspace root".into(),
            workspace.clone(),
            MatrixRow::Workspace,
        ));
        if let Some(home) = &env.home {
            entries.push((
                "$HOME".into(),
                canonical(home)?,
                MatrixRow::HomeAndFilesystemRoot,
            ));
        }
        entries.push((
            "/".into(),
            PathBuf::from("/"),
            MatrixRow::HomeAndFilesystemRoot,
        ));
        let default_base = env
            .cache_home()
            .map(|cache| cache.join("corral").join(workspace_id(&workspace)));
        Ok(Self {
            workspace,
            corral,
            default_base,
            entries,
        })
    }

    /// The first refusal for `root`: equal collisions first, then R inside, then containing.
    fn violation(
        &self,
        key: &'static str,
        given: &Path,
        root: &Path,
        other: Option<(&str, &Path)>,
    ) -> Option<RootViolation> {
        let other = other.map(|(other_key, path)| {
            (
                format!("storage.{other_key}"),
                path.to_path_buf(),
                MatrixRow::OtherConfigurableRoot,
            )
        });
        let candidates: Vec<&(String, PathBuf, MatrixRow)> =
            other.iter().chain(self.entries.iter()).collect();
        for relation in [Relation::Equal, Relation::Inside, Relation::Contains] {
            let depth = |path: &PathBuf| path.components().count();
            let refusing = candidates
                .iter()
                .filter(|(_, path, row)| {
                    Relation::of(root, path) == Some(relation) && row.refuses(relation)
                })
                .copied();
            // The nearest container names the collision: the deepest one R is inside, the
            // shallowest one inside R; ties keep matrix order.
            let nearest = match relation {
                Relation::Contains => refusing.min_by_key(|(_, path, _)| depth(path)),
                _ => refusing.rev().max_by_key(|(_, path, _)| depth(path)),
            };
            if let Some((name, path, row)) = nearest {
                return Some(RootViolation {
                    key,
                    root: given.to_path_buf(),
                    collision: Some((name.clone(), path.clone(), *row, relation)),
                    detail: format!("resolves to {}", root.display()),
                });
            }
        }
        None
    }
}

/// A storage class with its own root and deleter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageClass {
    Cache,
    Scratch,
    /// `.corral/agent/sessions` (T-252 agent runtime).
    AgentSessions,
}

impl fmt::Display for StorageClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Cache => "cache",
            Self::Scratch => "scratch",
            Self::AgentSessions => "agent-sessions",
        })
    }
}

/// The validated roots of one workspace. Resolving creates nothing.
#[derive(Debug, Clone)]
pub struct StorageLayout {
    containers: Containers,
    cache_root: PathBuf,
    scratch_root: PathBuf,
}

impl StorageLayout {
    /// Resolves `[storage]` for `workspace`, defaults included, and checks both roots against
    /// the containment matrix, reporting every violation.
    pub fn resolve(
        workspace: &Path,
        cfg: &StorageCfg,
        projects: &BTreeMap<String, Project>,
        env: &StorageEnv,
    ) -> std::result::Result<Self, Vec<RootViolation>> {
        let containers = Containers::of(workspace, projects, env).map_err(|error| {
            vec![RootViolation::syntax(
                "cache_root",
                workspace,
                &error.context("resolve the workspace containers"),
            )]
        })?;
        let (cache_root, scratch_root) = cfg.check_roots(&containers, true)?;
        Ok(Self {
            containers,
            cache_root,
            scratch_root,
        })
    }

    pub fn workspace(&self) -> &Path {
        &self.containers.workspace
    }

    pub fn corral_dir(&self) -> &Path {
        &self.containers.corral
    }

    pub fn cache_root(&self) -> &Path {
        &self.cache_root
    }

    pub fn scratch_root(&self) -> &Path {
        &self.scratch_root
    }

    pub fn class_root(&self, class: StorageClass) -> PathBuf {
        match class {
            StorageClass::Cache => self.cache_root.clone(),
            StorageClass::Scratch => self.scratch_root.clone(),
            StorageClass::AgentSessions => self.containers.corral.join("agent").join("sessions"),
        }
    }

    /// The container or protected fixed root `path` equals, if any: containers, `C/lock`,
    /// `C/rollback`, and every fixed path other than `agent/sessions`.
    pub fn protected(&self, path: &Path) -> Option<&str> {
        let sessions = self.class_root(StorageClass::AgentSessions);
        self.containers
            .entries
            .iter()
            .find(|(_, root, _)| root == path && *root != sessions)
            .map(|(name, _, _)| name.as_str())
    }
}

/// Checks the fixed class roots under `corral_dir` before anything is written: each must be a
/// real directory (`lstat`) or absent. Only then are the absent ones created.
pub fn prepare_fixed_roots(corral_dir: &Path) -> Result<()> {
    let mut missing = Vec::new();
    for dir in ["db", "blobs", "agent", "agent/sessions", "lock", "rollback"] {
        let path = corral_dir.join(dir);
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink() => bail!(
                "refusing to open the store: fixed class root .corral/{dir} ({}) is a symlink",
                path.display()
            ),
            Ok(meta) if !meta.is_dir() => bail!(
                "refusing to open the store: fixed class root .corral/{dir} ({}) is not a directory",
                path.display()
            ),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => missing.push(path),
            Err(error) => {
                return Err(error).with_context(|| format!("lstat {}", path.display()));
            }
        }
    }
    for path in missing {
        std::fs::create_dir(&path).with_context(|| format!("create {}", path.display()))?;
    }
    Ok(())
}
