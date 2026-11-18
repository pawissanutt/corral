use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use anyhow::{Context, Result};
pub use corral_types::Artifact;
use jiff::Timestamp;
use rusqlite::{OptionalExtension, params};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{
    Engine,
    error::refuse,
    events::{EventKind, NewEvent},
    model::Actor,
    store::{Store, documents, encode},
};

impl Store {
    pub fn insert_artifact(&self, artifact: &Artifact) -> Result<()> {
        self.connection
            .execute(
                "INSERT INTO artifacts(task,ordinal,sha256,document) VALUES(?1,?2,?3,?4)",
                params![
                    artifact.task,
                    artifact.id,
                    artifact.sha256,
                    encode(artifact)?
                ],
            )
            .context("insert artifact metadata")?;
        Ok(())
    }

    pub fn next_artifact_id(&self, task: &str) -> Result<u32> {
        self.connection
            .query_row(
                "SELECT COALESCE(MAX(ordinal),0)+1 FROM artifacts WHERE task=?1",
                [task],
                |row| row.get(0),
            )
            .context("allocate artifact reference")
    }

    pub fn list_artifacts(&self, task: Option<&str>) -> Result<Vec<Artifact>> {
        let mut statement = self.connection.prepare("SELECT document FROM artifacts WHERE (?1 IS NULL OR task=?1) ORDER BY task,ordinal").context("prepare artifact listing")?;
        documents(&mut statement, [task])
    }

    pub fn get_artifact(&self, reference: &str) -> Result<Option<Artifact>> {
        let (task, id) = parse_reference(reference)?;
        let document: Option<String> = self
            .connection
            .query_row(
                "SELECT document FROM artifacts WHERE task=?1 AND ordinal=?2",
                params![task, id],
                |row| row.get(0),
            )
            .optional()
            .context("look up artifact reference")?;
        document
            .map(|text| serde_json::from_str(&text).context("decode artifact metadata"))
            .transpose()
    }
}

pub(crate) struct ArtifactTarget<'a> {
    pub task: &'a str,
    pub run: Option<&'a str>,
    pub actor: Actor,
}

impl Engine {
    pub fn add_artifact(
        &self,
        task: &str,
        path: &Path,
        kind: Option<&str>,
        title: Option<&str>,
    ) -> Result<Artifact> {
        self.store.transaction(|store| {
            store.require_task(task)?;
            self.add_artifact_file_in(
                store,
                self.artifact_target(task),
                path,
                kind,
                title,
                Some(self.config.artifacts.max_bytes),
            )
        })
    }

    pub fn add_artifact_bytes(
        &self,
        task: &str,
        bytes: &[u8],
        kind: Option<&str>,
        title: &str,
    ) -> Result<Artifact> {
        let max = self.config.artifacts.max_bytes;
        if bytes.len() as u64 > max {
            return Err(size_refusal(task, bytes.len() as u64, max));
        }
        self.store.transaction(|store| {
            store.require_task(task)?;
            self.add_artifact_in(store, self.artifact_target(task), bytes, kind, title)
        })
    }

    fn artifact_target<'a>(&self, task: &'a str) -> ArtifactTarget<'a> {
        ArtifactTarget {
            task,
            run: None,
            actor: self.actor,
        }
    }

    pub(crate) fn add_artifact_file_in(
        &self,
        store: &Store,
        target: ArtifactTarget<'_>,
        path: &Path,
        kind: Option<&str>,
        title: Option<&str>,
        max: Option<u64>,
    ) -> Result<Artifact> {
        let file = File::open(path).with_context(|| format!("open artifact {}", path.display()))?;
        let length = file.metadata().context("read artifact size")?.len();
        if let Some(max) = max
            && length > max
        {
            return Err(size_refusal(target.task, length, max));
        }
        let mut bytes = Vec::new();
        file.take(max.map_or(u64::MAX, |n| n.saturating_add(1)))
            .read_to_end(&mut bytes)
            .context("read artifact bytes")?;
        if let Some(max) = max
            && bytes.len() as u64 > max
        {
            return Err(size_refusal(target.task, bytes.len() as u64, max));
        }
        let filename = path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy();
        self.add_artifact_in(store, target, &bytes, kind, title.unwrap_or(&filename))
    }

    pub(crate) fn add_artifact_in(
        &self,
        store: &Store,
        target: ArtifactTarget<'_>,
        bytes: &[u8],
        kind: Option<&str>,
        title: &str,
    ) -> Result<Artifact> {
        let hash = format!("{:x}", Sha256::digest(bytes));
        let path = self.blob_path(&hash);
        if !path.try_exists().context("inspect artifact blob")? {
            publish_blob(&path, bytes)?;
        }
        let artifact = Artifact {
            task: target.task.to_owned(),
            id: store.next_artifact_id(target.task)?,
            sha256: hash,
            bytes: bytes.len() as u64,
            kind: kind.map(str::to_owned),
            title: title.to_owned(),
            at: Timestamp::now(),
            by: target.actor,
        };
        store.insert_artifact(&artifact)?;
        store.append_event(&NewEvent {
            task: Some(artifact.task.clone()),
            run: target.run.map(str::to_owned),
            kind: EventKind::Artifact,
            actor: target.actor,
            detail: json!({"reference":artifact.reference(),"sha256":artifact.sha256,"bytes":artifact.bytes,"title":artifact.title,"kind":artifact.kind}),
        })?;
        Ok(artifact)
    }

    pub fn list_artifacts(&self, task: Option<&str>) -> Result<Vec<Artifact>> {
        self.store.list_artifacts(task)
    }

    /// Return the exact uncompressed bytes stored under a task-local reference.
    pub fn show_artifact(&self, reference: &str) -> Result<Vec<u8>> {
        let artifact = self.store.get_artifact(reference)?.ok_or_else(|| {
            refuse(
                format!("artifact {reference} does not exist; use corral artifact list to find it"),
                vec![],
            )
        })?;
        fs::read(self.blob_path(&artifact.sha256)).context("read artifact blob")
    }

    pub fn scan_artifacts(&self, worktree: &Path) -> Result<Vec<PathBuf>> {
        scan_globs(worktree, &self.config.artifacts.globs)
    }

    fn blob_path(&self, hash: &str) -> PathBuf {
        self.state_dir
            .join("blobs/sha256")
            .join(&hash[..2])
            .join(&hash[2..])
    }
}

/// Return sorted, deduplicated absolute file paths matching owner-configured globs.
/// Absolute patterns and parent traversal are rejected: scans stay in the worktree.
pub fn scan_globs(worktree: &Path, globs: &[String]) -> Result<Vec<PathBuf>> {
    let root = worktree
        .canonicalize()
        .context("resolve artifact scan worktree")?;
    let prefix = glob::Pattern::escape(root.to_str().context("artifact scan path is not UTF-8")?);
    let mut found = BTreeSet::new();
    for pattern in globs {
        if Path::new(pattern).is_absolute()
            || Path::new(pattern)
                .components()
                .any(|part| part == std::path::Component::ParentDir)
        {
            return Err(refuse(
                format!(
                    "artifact glob must stay inside the worktree: {pattern}; correct .corral-next/corral.toml"
                ),
                vec![],
            ));
        }
        for entry in glob::glob(&format!("{prefix}/{pattern}")).context("parse artifact glob")? {
            let path = entry.context("scan artifact glob")?;
            if path.is_file() {
                let target = path
                    .canonicalize()
                    .context("resolve matched artifact path")?;
                if target.starts_with(&root) {
                    found.insert(path);
                }
            }
        }
    }
    Ok(found.into_iter().collect())
}

fn parse_reference(reference: &str) -> Result<(&str, u32)> {
    let (task, id) = reference.rsplit_once(':').ok_or_else(|| {
        refuse(
            "artifact reference must be T-042:<n>; use corral artifact list",
            vec![],
        )
    })?;
    let id = id.parse().map_err(|_| {
        refuse(
            "artifact reference number is invalid; use corral artifact list",
            vec![],
        )
    })?;
    Ok((task, id))
}

fn size_refusal(task: &str, bytes: u64, max: u64) -> anyhow::Error {
    refuse(
        format!(
            "artifact is {bytes} bytes, above max_bytes {max} for {task}; use a smaller file or adjust .corral-next/corral.toml"
        ),
        vec![],
    )
}

fn publish_blob(path: &Path, bytes: &[u8]) -> Result<()> {
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let parent = path.parent().context("blob path has no parent")?;
    fs::create_dir_all(parent).context("create blob shard directory")?;
    let (tmp, mut file) = loop {
        let n = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let tmp = parent.join(format!(".incoming-{}-{n}", std::process::id()));
        match OpenOptions::new().write(true).create_new(true).open(&tmp) {
            Ok(file) => break (tmp, file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error).context("create temporary artifact blob"),
        }
    };
    let result = file
        .write_all(bytes)
        .context("write artifact blob")
        .and_then(|()| match fs::hard_link(&tmp, path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
            Err(error) => Err(error).context("publish artifact blob"),
        });
    let cleanup = fs::remove_file(&tmp).context("remove temporary artifact blob");
    result.and(cleanup)
}

#[cfg(test)]
#[path = "artifacts_tests.rs"]
mod tests;
