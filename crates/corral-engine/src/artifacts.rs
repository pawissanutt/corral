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

/// Narrow the artifact finder before applying its page bounds.
#[derive(Debug, Default, Clone, Copy)]
pub struct ArtifactFilter<'a> {
    pub words: Option<&'a str>,
    pub kind: Option<&'a str>,
    pub task: Option<&'a str>,
    pub project: Option<&'a str>,
}

// Jiff serializes timestamps in UTC with an optional decimal fraction. With
// the trailing Z removed, SQLite's text order matches chronological order.
const CATALOG_SCHEMA: &str = "
    CREATE INDEX IF NOT EXISTS artifacts_by_time
        ON artifacts(rtrim(json_extract(document, '$.at'), 'Z'), task || ':' || ordinal);
    CREATE INDEX IF NOT EXISTS artifact_events_by_reference
        ON events(json_extract(detail, '$.reference')) WHERE kind='artifact';";

pub(crate) fn prepare_catalog(store: &Store) -> Result<()> {
    store
        .connection
        .execute_batch(CATALOG_SCHEMA)
        .context("prepare artifact catalog indexes")
}

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

    /// Newest first, with descending reference as a tie-breaker. The exclusive
    /// anchor remains usable even when newer or non-matching rows are added.
    pub fn artifacts_before(
        &self,
        filter: ArtifactFilter<'_>,
        before: Option<(Timestamp, &str)>,
        limit: usize,
    ) -> Result<Vec<Artifact>> {
        let time = before.map(|(at, _)| at.to_string());
        let mut statement = self
            .connection
            .prepare(
                "SELECT a.document, t.project, a.task || ':' || a.ordinal AS reference,
                        rtrim(json_extract(a.document, '$.at'), 'Z') AS sort_time
                   FROM artifacts a JOIN tasks t ON t.id=a.task
                  WHERE (sort_time, reference) < (coalesce(?1, '~'), coalesce(?2, ''))
                    AND (?3 IS NULL OR a.task=?3)
                  ORDER BY sort_time DESC, reference DESC",
            )
            .context("prepare artifact catalog page")?;
        let mut rows = statement
            .query(params![
                time.as_deref().map(|at| at.trim_end_matches('Z')),
                before.map(|(_, reference)| reference),
                filter.task,
            ])
            .context("query artifact catalog page")?;
        let kind_prefix = filter.kind.map(str::to_lowercase);
        let project_prefix = filter.project.map(str::to_lowercase);
        let words = filter
            .words
            .unwrap_or_default()
            .split_whitespace()
            .map(str::to_lowercase)
            .collect::<Vec<_>>();
        let mut selected = Vec::new();
        let mut run = self
            .connection
            .prepare(
                "SELECT run FROM events
                 WHERE kind='artifact' AND json_extract(detail, '$.reference')=?1
                 ORDER BY seq LIMIT 1",
            )
            .context("prepare artifact run attribution")?;
        while selected.len() < limit {
            let Some(row) = rows.next().context("read artifact catalog row")? else {
                break;
            };
            let project: Option<String> = row.get(1).context("read artifact project")?;
            if let Some(prefix) = &project_prefix
                && !project
                    .as_ref()
                    .is_some_and(|name| name.to_lowercase().starts_with(prefix))
            {
                continue;
            }
            let mut artifact: Artifact = serde_json::from_str(
                row.get_ref(0)
                    .context("read artifact metadata")?
                    .as_str()
                    .context("artifact metadata is not text")?,
            )
            .context("decode artifact metadata")?;
            let kind = if kind_prefix.is_some() || !words.is_empty() {
                artifact.kind.as_deref().map(str::to_lowercase)
            } else {
                None
            };
            if let Some(prefix) = &kind_prefix
                && !kind.as_ref().is_some_and(|name| name.starts_with(prefix))
            {
                continue;
            }
            let reference = row
                .get_ref(2)
                .context("read artifact reference")?
                .as_str()
                .context("artifact reference is not text")?;
            if !words.is_empty() {
                let title = artifact.title.to_lowercase();
                let reference = reference.to_ascii_lowercase();
                if !words.iter().all(|word| {
                    title.contains(word)
                        || kind.as_ref().is_some_and(|kind| kind.contains(word))
                        || reference.contains(word)
                }) {
                    continue;
                }
            }
            artifact.project = project;
            artifact.run = run
                .query_row([reference], |row| row.get(0))
                .optional()
                .context("read artifact run attribution")?
                .flatten();
            selected.push(artifact);
        }
        Ok(selected)
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
            let run = self.artifact_run(store, task)?;
            self.add_artifact_file_in(
                store,
                self.artifact_target(task, run.as_deref()),
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
            let run = self.artifact_run(store, task)?;
            self.add_artifact_in(
                store,
                self.artifact_target(task, run.as_deref()),
                bytes,
                kind,
                title,
            )
        })
    }

    fn artifact_run(&self, store: &Store, task: &str) -> Result<Option<String>> {
        // Run-launched CLI commands inherit this context. Owner submissions
        // (including daemon RPCs) are task-level attachments.
        if !matches!(self.actor, Actor::Agent | Actor::Reviewer)
            || std::env::var_os("CORRAL_NEXT_ROOT").as_deref() != Some(self.root.as_os_str())
            || std::env::var("CORRAL_TASK").as_deref() != Ok(task)
        {
            return Ok(None);
        }
        let Ok(run) = std::env::var("CORRAL_RUN") else {
            return Ok(None);
        };
        let exists: bool = store
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM runs WHERE task=?1 AND id=?2)",
                params![task, run],
                |row| row.get(0),
            )
            .context("look up artifact's submitting run")?;
        Ok(exists.then_some(run))
    }

    fn artifact_target<'a>(&self, task: &'a str, run: Option<&'a str>) -> ArtifactTarget<'a> {
        ArtifactTarget {
            task,
            run,
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
            project: None,
            run: None,
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

    /// Open a blob identified by catalog metadata. Useful for bounded paging;
    /// the caller never needs to construct or accept a filesystem path.
    pub fn open_artifact_blob(&self, artifact: &Artifact) -> Result<File> {
        anyhow::ensure!(
            artifact.sha256.len() == 64 && artifact.sha256.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid artifact hash"
        );
        File::open(self.blob_path(&artifact.sha256)).context("open artifact blob")
    }

    /// Select artifact candidates from the submitted change and ignored evidence scratch.
    pub(crate) fn scan_artifacts(
        &self,
        worktree: &Path,
        base: &str,
        head: &str,
    ) -> Result<Vec<PathBuf>> {
        let globs = &self.config.artifacts.globs;
        if globs.is_empty() {
            return Ok(Vec::new());
        }
        let changed = corral_git::changed_path_bytes_between(worktree, base, head)
            .context("list submitted paths for artifact collection")?;
        let scratch = corral_git::ignored_artifact_scratch(worktree)
            .context("list ignored evidence for artifact collection")?;
        // Text globs cannot match non-UTF-8 names; those paths must not block submission.
        let changed = changed
            .into_iter()
            .filter_map(|path| String::from_utf8(path).ok());
        scan_globs(worktree, globs, changed.chain(scratch))
    }

    fn blob_path(&self, hash: &str) -> PathBuf {
        self.state_dir
            .join("blobs/sha256")
            .join(&hash[..2])
            .join(&hash[2..])
    }
}

/// Filter repository-relative Git paths by the configured globs without walking the worktree.
fn scan_globs(
    worktree: &Path,
    globs: &[String],
    candidates: impl IntoIterator<Item = String>,
) -> Result<Vec<PathBuf>> {
    use std::path::Component;

    let mut patterns = Vec::new();
    for pattern in globs {
        let path = Path::new(pattern);
        if path.components().any(|part| {
            matches!(
                part,
                Component::RootDir | Component::Prefix(_) | Component::ParentDir
            )
        }) {
            return Err(refuse(
                format!(
                    "artifact glob must stay inside the worktree: {pattern}; correct .corral-next/corral.toml"
                ),
                vec![],
            ));
        }
        // A trailing separator requires a directory, which cannot be an artifact.
        if !pattern.ends_with(std::path::is_separator) {
            let normalized = path
                .components()
                .filter_map(|part| match part {
                    Component::Normal(value) => value.to_str(),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("/");
            patterns.push(glob::Pattern::new(&normalized).context("parse artifact glob")?);
        }
    }
    let options = glob::MatchOptions {
        require_literal_separator: true,
        ..glob::MatchOptions::new()
    };
    let root = worktree
        .canonicalize()
        .context("resolve artifact scan worktree")?;
    let mut found = BTreeSet::new();
    for candidate in candidates {
        if !patterns.iter().any(|p| p.matches_with(&candidate, options)) {
            continue;
        }
        let path = root.join(candidate);
        if path.is_file()
            && path
                .canonicalize()
                .context("resolve matched artifact path")?
                .starts_with(&root)
        {
            found.insert(path);
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
