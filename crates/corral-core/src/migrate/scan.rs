//! The legacy tree: its walk, the M0 manifest (every legacy file outside CACHEDIR.TAG trees,
//! with size, mtime and content hash) and the dry run's classification and counts.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read};
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::blockers::{self, Blocker};
use super::classify::{self, Disposition};
use super::journal::{Journal, read_fence};
use super::{DB, DISCARD, JOURNAL, MANIFEST, MIGRATING, durable};

pub use classify::Area;

/// Top-level `.corral` entries that are not legacy store files: v2 roots, the migration's own
/// files, and the configuration and lessons both layouts read in place.
const NOT_LEGACY: &[&str] = &[
    DB,
    MIGRATING,
    DISCARD,
    "blobs",
    "agent",
    "lock",
    "rollback",
    JOURNAL,
    "migration.json.tmp",
    MANIFEST,
    "migration.m0.json.tmp",
    "corral.toml",
    "pins.toml",
    ".gitignore",
    "lessons",
];

/// Owner template overrides v2 still reads in place (`Store::template_override`).
const SHARED_FILES: &[&str] = &["prompts/run.md", "prompts/review.md"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    File,
    Symlink,
    Special,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestEntry {
    /// Relative to `.corral`, `/`-separated.
    pub path: String,
    pub kind: EntryKind,
    pub size: u64,
    pub mtime_ns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

/// A directory tagged with `CACHEDIR.TAG` (Cargo build output): left in place, never imported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildRoot {
    pub path: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub entries: Vec<ManifestEntry>,
    pub build_roots: Vec<BuildRoot>,
}

fn mtime_ns(meta: &fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
}

fn rel(path: &Path, corral: &Path) -> String {
    path.strip_prefix(corral)
        .unwrap_or(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn sorted_children(dir: &Path) -> Result<Vec<std::path::PathBuf>> {
    let mut children = fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<io::Result<Vec<_>>>()
        .with_context(|| format!("reading {}", dir.display()))?;
    children.sort();
    Ok(children)
}

fn tree_bytes(dir: &Path) -> Result<u64> {
    let mut total = 0;
    for child in sorted_children(dir)? {
        let meta = fs::symlink_metadata(&child)
            .with_context(|| format!("inspecting {}", child.display()))?;
        total += if meta.is_dir() {
            tree_bytes(&child)?
        } else {
            meta.len()
        };
    }
    Ok(total)
}

fn walk_dir(corral: &Path, dir: &Path, out: &mut Manifest) -> Result<()> {
    for child in sorted_children(dir)? {
        let path = rel(&child, corral);
        if dir == corral && NOT_LEGACY.contains(&path.as_str()) {
            continue;
        }
        if path == super::FENCE_PATH
            || path == format!("{}.tmp", super::FENCE_PATH)
            || SHARED_FILES.contains(&path.as_str())
        {
            continue;
        }
        let meta = fs::symlink_metadata(&child)
            .with_context(|| format!("inspecting {}", child.display()))?;
        let file_type = meta.file_type();
        if file_type.is_dir() {
            if child.join("CACHEDIR.TAG").is_file() {
                out.build_roots.push(BuildRoot {
                    bytes: tree_bytes(&child)?,
                    path,
                });
            } else {
                walk_dir(corral, &child, out)?;
            }
            continue;
        }
        let kind = if file_type.is_symlink() {
            EntryKind::Symlink
        } else if file_type.is_file() {
            EntryKind::File
        } else {
            EntryKind::Special
        };
        out.entries.push(ManifestEntry {
            path,
            kind,
            size: meta.len(),
            mtime_ns: mtime_ns(&meta),
            sha256: None,
        });
    }
    Ok(())
}

/// Every legacy entry under `corral` (no content hashes), sorted by path.
pub(crate) fn walk(corral: &Path) -> Result<Manifest> {
    let mut out = Manifest::default();
    walk_dir(corral, corral, &mut out)?;
    Ok(out)
}

pub(crate) fn sha256_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0; 1 << 16];
    loop {
        let n = file
            .read(&mut buf)
            .with_context(|| format!("reading {}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn entry_hash(corral: &Path, entry: &ManifestEntry) -> Result<Option<String>> {
    let path = corral.join(&entry.path);
    Ok(match entry.kind {
        EntryKind::File => Some(sha256_file(&path)?),
        EntryKind::Symlink => {
            let target =
                fs::read_link(&path).with_context(|| format!("reading link {}", path.display()))?;
            Some(format!(
                "{:x}",
                Sha256::digest(target.as_os_str().as_encoded_bytes())
            ))
        }
        EntryKind::Special => None,
    })
}

impl Manifest {
    /// M0: the walk with every entry's content hash.
    pub(crate) fn capture(corral: &Path) -> Result<Manifest> {
        let mut manifest = walk(corral)?;
        for entry in &mut manifest.entries {
            entry.sha256 = entry_hash(corral, entry)?;
        }
        Ok(manifest)
    }

    /// Writes the manifest durably and returns its digest (the journal's `m0_digest`).
    pub(crate) fn save(&self, corral: &Path) -> Result<String> {
        let bytes = serde_json::to_vec(self).context("serializing the M0 manifest")?;
        durable::write(&corral.join(MANIFEST), &bytes, || Ok(()))?;
        Ok(format!("{:x}", Sha256::digest(&bytes)))
    }

    /// The saved manifest, refused unless its digest is `digest`.
    pub(crate) fn load(corral: &Path, digest: &str) -> Result<Manifest> {
        let path = corral.join(MANIFEST);
        let bytes = fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        let found = format!("{:x}", Sha256::digest(&bytes));
        anyhow::ensure!(
            found == digest,
            "{} has digest {found}, the journal records {digest}",
            path.display()
        );
        serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))
    }

    /// Differences of the current tree from this manifest by path, kind, size and mtime, outside
    /// CACHEDIR.TAG trees; empty when the legacy files are unchanged.
    pub(crate) fn differences(&self, now: &Manifest) -> Vec<String> {
        let before: BTreeMap<&str, &ManifestEntry> =
            self.entries.iter().map(|e| (e.path.as_str(), e)).collect();
        let after: BTreeMap<&str, &ManifestEntry> =
            now.entries.iter().map(|e| (e.path.as_str(), e)).collect();
        let mut out = Vec::new();
        for (path, was) in &before {
            match after.get(path) {
                None => out.push(format!("{path}: removed")),
                Some(is)
                    if is.kind != was.kind
                        || is.size != was.size
                        || is.mtime_ns != was.mtime_ns =>
                {
                    out.push(format!(
                        "{path}: changed ({} bytes -> {} bytes)",
                        was.size, is.size
                    ));
                }
                Some(_) => {}
            }
        }
        for path in after.keys().filter(|path| !before.contains_key(*path)) {
            out.push(format!("{path}: added"));
        }
        out
    }

    /// Build roots of this manifest that are gone now (reclaimed by `storage gc`).
    pub(crate) fn reclaimed_build_roots(&self, now: &Manifest) -> Vec<BuildRoot> {
        self.build_roots
            .iter()
            .filter(|root| !now.build_roots.iter().any(|r| r.path == root.path))
            .cloned()
            .collect()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Count {
    pub files: u64,
    pub bytes: u64,
}

impl Count {
    fn add(&mut self, bytes: u64) {
        self.files += 1;
        self.bytes += bytes;
    }
}

/// One classified legacy file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Classified {
    pub path: String,
    pub area: Area,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct DryRun {
    pub state: String,
    pub fence: Option<String>,
    pub ok: bool,
    pub blockers: Vec<Blocker>,
    /// Records that look live but are history (process-lost gate markers), reported only.
    pub history: Vec<Blocker>,
    pub unclassified: Vec<String>,
    pub tasks_by_schema: BTreeMap<String, u64>,
    pub runs: u64,
    pub review_runs: u64,
    pub plan_runs: u64,
    pub event_lines: u64,
    pub areas: BTreeMap<String, Count>,
    pub records_by_area: BTreeMap<String, u64>,
    pub artifacts_raw_bytes: u64,
    pub artifacts_stored_bytes: u64,
    pub legacy_review_files: Count,
    pub build_output: Vec<BuildRoot>,
    pub build_output_bytes: u64,
    pub tmp: Vec<String>,
}

/// Classifies every walked entry; unknown ones are returned separately.
pub(crate) fn classify_all(manifest: &Manifest) -> (Vec<Classified>, Vec<String>) {
    let mut known = Vec::new();
    let mut unknown = Vec::new();
    for entry in &manifest.entries {
        match (entry.kind, classify::classify(&entry.path)) {
            (EntryKind::Special, _) | (_, None) => unknown.push(entry.path.clone()),
            (_, Some(area)) => known.push(Classified {
                path: entry.path.clone(),
                area,
            }),
        }
    }
    (known, unknown)
}

pub(crate) fn dry_run(corral: &Path) -> Result<DryRun> {
    let journal = Journal::read(corral)?;
    let mut out = DryRun {
        state: journal
            .as_ref()
            .map_or_else(|| "legacy".to_owned(), |j| j.state.as_str().to_owned()),
        fence: read_fence(corral)?,
        ..DryRun::default()
    };
    let manifest = walk(corral)?;
    let sizes: BTreeMap<&str, u64> = manifest
        .entries
        .iter()
        .map(|e| (e.path.as_str(), e.size))
        .collect();
    let (known, unknown) = classify_all(&manifest);
    out.unclassified = unknown;
    for item in &known {
        let bytes = sizes.get(item.path.as_str()).copied().unwrap_or(0);
        out.areas
            .entry(item.area.name.to_owned())
            .or_default()
            .add(bytes);
        match item.area.disposition {
            Disposition::Record | Disposition::Authorization | Disposition::Inbox => {
                *out.records_by_area
                    .entry(item.area.name.to_owned())
                    .or_default() += 1;
            }
            Disposition::Artifact => out.artifacts_stored_bytes += bytes,
            Disposition::Evidence if item.path.starts_with("state/reviews/") => {
                out.legacy_review_files.add(bytes);
            }
            Disposition::Tmp => out.tmp.push(item.path.clone()),
            _ => {}
        }
    }
    out.build_output_bytes = manifest.build_roots.iter().map(|r| r.bytes).sum();
    out.build_output = manifest.build_roots;
    let legacy = blockers::LegacyBoard::read(corral)?;
    out.event_lines = legacy.event_lines;
    for (task, schema) in &legacy.tasks {
        *out.tasks_by_schema.entry(schema.clone()).or_default() += 1;
        out.runs += task.task.runs.len() as u64;
        out.review_runs += task
            .task
            .runs
            .iter()
            .map(|run| run.review_runs.len() as u64)
            .sum::<u64>();
        out.plan_runs += task.task.plan_runs.len() as u64;
        out.artifacts_raw_bytes += task.task.artifacts.iter().map(|a| a.bytes).sum::<u64>();
    }
    let (live, history) = legacy.blockers(corral)?;
    out.blockers = live;
    out.history = history;
    out.ok = out.blockers.is_empty() && out.unclassified.is_empty();
    Ok(out)
}
