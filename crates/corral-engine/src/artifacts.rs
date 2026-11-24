use std::{
    borrow::Cow,
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
    ArtifactKind, Engine,
    error::refuse,
    events::{EventKind, NewEvent},
    model::Actor,
    store::{Store, encode},
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
        ON events(json_extract(detail, '$.reference')) WHERE kind='artifact';
    CREATE INDEX IF NOT EXISTS artifacts_by_replaces
        ON artifacts(json_extract(document, '$.replaces'));";

const REPLACED_BY: &str = "(SELECT newer.task || ':' || newer.ordinal FROM artifacts newer
    WHERE json_extract(newer.document, '$.replaces') = (a.task || ':' || a.ordinal))";

pub(crate) fn prepare_catalog(store: &Store) -> Result<()> {
    store
        .connection
        .execute_batch(CATALOG_SCHEMA)
        .context("prepare artifact catalog indexes")
}

impl Store {
    pub fn insert_artifact(&self, artifact: &Artifact) -> Result<()> {
        let document = if artifact.replaced_by.is_some() {
            encode(&Artifact {
                replaced_by: None,
                ..artifact.clone()
            })?
        } else {
            encode(artifact)?
        };
        self.connection
            .execute(
                "INSERT INTO artifacts(task,ordinal,sha256,document) VALUES(?1,?2,?3,?4)",
                params![artifact.task, artifact.id, artifact.sha256, document],
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

    fn has_artifact_content(&self, task: &str, title: &str, sha256: &str) -> Result<bool> {
        self.connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM artifacts WHERE task=?1 AND sha256=?2
                    AND json_extract(document, '$.title')=?3)",
                params![task, sha256, title],
                |row| row.get(0),
            )
            .context("check for an unchanged scanned artifact")
    }

    pub fn list_artifacts(&self, task: Option<&str>) -> Result<Vec<Artifact>> {
        let mut statement = self
            .connection
            .prepare(&format!(
                "SELECT a.document, {REPLACED_BY} FROM artifacts a
                  WHERE (?1 IS NULL OR a.task=?1) ORDER BY a.task, a.ordinal"
            ))
            .context("prepare artifact listing")?;
        let mut rows = statement.query([task]).context("query artifact listing")?;
        let mut artifacts = Vec::new();
        while let Some(row) = rows.next().context("read artifact listing")? {
            artifacts.push(decode_artifact(row, 1)?);
        }
        Ok(artifacts)
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
            .prepare(&format!(
                "SELECT a.document, t.project, a.task || ':' || a.ordinal AS reference,
                        rtrim(json_extract(a.document, '$.at'), 'Z') AS sort_time, {REPLACED_BY}
                   FROM artifacts a JOIN tasks t ON t.id=a.task
                  WHERE (sort_time, reference) < (coalesce(?1, '~'), coalesce(?2, ''))
                    AND (?3 IS NULL OR a.task=?3)
                  ORDER BY sort_time DESC, reference DESC",
            ))
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
            let mut artifact = decode_artifact(row, 4)?;
            let kind = artifact.kind.as_str();
            if let Some(prefix) = &kind_prefix
                && !kind.starts_with(prefix)
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
                    title.contains(word) || kind.contains(word) || reference.contains(word)
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
        let mut statement = self
            .connection
            .prepare_cached(&format!(
                "SELECT a.document, {REPLACED_BY} FROM artifacts a
                  WHERE a.task=?1 AND a.ordinal=?2"
            ))
            .context("prepare artifact lookup")?;
        let mut rows = statement
            .query(params![task, id])
            .context("query artifact reference")?;
        let Some(row) = rows.next().context("look up artifact reference")? else {
            return Ok(None);
        };
        Ok(Some(decode_artifact(row, 1)?))
    }

    // Called inside the same writer transaction that allocates and publishes the new row.
    fn validate_replacement(&self, task: &str, reference: &str) -> Result<String> {
        let artifact = self
            .get_artifact(reference)?
            .ok_or_else(|| refuse(format!("artifact {reference} does not exist"), vec![]))?;
        if artifact.task != task {
            return Err(refuse(
                format!(
                    "artifact {reference} belongs to {}, not {task}",
                    artifact.task
                ),
                vec![],
            ));
        }
        if let Some(mut newest) = artifact.replaced_by {
            while let Some(next) = self
                .get_artifact(&newest)?
                .with_context(|| format!("missing artifact replacement {newest}"))?
                .replaced_by
            {
                newest = next;
            }
            return Err(refuse(
                format!("artifact {reference} is already replaced; use --replaces {newest}"),
                vec![],
            ));
        }
        Ok(artifact.reference())
    }
}

fn decode_artifact(row: &rusqlite::Row<'_>, replacement_column: usize) -> Result<Artifact> {
    let mut artifact: Artifact = serde_json::from_str(
        row.get_ref(0)
            .context("read artifact metadata")?
            .as_str()
            .context("artifact metadata is not text")?,
    )
    .context("decode artifact metadata")?;
    artifact.replaced_by = row
        .get(replacement_column)
        .context("read artifact replacement")?;
    Ok(artifact)
}

pub(crate) struct ArtifactTarget<'a> {
    pub task: &'a str,
    pub run: Option<&'a str>,
    pub actor: Actor,
    pub replaces: Option<&'a str>,
}

impl Engine {
    pub fn add_artifact(
        &self,
        task: &str,
        path: &Path,
        kind: ArtifactKind,
        title: Option<&str>,
    ) -> Result<Artifact> {
        self.add_artifact_with_replacement(task, path, kind, title, None)
    }

    pub fn add_artifact_with_replacement(
        &self,
        task: &str,
        path: &Path,
        kind: ArtifactKind,
        title: Option<&str>,
        replaces: Option<&str>,
    ) -> Result<Artifact> {
        self.store.transaction(|store| {
            store.require_task(task)?;
            let replaces = replaces
                .map(|reference| store.validate_replacement(task, reference))
                .transpose()?;
            let run = self.artifact_run(store, task)?;
            self.add_artifact_file_in(
                store,
                ArtifactTarget {
                    replaces: replaces.as_deref(),
                    ..self.artifact_target(task, run.as_deref())
                },
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
        kind: ArtifactKind,
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

    /// Record at most one successful output per tool and run, atomically with its event.
    pub fn record_tool_output(
        &self,
        task: &str,
        run: &str,
        name: &str,
        bytes: &[u8],
    ) -> Result<()> {
        self.store.transaction(|store| {
            store.require_task(task)?;
            let exists: bool = store.connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM runs WHERE task=?1 AND id=?2)",
                params![task, run], |row| row.get(0),
            ).context("look up tool run")?;
            anyhow::ensure!(exists, "run {run} does not exist for {task}");
            let recorded: bool = store.connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM events WHERE task=?1 AND run=?2 AND kind='artifact' AND json_extract(detail, '$.tool_record')=?3)",
                params![task, run, name], |row| row.get(0),
            ).context("look up recorded tool output")?;
            if !recorded {
                let max = self.config.artifacts.max_bytes;
                if bytes.len() as u64 > max {
                    return Err(size_refusal(task, bytes.len() as u64, max));
                }
                let artifact = self.add_artifact_in(store, self.artifact_target(task, Some(run)), bytes, ArtifactKind::Tool, name)?;
                store.connection.execute(
                    "UPDATE events SET detail=json_set(detail, '$.tool_record', ?1) WHERE kind='artifact' AND json_extract(detail, '$.reference')=?2",
                    params![name, artifact.reference()],
                ).context("mark recorded tool output")?;
            }
            Ok(())
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
            replaces: None,
        }
    }

    pub(crate) fn add_artifact_file_in(
        &self,
        store: &Store,
        target: ArtifactTarget<'_>,
        path: &Path,
        kind: ArtifactKind,
        title: Option<&str>,
        max: Option<u64>,
    ) -> Result<Artifact> {
        let bytes = read_artifact_file(target.task, path, max)?;
        let filename = artifact_filename(path);
        self.add_artifact_in(store, target, &bytes, kind, title.unwrap_or(&filename))
    }

    pub(crate) fn add_scanned_artifact_in(
        &self,
        store: &Store,
        target: ArtifactTarget<'_>,
        path: &Path,
    ) -> Result<()> {
        let bytes = read_artifact_file(target.task, path, Some(self.config.artifacts.max_bytes))?;
        let title = artifact_filename(path);
        let hash = format!("{:x}", Sha256::digest(&bytes));
        if !store.has_artifact_content(target.task, &title, &hash)? {
            self.publish_artifact_in(store, target, &bytes, ArtifactKind::Report, &title, hash)?;
        }
        Ok(())
    }

    pub(crate) fn add_artifact_in(
        &self,
        store: &Store,
        target: ArtifactTarget<'_>,
        bytes: &[u8],
        kind: ArtifactKind,
        title: &str,
    ) -> Result<Artifact> {
        let hash = format!("{:x}", Sha256::digest(bytes));
        self.publish_artifact_in(store, target, bytes, kind, title, hash)
    }

    fn publish_artifact_in(
        &self,
        store: &Store,
        target: ArtifactTarget<'_>,
        bytes: &[u8],
        kind: ArtifactKind,
        title: &str,
        hash: String,
    ) -> Result<Artifact> {
        if kind == ArtifactKind::Legacy {
            return Err(refuse("legacy is reserved for existing artifacts", vec![]));
        }
        let path = self.blob_path(&hash);
        if !path.try_exists().context("inspect artifact blob")? {
            publish_blob(&path, bytes)?;
        }
        let artifact = Artifact {
            task: target.task.to_owned(),
            id: store.next_artifact_id(target.task)?,
            sha256: hash,
            bytes: bytes.len() as u64,
            kind,
            title: title.to_owned(),
            at: Timestamp::now(),
            by: target.actor,
            project: None,
            run: None,
            replaces: target.replaces.map(str::to_owned),
            replaced_by: None,
        };
        store.insert_artifact(&artifact)?;
        let mut detail = json!({"reference":artifact.reference(),"sha256":artifact.sha256,"bytes":artifact.bytes,"title":artifact.title,"kind":artifact.kind});
        if let Some(reference) = target.replaces {
            detail["replaces"] = json!(reference);
        }
        store.append_event(&NewEvent {
            task: Some(artifact.task.clone()),
            run: target.run.map(str::to_owned),
            kind: EventKind::Artifact,
            actor: target.actor,
            detail,
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

fn artifact_filename(path: &Path) -> Cow<'_, str> {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
}

fn read_artifact_file(task: &str, path: &Path, max: Option<u64>) -> Result<Vec<u8>> {
    let file = File::open(path).with_context(|| format!("open artifact {}", path.display()))?;
    let length = file.metadata().context("read artifact size")?.len();
    if let Some(max) = max
        && length > max
    {
        return Err(size_refusal(task, length, max));
    }
    let mut bytes = Vec::new();
    file.take(max.map_or(u64::MAX, |n| n.saturating_add(1)))
        .read_to_end(&mut bytes)
        .context("read artifact bytes")?;
    if let Some(max) = max
        && bytes.len() as u64 > max
    {
        return Err(size_refusal(task, bytes.len() as u64, max));
    }
    Ok(bytes)
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
