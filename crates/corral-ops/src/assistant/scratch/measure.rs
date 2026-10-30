//! Bounded disk accounting for one scratch worktree; it runs in cleanup and stats, never sync.
use std::io::ErrorKind;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;

/// Dependency and build trees, reported apart from the scratch's own files.
const DEPENDENCY_TREES: [&str; 7] = [
    "node_modules",
    "target",
    ".venv",
    "venv",
    "__pycache__",
    ".gradle",
    ".pnpm-store",
];

/// Entries visited before the walk stops and reports lower bounds.
const MAX_ENTRIES: usize = 250_000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Measured {
    /// Bytes outside dependency trees.
    pub bytes: u64,
    /// Bytes inside dependency and build trees such as `node_modules` or a local `target`.
    pub dependency_bytes: u64,
    /// The walk reached its entry bound; both counts are lower bounds.
    pub truncated: bool,
}

impl Measured {
    pub fn total(&self) -> u64 {
        self.bytes.saturating_add(self.dependency_bytes)
    }
}

/// Symlinks are never followed, so a shared Cargo target linked into a scratch is not counted;
/// entries that vanish during the walk are skipped.
pub fn measure(root: &Path) -> Result<Measured> {
    let mut measured = Measured::default();
    let mut pending = vec![(root.to_path_buf(), false)];
    let mut visited = 0usize;
    while let Some((dir, dependency)) = pending.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("measuring {}", dir.display()));
            }
        };
        for entry in entries {
            let entry = entry.with_context(|| format!("measuring {}", dir.display()))?;
            visited += 1;
            if visited > MAX_ENTRIES {
                measured.truncated = true;
                return Ok(measured);
            }
            let meta = match entry.metadata() {
                Ok(meta) => meta,
                Err(error) if error.kind() == ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("measuring {}", entry.path().display()));
                }
            };
            if meta.is_symlink() {
                continue;
            }
            if meta.is_dir() {
                let tree = dependency
                    || entry
                        .file_name()
                        .to_str()
                        .is_some_and(|name| DEPENDENCY_TREES.contains(&name));
                pending.push((entry.path(), tree));
            } else if dependency {
                measured.dependency_bytes += meta.len();
            } else {
                measured.bytes += meta.len();
            }
        }
    }
    Ok(measured)
}
