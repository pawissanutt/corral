use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    fs, io,
    path::Path,
};

use anyhow::{Context, Result, ensure};
use corral_engine::{
    Config, EventKind, Priority, Status, Store, Task, events::NewEvent, model::actor_from_env,
    refuse,
};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Deserialize)]
struct Envelope {
    schema_version: u32,
    task: LegacyTask,
}

#[derive(Deserialize)]
struct LegacyTask {
    id: String,
    title: String,
    status: LegacyStatus,
    project: Option<String>,
    #[serde(default)]
    priority: Priority,
    kind: Option<String>,
    size: Option<String>,
    parent: Option<String>,
    plan: Option<String>,
    #[serde(default)]
    children: Vec<String>,
    #[serde(default)]
    depends_on: Vec<String>,
    #[serde(default)]
    runs: Vec<LegacyRun>,
    created: Timestamp,
    updated: Timestamp,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum LegacyStatus {
    Idea,
    Backlog,
    Ready,
    Running,
    NeedsMe,
    Review,
    Done,
    Dropped,
}

#[derive(Deserialize)]
struct LegacyRun {
    branch: String,
    worktree: String,
}

#[derive(Serialize)]
pub(crate) struct Report {
    imported: BTreeMap<&'static str, usize>,
    skipped_closed: usize,
    skipped_existing: usize,
    unparsable: Vec<FileError>,
    unconfigured_projects: BTreeSet<String>,
    dependencies_already_done: usize,
    dependencies_needing_attention: BTreeMap<String, BTreeSet<String>>,
    next_id: u64,
}

#[derive(Serialize)]
struct FileError {
    file: String,
    reason: String,
}

const IMPORT_STATUSES: [Status; 5] = [
    Status::Idea,
    Status::Backlog,
    Status::Ready,
    Status::Done,
    Status::Dropped,
];

impl Report {
    pub(crate) fn write(&self, out: &mut impl io::Write) -> io::Result<()> {
        writeln!(out, "Import counts:")?;
        for status in IMPORT_STATUSES {
            writeln!(
                out,
                "  {}: {}",
                status.as_str(),
                self.imported[status.as_str()]
            )?;
        }
        writeln!(out, "Skipped as closed: {}", self.skipped_closed)?;
        writeln!(out, "Skipped as existing: {}", self.skipped_existing)?;
        writeln!(out, "Unparsable: {}", self.unparsable.len())?;
        for error in &self.unparsable {
            writeln!(out, "  {}: {}", error.file, error.reason)?;
        }
        writeln!(
            out,
            "Unconfigured projects: {}",
            self.unconfigured_projects.len()
        )?;
        for project in &self.unconfigured_projects {
            writeln!(out, "  {project}")?;
        }
        writeln!(
            out,
            "Dependencies already done: {}",
            self.dependencies_already_done
        )?;
        writeln!(
            out,
            "Dependencies needing owner attention: {}",
            self.dependencies_needing_attention.len()
        )?;
        for (id, tasks) in &self.dependencies_needing_attention {
            write!(out, "  {id} (needed by ")?;
            for (index, task) in tasks.iter().enumerate() {
                if index > 0 {
                    write!(out, ", ")?;
                }
                write!(out, "{task}")?;
            }
            writeln!(out, ")")?;
        }
        writeln!(out, "Next task ID: T-{:03}", self.next_id)
    }
}

pub(crate) fn run(root: &Path, dir: &Path, all: bool, dry_run: bool) -> Result<Report> {
    let state = root.join(".corral-next");
    let db = state.join("corral.db");
    if !db.is_file() {
        return Err(refuse(
            "workspace is not initialized; run corral init",
            vec![],
        ));
    }
    let config = Config::load(&state.join("corral.toml"))?;
    let export = Export::read(dir)?;
    if dry_run {
        let store = Store::open_read_only(&db)?;
        store.read_snapshot(|| summarize(&store, &config, export, all, false))
    } else {
        let store = Store::open(&db)?;
        store.transaction(|store| summarize(store, &config, export, all, true))
    }
}

struct Export {
    tasks: Vec<Task>,
    unparsable: Vec<FileError>,
    highest_id: Option<u32>,
}

impl Export {
    fn read(dir: &Path) -> Result<Self> {
        let mut paths = Vec::new();
        for entry in
            fs::read_dir(dir).with_context(|| format!("read export directory {}", dir.display()))?
        {
            let path = entry
                .with_context(|| format!("read export directory entry in {}", dir.display()))?
                .path();
            if path.extension().is_some_and(|extension| extension == "md") {
                paths.push(path);
            }
        }
        paths.sort_unstable();

        let mut parsed = Vec::new();
        let mut unparsable = Vec::new();
        let mut highest_id = None;
        for path in paths {
            let id = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_default();
            let number = id_number(id);
            highest_id = highest_id.max(number);
            let result = if number.is_some() {
                fs::read_to_string(&path)
                    .context("read task Markdown")
                    .and_then(|text| parse(&text, id))
            } else {
                Err(anyhow::anyhow!(
                    "filename must be a task ID followed by .md"
                ))
            };
            match result {
                Ok(task) => parsed.push(task),
                Err(error) => unparsable.push(FileError {
                    file: path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                    reason: format!("{error:#}"),
                }),
            }
        }

        // The member's own parent is authoritative; older exports may only list
        // children on the parent or have a versioned plan backlink on the member.
        let mut parents = BTreeMap::new();
        for (index, entry) in parsed.iter_mut().enumerate() {
            for child in entry
                .children
                .drain(..)
                .filter(|child| child != &entry.task.id)
            {
                parents
                    .entry(child)
                    .and_modify(|parent| {
                        if *parent != Some(index) {
                            *parent = None;
                        }
                    })
                    .or_insert(Some(index));
            }
        }
        for index in 0..parsed.len() {
            if parsed[index].task.parent.is_none() {
                let parent = parents
                    .get(&parsed[index].task.id)
                    .copied()
                    .flatten()
                    .map(|parent| parsed[parent].task.id.clone());
                parsed[index].task.parent = parent.or_else(|| parsed[index].plan_parent.take());
            }
        }
        let tasks = parsed.into_iter().map(|parsed| parsed.task).collect();
        Ok(Self {
            tasks,
            unparsable,
            highest_id,
        })
    }
}

struct ParsedTask {
    task: Task,
    children: Vec<String>,
    plan_parent: Option<String>,
}

fn id_number(id: &str) -> Option<u32> {
    let digits = id.strip_prefix("T-")?;
    if digits.len() < 3
        || (digits.len() > 3 && digits.starts_with('0'))
        || !digits.bytes().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    digits.parse().ok()
}

fn parse(text: &str, id: &str) -> Result<ParsedTask> {
    let mut lines = text.split_inclusive('\n');
    let first = lines.next().unwrap_or_default();
    ensure!(first.trim_end() == "---", "missing frontmatter start");
    let mut yaml_end = first.len();
    let mut body = None;
    for line in lines {
        if line.trim_end() == "---" {
            body = Some(&text[yaml_end + line.len()..]);
            break;
        }
        yaml_end += line.len();
    }
    let body = body.context("unterminated frontmatter")?;
    let Envelope {
        schema_version,
        task,
    } = serde_yaml_ng::from_str(&text[first.len()..yaml_end]).context("parse task frontmatter")?;
    ensure!(
        matches!(schema_version, 2 | 3),
        "unsupported task schema {schema_version}; expected 2 or 3"
    );
    ensure!(
        task.id == id,
        "frontmatter ID {} does not match filename ID {id}",
        task.id
    );
    let (status, old_status) = match task.status {
        LegacyStatus::Idea => (Status::Idea, None),
        LegacyStatus::Backlog => (Status::Backlog, None),
        LegacyStatus::Ready => (Status::Ready, None),
        LegacyStatus::Running => (Status::Backlog, Some("running")),
        LegacyStatus::NeedsMe => (Status::Backlog, Some("needs_me")),
        LegacyStatus::Review => (Status::Backlog, Some("review")),
        LegacyStatus::Done => (Status::Done, None),
        LegacyStatus::Dropped => (Status::Dropped, None),
    };
    let mut body = body.to_owned();
    if let Some(old_status) = old_status {
        if !body.is_empty() {
            if !body.ends_with('\n') {
                body.push('\n');
            }
            if !body.ends_with("\n\n") {
                body.push('\n');
            }
        }
        write!(
            body,
            "Imported from legacy status {old_status}; old runs were not adopted."
        )?;
        if let Some(run) = task.runs.last() {
            writeln!(
                body,
                " Last run: branch {:?}, worktree {:?}.",
                run.branch, run.worktree
            )?;
        } else {
            body.push_str(" No legacy run was recorded.\n");
        }
    }
    let plan_parent = task.plan.and_then(|mut plan| {
        let (parent, _) = plan.split_once('@')?;
        if parent == id || id_number(parent).is_none() {
            return None;
        }
        plan.truncate(parent.len());
        Some(plan)
    });
    Ok(ParsedTask {
        task: Task {
            id: task.id,
            title: task.title,
            body,
            status,
            project: task.project,
            priority: task.priority,
            kind: task.kind,
            size: task.size,
            parent: task.parent,
            depends_on: task.depends_on,
            created: task.created,
            updated: task.updated,
            ..Task::default()
        },
        children: task.children,
        plan_parent,
    })
}

fn summarize(
    store: &Store,
    config: &Config,
    export: Export,
    all: bool,
    apply: bool,
) -> Result<Report> {
    let ids = export
        .tasks
        .iter()
        .flat_map(|task| {
            std::iter::once(task.id.as_str()).chain(task.depends_on.iter().map(String::as_str))
        })
        .collect();
    let existing = store.task_statuses(&ids)?;
    let mut report = Report {
        imported: IMPORT_STATUSES
            .into_iter()
            .map(|status| (status.as_str(), 0))
            .collect(),
        skipped_closed: 0,
        skipped_existing: 0,
        unparsable: export.unparsable,
        unconfigured_projects: BTreeSet::new(),
        dependencies_already_done: 0,
        dependencies_needing_attention: BTreeMap::new(),
        next_id: store.next_task_number()?,
    };
    let mut selected = Vec::new();
    let mut excluded_closed = BTreeMap::new();
    for task in export.tasks {
        let closed = !all && matches!(task.status, Status::Done | Status::Dropped);
        if existing.contains_key(&task.id) {
            report.skipped_existing += 1;
        } else if closed {
            report.skipped_closed += 1;
        } else {
            selected.push(task);
            continue;
        }
        if closed {
            excluded_closed.insert(task.id, task.status);
        }
    }
    let importing: BTreeSet<_> = selected.iter().map(|task| task.id.as_str()).collect();
    for task in &selected {
        *report.imported.entry(task.status.as_str()).or_default() += 1;
        if let Some(project) = &task.project
            && !config.projects.contains_key(project)
        {
            report.unconfigured_projects.insert(project.clone());
        }
        for id in &task.depends_on {
            let closed = excluded_closed.get(id).copied();
            if closed == Some(Status::Done) {
                report.dependencies_already_done += 1;
                continue;
            }
            let stored = existing.get(id).copied();
            if (stored.is_none() && !importing.contains(id.as_str()))
                || (closed == Some(Status::Dropped) && stored != Some(Status::Done))
            {
                report
                    .dependencies_needing_attention
                    .entry(id.clone())
                    .or_default()
                    .insert(task.id.clone());
            }
        }
    }
    if apply {
        let actor = actor_from_env();
        for mut task in selected {
            if report.dependencies_already_done != 0 {
                task.depends_on
                    .retain(|id| excluded_closed.get(id) != Some(&Status::Done));
            }
            store
                .insert_task(&task)
                .with_context(|| format!("import task {}", task.id))?;
            store
                .append_event(&NewEvent {
                    task: Some(task.id.clone()),
                    run: None,
                    kind: EventKind::Created,
                    actor,
                    detail: json!({"status": task.status, "title": task.title, "imported": true}),
                })
                .with_context(|| format!("record import of {}", task.id))?;
        }
    }
    if let Some(highest) = export.highest_id {
        let next_id = u64::from(highest) + 1;
        if apply && next_id > report.next_id {
            store.reserve_task_ids_through(highest)?;
        }
        report.next_id = report.next_id.max(next_id);
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_statuses_and_notes_only_the_last_active_run() {
        for (legacy, mapped, active) in [
            ("idea", Status::Idea, false),
            ("backlog", Status::Backlog, false),
            ("ready", Status::Ready, false),
            ("running", Status::Backlog, true),
            ("needs_me", Status::Backlog, true),
            ("review", Status::Backlog, true),
            ("done", Status::Done, false),
            ("dropped", Status::Dropped, false),
        ] {
            let source = format!(
                "\
---
schema_version: 3
task:
  id: T-002
  title: Test
  status: {legacy}
  created: 2026-01-01T00:00:00Z
  updated: 2026-01-02T00:00:00Z
  runs:
  - branch: old/first
    worktree: /tmp/first
  - branch: old/last
    worktree: /tmp/last
recoveries: []
---
Body
---
Still the body"
            );
            let task = parse(&source, "T-002").unwrap().task;
            assert_eq!(task.status, mapped);
            if active {
                let note = task
                    .body
                    .strip_prefix("Body\n---\nStill the body\n\n")
                    .unwrap();
                assert_eq!(note.lines().count(), 1);
                assert!(note.contains(legacy), "{note}");
                assert!(
                    note.contains("old/last") && note.contains("/tmp/last"),
                    "{note}"
                );
                assert!(!note.contains("first"), "{note}");
            } else {
                assert_eq!(task.body, "Body\n---\nStill the body");
            }
        }
    }
}
