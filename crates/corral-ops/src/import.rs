//! Owner-authored Markdown plans become a tracker and ordinary child tasks.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, Result, ensure};
use corral_core::{
    Artifact, ArtifactKind, Origin, PlanDocument, PlanScope, Status, Store, Task, TaskKind,
    TaskSize,
};
use serde::{Deserialize, Serialize};

use crate::Ctx;

#[cfg(test)]
#[path = "import_tests.rs"]
mod tests;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    title: String,
    project: Option<String>,
    #[serde(default)]
    ready: bool,
    tasks: Vec<Entry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    key: String,
    title: String,
    project: Option<String>,
    kind: Option<TaskKind>,
    size: Option<TaskSize>,
    priority: Option<String>,
    profile: Option<String>,
    #[serde(default)]
    labels: Vec<String>,
    #[serde(default)]
    body: String,
    #[serde(default)]
    depends: Vec<String>,
}

/// Both preview and publication return the same planned cards and edges.
#[derive(Debug, Serialize)]
pub struct ImportResult {
    pub tracker: Task,
    pub tasks: Vec<Task>,
    pub dry_run: bool,
}

/// Validate the whole plan before publishing its cards and source snapshot.
///
/// # Errors
/// Returns an error for malformed input, invalid metadata/dependencies, an open
/// tracker with the same source digest, or failed atomic batch publication.
pub fn import_plan(ctx: &Ctx, path: &Path, dry_run: bool, origin: Origin) -> Result<ImportResult> {
    let bytes = crate::plan::read_plan(ctx, path)?;
    let text = std::str::from_utf8(&bytes).context("import plan must be UTF-8")?;
    let (document, body) = parse(text)?;
    let digest = crate::verification::digest(&bytes);
    let source = path
        .canonicalize()
        .with_context(|| format!("resolving import source {}", path.display()))?;
    let file_name = source
        .file_name()
        .context("import source has no file name")?
        .to_string_lossy();
    let count = document
        .tasks
        .len()
        .checked_add(1)
        .context("too many tasks")?;
    let mut cards = ctx.store.create_batch(count, dry_run, |store, cards| {
        ensure!(
            u64::try_from(bytes.len())? <= store.config.artifacts.max_bytes,
            "plan exceeds artifacts.max_bytes"
        );
        ensure!(
            !store.list()?.iter().any(|task| task.tracker
                && !matches!(task.status, Status::Done | Status::Dropped)
                && task.source_digest.as_deref() == Some(&digest)),
            "an open tracker already imports this source digest"
        );
        validate(store, &document)?;
        let (tracker, children) = cards.split_first_mut().context("missing tracker draft")?;
        let ids: BTreeMap<_, _> = document
            .tasks
            .iter()
            .zip(children.iter())
            .map(|(entry, task)| (entry.key.as_str(), task.id.clone()))
            .collect();
        tracker.title = document.title.clone();
        tracker.project = document.project.clone();
        tracker.origin = Some(origin);
        tracker.body = body.to_owned();
        tracker.tracker = true;
        tracker.source_digest = Some(digest.clone());
        tracker.children = children.iter().map(|task| task.id.clone()).collect();
        let snapshot = format!("artifacts/{}/R0/{}-{file_name}", tracker.id, &digest[..8]);
        tracker.artifacts.push(Artifact {
            run: "R0".into(),
            kind: ArtifactKind::Plan,
            title: document.title.clone(),
            path: snapshot.clone(),
            source: source.to_string_lossy().into_owned(),
            sha256: digest.clone(),
            bytes: u64::try_from(bytes.len())?,
            added: tracker.created,
            tier_override: None,
            compressed_bytes: None,
            purged_at: None,
        });
        for (entry, child) in document.tasks.iter().zip(children) {
            child.title = entry.title.clone();
            child.project = entry.project.clone().or_else(|| document.project.clone());
            child.kind = entry.kind.map(|kind| kind.as_str().to_owned());
            child.size = entry.size;
            if let Some(priority) = &entry.priority {
                child.priority = priority.clone();
            }
            child.profile = entry.profile.clone();
            child.labels = entry.labels.clone();
            child.origin = Some(origin);
            child.body = entry.body.clone();
            child.parent = Some(tracker.id.clone());
            child.status = if document.ready {
                Status::Ready
            } else {
                Status::Backlog
            };
            child.queued = document.ready;
            child.depends_on = entry
                .depends
                .iter()
                .map(|dependency| {
                    ids.get(dependency.as_str())
                        .cloned()
                        .map(Ok)
                        .unwrap_or_else(|| Store::normalize_id(dependency))
                })
                .collect::<Result<_>>()?;
        }
        Ok(vec![(snapshot.into(), bytes.clone())])
    })?;
    let tasks = cards.split_off(1);
    let tracker = cards.pop().context("missing imported tracker")?;
    Ok(ImportResult {
        tracker,
        tasks,
        dry_run,
    })
}

fn parse(text: &str) -> Result<(Document, &str)> {
    let mut lines = text.split_inclusive('\n');
    let first = lines.next().context("missing import frontmatter")?;
    ensure!(
        first.trim_end() == "---",
        "missing import frontmatter start"
    );
    let mut consumed = first.len();
    let mut yaml = String::new();
    for line in lines {
        consumed += line.len();
        if line.trim_end() == "---" {
            let document = serde_yaml_ng::from_str(&yaml).context("parsing import frontmatter")?;
            return Ok((document, &text[consumed..]));
        }
        yaml.push_str(line);
    }
    anyhow::bail!("unterminated import frontmatter")
}

fn validate(store: &Store, document: &Document) -> Result<()> {
    ensure!(
        !document.title.trim().is_empty(),
        "import title must not be blank"
    );
    ensure!(!document.tasks.is_empty(), "import tasks must not be empty");
    let mut keys = BTreeSet::new();
    for entry in &document.tasks {
        ensure!(!entry.key.trim().is_empty(), "task key must not be blank");
        ensure!(
            keys.insert(entry.key.as_str()),
            "duplicate task key {}",
            entry.key
        );
        ensure!(
            !entry.title.trim().is_empty(),
            "task {} title must not be blank",
            entry.key
        );
        if let Some(priority) = &entry.priority {
            ensure!(
                ["P0", "P1", "P2", "P3"].contains(&priority.as_str()),
                "invalid priority {priority:?} for {}",
                entry.key
            );
        }
    }
    for project in document.project.iter().chain(
        document
            .tasks
            .iter()
            .filter_map(|entry| entry.project.as_ref()),
    ) {
        ensure!(
            store.config.projects.contains_key(project),
            "unknown project {project:?}"
        );
    }
    let mut scopes = Vec::new();
    for entry in &document.tasks {
        let mut dependencies = BTreeSet::new();
        let mut local = Vec::new();
        for dependency in &entry.depends {
            let resolved = if keys.contains(dependency.as_str()) {
                local.push(dependency.clone());
                dependency.clone()
            } else {
                ensure!(
                    dependency.starts_with("T-"),
                    "unknown dependency key {dependency:?} for {}",
                    entry.key
                );
                store.get(dependency)?.id
            };
            ensure!(
                dependencies.insert(resolved),
                "duplicate dependency {dependency:?} for {}",
                entry.key
            );
        }
        scopes.push(PlanScope {
            task: entry.key.clone(),
            depends_on: local,
            write_scope: Vec::new(),
            acceptance: Vec::new(),
        });
    }
    crate::plan_decompose::validate(&PlanDocument {
        goal: document.title.clone(),
        dependencies: scopes,
        non_goals: Vec::new(),
        decisions: Vec::new(),
        affected_paths: Vec::new(),
        contracts: Vec::new(),
        acceptance: Vec::new(),
        verification: Vec::new(),
        owner_decisions: Vec::new(),
    })
}
