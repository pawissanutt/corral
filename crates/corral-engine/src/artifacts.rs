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
    citations::{ArtifactSource, encode_artifact_record},
    config::reserved_project_name,
    error::refuse,
    events::{EventKind, NewEvent},
    model::Actor,
    store::Store,
};

/// Narrow the artifact finder before applying its page bounds.
#[derive(Debug, Default, Clone, Copy)]
pub struct ArtifactFilter<'a> {
    pub all: bool,
    pub words: Option<&'a str>,
    pub kind: Option<&'a str>,
    pub task: Option<&'a str>,
    pub project: Option<&'a str>,
}

// Jiff serializes timestamps in UTC with an optional decimal fraction. With
// the trailing Z removed, SQLite's text order matches chronological order.
const CATALOG_SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS project_artifacts(
        project TEXT NOT NULL, ordinal INTEGER NOT NULL, sha256 TEXT NOT NULL,
        document TEXT NOT NULL, PRIMARY KEY(project, ordinal));
    CREATE INDEX IF NOT EXISTS artifacts_by_time
        ON artifacts(rtrim(json_extract(document, '$.at'), 'Z'), task || ':' || ordinal);
    CREATE INDEX IF NOT EXISTS project_artifacts_by_time
        ON project_artifacts(rtrim(json_extract(document, '$.at'), 'Z'), project || ':' || ordinal);
    CREATE INDEX IF NOT EXISTS artifact_events_by_reference
        ON events(json_extract(detail, '$.reference')) WHERE kind='artifact';
    CREATE INDEX IF NOT EXISTS artifacts_by_replaces
        ON artifacts(json_extract(document, '$.replaces'));
    CREATE INDEX IF NOT EXISTS project_artifacts_by_replaces
        ON project_artifacts(json_extract(document, '$.replaces'));";

// Project the ordering and replacement expressions in each branch so SQLite
// can use the base-table indexes for the merged catalog reads.
const TASK_ARTIFACTS: &str = "SELECT a.task AS owner, a.ordinal, a.sha256, a.document, t.project,
        a.task || ':' || a.ordinal AS reference,
        rtrim(json_extract(a.document, '$.at'), 'Z') AS sort_time,
        json_extract(a.document, '$.replaces') AS replaces
    FROM main.artifacts a LEFT JOIN main.tasks t ON t.id=a.task";
const REPLACED_BY: &str = "SELECT reference FROM artifact_catalog WHERE replaces=?1 LIMIT 1";

pub(crate) fn prepare_catalog(store: &Store) -> Result<()> {
    store
        .connection
        .execute_batch(&format!(
            "{CATALOG_SCHEMA}
             CREATE VIEW IF NOT EXISTS artifact_catalog AS
             {TASK_ARTIFACTS}
             UNION ALL SELECT project, ordinal, sha256, document, project, project || ':' || ordinal,
                 rtrim(json_extract(document, '$.at'), 'Z'), json_extract(document, '$.replaces')
             FROM project_artifacts;"
        ))
        .context("prepare artifact catalog")
}

pub(crate) fn prepare_read_only_catalog(store: &Store) -> Result<()> {
    let exists: bool = store.connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.sqlite_schema WHERE type='view' AND name='artifact_catalog')",
        [],
        |row| row.get(0),
    ).context("check for artifact catalog view")?;
    if !exists {
        // An older store has only task artifacts. Give this connection the same
        // read interface without modifying the database or its schema version.
        store
            .connection
            .execute_batch(&format!(
                "CREATE TEMP VIEW artifact_catalog AS {TASK_ARTIFACTS};"
            ))
            .context("prepare legacy read-only artifact catalog")?;
    }
    Ok(())
}

impl Store {
    pub fn insert_artifact(&self, artifact: &Artifact) -> Result<()> {
        self.insert_owned_artifact(ArtifactOwner::Task(&artifact.task), artifact, None)
    }

    fn insert_owned_artifact(
        &self,
        owner: ArtifactOwner<'_>,
        artifact: &Artifact,
        source: Option<ArtifactSource>,
    ) -> Result<()> {
        let document = encode_artifact_record(artifact, source)?;
        let insert = match owner {
            ArtifactOwner::Task(_) => {
                "INSERT INTO artifacts(task,ordinal,sha256,document) VALUES(?1,?2,?3,?4)"
            }
            ArtifactOwner::Project(_) => {
                "INSERT INTO project_artifacts(project,ordinal,sha256,document) VALUES(?1,?2,?3,?4)"
            }
        };
        self.connection
            .execute(
                insert,
                params![owner.id(), artifact.id, artifact.sha256, document],
            )
            .context("insert artifact metadata")?;
        Ok(())
    }

    pub fn next_artifact_id(&self, owner: &str) -> Result<u32> {
        self.connection
            .query_row(
                "SELECT COALESCE((SELECT ordinal FROM artifact_catalog WHERE owner=?1
                                  ORDER BY ordinal DESC LIMIT 1),0)+1",
                [owner],
                |row| row.get(0),
            )
            .context("allocate artifact reference")
    }

    fn has_artifact_content(
        &self,
        task: &str,
        title: &str,
        content: &ArtifactContent<'_>,
    ) -> Result<bool> {
        // Legacy rows lack a format hint and keep deduplicating by filename/title.
        self.connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM artifact_catalog WHERE owner=?1 AND sha256=?2
                    AND json_extract(document, '$.title')=?3
                    AND coalesce(json_extract(document, '$.source_format'), ?4) IS ?4)",
                params![
                    task,
                    content.hash,
                    title,
                    content.source.map(ArtifactSource::as_str)
                ],
                |row| row.get(0),
            )
            .context("check for an unchanged scanned artifact")
    }

    pub fn list_artifacts(&self, owner: Option<&str>) -> Result<Vec<Artifact>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT document, owner, ordinal FROM artifact_catalog
                  WHERE (?1 IS NULL OR owner=?1) ORDER BY owner, ordinal",
            )
            .context("prepare artifact listing")?;
        let mut replacements = self.artifact_replacements()?;
        let mut rows = statement.query([owner]).context("query artifact listing")?;
        let mut artifacts = Vec::new();
        while let Some(row) = rows.next().context("read artifact listing")? {
            let mut artifact = decode_artifact(row)?;
            artifact.replaced_by = replaced_by(&mut replacements, &artifact.reference())?;
            artifacts.push(artifact);
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
            // Project sort_time so SQLite can merge the two ordered index scans.
            .prepare(
                "SELECT document, project, reference, sort_time FROM artifact_catalog
                  WHERE sort_time <= coalesce(?1, '~')
                    AND (sort_time, reference) < (coalesce(?1, '~'), coalesce(?2, ''))
                    AND (?3 IS NULL OR owner=?3)
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
        let mut replacements = self.artifact_replacements()?;
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
            let mut artifact = decode_artifact(row)?;
            let kind = artifact.kind.as_str();
            if !filter.all
                && filter.kind.is_none()
                && matches!(
                    artifact.kind.group(),
                    corral_types::ArtifactGroup::Submission | corral_types::ArtifactGroup::Record
                )
            {
                continue;
            }
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
                let reference = reference.to_lowercase();
                if !words.iter().all(|word| {
                    title.contains(word) || kind.contains(word) || reference.contains(word)
                }) {
                    continue;
                }
            }
            artifact.project = project;
            artifact.replaced_by = replaced_by(&mut replacements, reference)?;
            if !filter.all && filter.kind.is_none() && artifact.replaced_by.is_some() {
                continue;
            }
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
        let (owner, id) = parse_reference(reference)?;
        let mut statement = self
            .connection
            .prepare_cached("SELECT document FROM artifact_catalog WHERE owner=?1 AND ordinal=?2")
            .context("prepare artifact lookup")?;
        let mut rows = statement
            .query(params![owner, id])
            .context("query artifact reference")?;
        let Some(row) = rows.next().context("look up artifact reference")? else {
            return Ok(None);
        };
        let mut artifact = decode_artifact(row)?;
        let mut replacements = self.artifact_replacements()?;
        artifact.replaced_by = replaced_by(&mut replacements, &artifact.reference())?;
        Ok(Some(artifact))
    }

    fn artifact_replacements(&self) -> Result<rusqlite::CachedStatement<'_>> {
        self.connection
            .prepare_cached(REPLACED_BY)
            .context("prepare artifact replacement lookup")
    }

    // Called inside the same writer transaction that allocates and publishes the new row.
    fn validate_replacement(&self, owner: &str, reference: &str) -> Result<String> {
        let artifact = self
            .get_artifact(reference)?
            .ok_or_else(|| refuse(format!("artifact {reference} does not exist"), vec![]))?;
        if artifact.task != owner {
            return Err(refuse(
                format!(
                    "artifact {reference} belongs to {}, not {owner}",
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

fn decode_artifact(row: &rusqlite::Row<'_>) -> Result<Artifact> {
    serde_json::from_str(
        row.get_ref(0)
            .context("read artifact metadata")?
            .as_str()
            .context("artifact metadata is not text")?,
    )
    .context("decode artifact metadata")
}

fn replaced_by(lookup: &mut rusqlite::Statement<'_>, reference: &str) -> Result<Option<String>> {
    lookup
        .query_row([reference], |row| row.get(0))
        .optional()
        .context("read artifact replacement")
}

#[derive(Clone, Copy)]
pub(crate) enum ArtifactOwner<'a> {
    Task(&'a str),
    Project(&'a str),
}

impl<'a> ArtifactOwner<'a> {
    fn id(self) -> &'a str {
        match self {
            Self::Task(id) | Self::Project(id) => id,
        }
    }
}

pub(crate) struct ArtifactTarget<'a> {
    pub owner: ArtifactOwner<'a>,
    pub run: Option<&'a str>,
    pub actor: Actor,
    pub replaces: Option<&'a str>,
}

struct ArtifactContent<'a> {
    bytes: &'a [u8],
    hash: String,
    source: Option<ArtifactSource>,
}

impl<'a> ArtifactContent<'a> {
    fn new(bytes: &'a [u8], path: Option<&Path>) -> Self {
        Self {
            bytes,
            hash: format!("{:x}", Sha256::digest(bytes)),
            source: path.map(ArtifactSource::from_path),
        }
    }
}

impl Engine {
    pub fn add_artifact(
        &self,
        owner: &str,
        path: &Path,
        kind: ArtifactKind,
        title: Option<&str>,
    ) -> Result<Artifact> {
        self.add_artifact_with_replacement(owner, path, kind, title, None)
    }

    pub fn add_artifact_with_replacement(
        &self,
        owner: &str,
        path: &Path,
        kind: ArtifactKind,
        title: Option<&str>,
        replaces: Option<&str>,
    ) -> Result<Artifact> {
        self.store.transaction(|store| {
            let owner = self.require_artifact_owner(store, owner)?;
            let replaces = replaces
                .map(|reference| store.validate_replacement(owner.id(), reference))
                .transpose()?;
            let run = match owner {
                ArtifactOwner::Task(task) => self.artifact_run(store, task)?,
                ArtifactOwner::Project(_) => None,
            };
            self.add_artifact_file_in(
                store,
                ArtifactTarget {
                    owner,
                    run: run.as_deref(),
                    actor: self.actor,
                    replaces: replaces.as_deref(),
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

    pub(crate) fn artifact_run(&self, store: &Store, task: &str) -> Result<Option<String>> {
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

    pub(crate) fn artifact_target<'a>(
        &self,
        task: &'a str,
        run: Option<&'a str>,
    ) -> ArtifactTarget<'a> {
        ArtifactTarget {
            owner: ArtifactOwner::Task(task),
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
        let bytes = read_artifact_file(target.owner.id(), path, max)?;
        let filename = artifact_filename(path);
        self.publish_artifact_in(
            store,
            target,
            ArtifactContent::new(&bytes, Some(path)),
            kind,
            title.unwrap_or(&filename),
        )
    }

    pub(crate) fn add_scanned_artifact_in(
        &self,
        store: &Store,
        target: ArtifactTarget<'_>,
        path: &Path,
    ) -> Result<()> {
        let owner = target.owner.id();
        let bytes = read_artifact_file(owner, path, Some(self.config.artifacts.max_bytes))?;
        let title = artifact_filename(path);
        let content = ArtifactContent::new(&bytes, Some(path));
        if !store.has_artifact_content(owner, &title, &content)? {
            self.publish_artifact_in(store, target, content, ArtifactKind::Report, &title)?;
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
        self.publish_artifact_in(
            store,
            target,
            ArtifactContent::new(bytes, None),
            kind,
            title,
        )
    }

    fn publish_artifact_in(
        &self,
        store: &Store,
        target: ArtifactTarget<'_>,
        content: ArtifactContent<'_>,
        kind: ArtifactKind,
        title: &str,
    ) -> Result<Artifact> {
        let ArtifactContent {
            bytes,
            hash,
            source,
        } = content;
        if kind == ArtifactKind::Legacy {
            return Err(refuse("legacy is reserved for existing artifacts", vec![]));
        }
        let path = self.blob_path(&hash);
        if !path.try_exists().context("inspect artifact blob")? {
            publish_blob(&path, bytes)?;
        }
        let (task, project) = match target.owner {
            ArtifactOwner::Task(task) => (Some(task.to_owned()), None),
            ArtifactOwner::Project(project) => (None, Some(project.to_owned())),
        };
        let owner = target.owner.id();
        let artifact = Artifact {
            task: owner.to_owned(),
            id: store.next_artifact_id(owner)?,
            sha256: hash,
            bytes: bytes.len() as u64,
            kind,
            title: title.to_owned(),
            at: Timestamp::now(),
            by: target.actor,
            project,
            run: None,
            replaces: target.replaces.map(str::to_owned),
            replaced_by: None,
        };
        store.insert_owned_artifact(target.owner, &artifact, source)?;
        let mut detail = json!({"reference":artifact.reference(),"sha256":artifact.sha256,"bytes":artifact.bytes,"title":artifact.title,"kind":artifact.kind});
        if let Some(reference) = target.replaces {
            detail["replaces"] = json!(reference);
        }
        store.append_event(&NewEvent {
            task,
            run: target.run.map(str::to_owned),
            kind: EventKind::Artifact,
            actor: target.actor,
            detail,
        })?;
        Ok(artifact)
    }

    fn require_artifact_owner<'a>(
        &self,
        store: &Store,
        owner: &'a str,
    ) -> Result<ArtifactOwner<'a>> {
        if store.get_task(owner)?.is_some() {
            return Ok(ArtifactOwner::Task(owner));
        }
        if !owner.trim().is_empty()
            && !reserved_project_name(owner)
            && self.config.projects.contains_key(owner)
        {
            return Ok(ArtifactOwner::Project(owner));
        }
        Err(refuse(
            format!(
                "unknown artifact owner {owner:?}; use corral list --all for tasks or corral config for projects"
            ),
            vec![],
        ))
    }

    pub fn list_artifacts(&self, owner: Option<&str>) -> Result<Vec<Artifact>> {
        if let Some(owner) = owner {
            self.require_artifact_owner(&self.store, owner)?;
        }
        self.store.list_artifacts(owner)
    }

    pub fn require_artifact(&self, reference: &str) -> Result<Artifact> {
        if let Some(artifact) = self.store.get_artifact(reference)? {
            return Ok(artifact);
        }
        let (owner, _) = parse_reference(reference)?;
        self.require_artifact_owner(&self.store, owner)?;
        Err(refuse(
            format!("artifact {reference} does not exist; use corral artifact list to find it"),
            vec![],
        ))
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

/// Parse a task or project artifact reference without checking catalog membership.
pub fn parse_reference(reference: &str) -> Result<(&str, u32)> {
    let (owner, id) = reference
        .split_once(':')
        .filter(|(owner, _)| !owner.trim().is_empty())
        .ok_or_else(|| {
            refuse(
                "artifact reference must be <owner>:<n>; use corral artifact list",
                vec![],
            )
        })?;
    let id = id
        .bytes()
        .all(|byte| byte.is_ascii_digit())
        .then(|| id.parse().ok())
        .flatten()
        .filter(|&id| id > 0)
        .ok_or_else(|| {
            refuse(
                "artifact reference number is invalid; use corral artifact list",
                vec![],
            )
        })?;
    Ok((owner, id))
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
