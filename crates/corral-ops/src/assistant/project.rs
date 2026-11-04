//! Project selection at open and the effective directory shared by launch and host readback.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use corral_core::Store;
use corral_core::assistant::AssistantSession;
use serde::Serialize;

/// The deepest configured canonical checkout containing `start`, including its worktrees.
pub fn project_at(store: &Store, start: &Path) -> Result<Option<String>> {
    let start = start
        .canonicalize()
        .with_context(|| format!("resolving {}", start.display()))?;
    let root = store
        .root()
        .canonicalize()
        .context("resolving workspace root")?;
    if start == root {
        return Ok(None);
    }
    let mut selected = None;
    for name in store.config.projects.keys() {
        let path = store.project_path(name)?;
        let canonical = match path.canonicalize() {
            Ok(path) => path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error).with_context(|| format!("resolving project {name:?}")),
        };
        let depth = canonical.components().count();
        if start.starts_with(&canonical)
            && selected
                .as_ref()
                .is_none_or(|(longest, _)| depth > *longest)
        {
            selected = Some((depth, name.clone()));
        }
    }
    Ok(selected.map(|(_, name)| name))
}

#[derive(Debug, Serialize)]
pub struct Directory {
    pub path: PathBuf,
    pub branch: Option<String>,
}

/// Scratch wins only while its directory exists; an unlinked session falls back to its project.
pub fn directory(store: &Store, session: &AssistantSession) -> Result<Directory> {
    if let Some(scratch) = session.scratch.iter().find(|scratch| {
        session.linked_scratch.as_deref() == Some(scratch.slug.as_str()) && scratch.path.is_dir()
    }) {
        return Ok(Directory {
            path: scratch.path.clone(),
            branch: Some(scratch.branch.clone()),
        });
    }
    Ok(Directory {
        path: match session.project.as_deref() {
            Some(project) => store.project_path(project)?,
            None => store.root().to_owned(),
        },
        branch: None,
    })
}
