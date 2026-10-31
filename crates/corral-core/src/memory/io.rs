use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use super::{ENTRY_BYTES, MemoryScope, MemorySelector, hash, key};
use crate::Store;

/// Bound directory enumeration as well as individual file reads. Refuse oversized
/// scopes rather than silently omitting unknown applicable projection input.
const MAX_SCOPE_FILES: usize = 4096;

fn regular(path: &Path, directory: bool) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(meta) if (directory && meta.is_dir()) || (!directory && meta.is_file()) => Ok(true),
        Ok(_) => bail!(
            "{} must be a regular {} (no symlinks or special files)",
            path.display(),
            if directory { "directory" } else { "file" }
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e).with_context(|| format!("inspecting memory {}", path.display())),
    }
}

pub(super) fn scope_path(store: &Store, scope: &MemoryScope, create: bool) -> Result<PathBuf> {
    // The Store root is trusted, but none of the memory subtree may redirect IO.
    regular(store.dir(), true)?;
    let parts = match scope {
        MemoryScope::Workspace => vec!["memory", "workspace"],
        MemoryScope::Project(project) => {
            key(project)?;
            vec!["memory", "projects", project.as_str()]
        }
        MemoryScope::Role(role) => vec!["memory", "roles", role.as_str()],
    };
    let mut path = store.dir().to_path_buf();
    for part in parts {
        path.push(part);
        if !regular(&path, true)? && create {
            fs::create_dir(&path).with_context(|| format!("creating memory {}", path.display()))?;
            regular(&path, true)?;
        }
    }
    Ok(path)
}

pub(super) fn entry_path(
    store: &Store,
    selector: &MemorySelector,
    create: bool,
) -> Result<PathBuf> {
    key(&selector.id)?;
    let path = scope_path(store, &selector.scope, create)?.join(format!("{}.md", selector.id));
    regular(&path, false)?;
    Ok(path)
}

pub(super) fn read(path: &Path) -> Result<Option<String>> {
    if !regular(path, false)? {
        return Ok(None);
    }
    let file =
        fs::File::open(path).with_context(|| format!("opening memory {}", path.display()))?;
    let metadata = file
        .metadata()
        .with_context(|| format!("inspecting open memory {}", path.display()))?;
    if !metadata.is_file() || metadata.len() > ENTRY_BYTES as u64 {
        bail!(
            "{} must be a regular file of at most {ENTRY_BYTES} bytes",
            path.display()
        );
    }
    let mut bytes = Vec::new();
    file.take(ENTRY_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading memory {}", path.display()))?;
    if bytes.len() > ENTRY_BYTES {
        bail!("memory {} exceeds {ENTRY_BYTES} bytes", path.display());
    }
    String::from_utf8(bytes)
        .with_context(|| format!("memory {} is not UTF-8", path.display()))
        .map(Some)
}

/// Use an exclusively created regular file in the destination directory. Never
/// open the predictable `<entry>.md.tmp` path: it may be a symlink, hard link or
/// FIFO planted by another writer. NamedTempFile cleans up on every error path;
/// persist atomically replaces the destination without following its contents.
pub(super) fn atomic_write(path: &Path, content: &str) -> Result<()> {
    let parent = path
        .parent()
        .context("memory entry requires a parent directory")?;
    let mut temp = tempfile::NamedTempFile::new_in(parent).with_context(|| {
        format!(
            "exclusively creating temporary memory in {}",
            parent.display()
        )
    })?;
    temp.write_all(content.as_bytes())
        .with_context(|| format!("writing temporary memory for {}", path.display()))?;
    temp.persist(path)
        .with_context(|| format!("atomically replacing memory {}", path.display()))?;
    Ok(())
}

pub(super) fn check_hash(path: &Path, expected: Option<&str>) -> Result<()> {
    let actual = read(path)?.map(|text| hash(&text));
    if actual.as_deref() != expected {
        bail!(
            "memory hash conflict at {}: re-read the entry (create requires explicit absence)",
            path.display()
        );
    }
    Ok(())
}

pub(super) fn ids(store: &Store, scope: &MemoryScope) -> Result<Vec<String>> {
    let path = scope_path(store, scope, false)?;
    let directory = match fs::read_dir(&path) {
        Ok(directory) => directory,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("listing memory {}", path.display())),
    };
    let mut ids = Vec::new();
    for (index, entry) in directory.enumerate() {
        if index >= MAX_SCOPE_FILES {
            bail!(
                "memory scope {} exceeds {MAX_SCOPE_FILES} files",
                path.display()
            );
        }
        let entry = entry.with_context(|| format!("listing memory {}", path.display()))?;
        let entry_path = entry.path();
        // Non-Markdown editor files are not entries. Markdown paths must be valid,
        // regular entries; never silently skip malformed memory.
        if entry_path.extension().is_none_or(|ext| ext != "md") {
            continue;
        }
        regular(&entry_path, false)?;
        let id = entry_path
            .file_stem()
            .and_then(|s| s.to_str())
            .with_context(|| format!("invalid memory filename {}", entry_path.display()))?;
        key(id).with_context(|| format!("invalid memory filename {}", entry_path.display()))?;
        ids.push(id.to_owned());
    }
    ids.sort();
    Ok(ids)
}
