use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    fs, io,
    path::Path,
};

use anyhow::{Context, Result, ensure};
use corral_engine::{
    Actor, Config, EventKind, Priority, Status, Store, Task, TaskDifficulty, events::NewEvent,
    refuse,
};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

mod history;

use history::History;

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
    difficulty: Option<TaskDifficulty>,
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
    status_since: Option<Timestamp>,
    origin: Option<String>,
    landed: Option<LegacyLanded>,
    result: Option<LegacyResult>,
    #[serde(default)]
    comments: Vec<LegacyComment>,
    hold: Option<LegacyHold>,
    #[serde(default)]
    dispatch_hold: bool,
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

#[derive(Deserialize)]
struct LegacyLanded {
    sha: String,
}

#[derive(Deserialize)]
struct LegacyResult {
    text: Option<String>,
}

#[derive(Deserialize)]
struct LegacyComment {
    author: String,
    at: Timestamp,
    text: String,
}

#[derive(Deserialize)]
struct LegacyHold {
    reason: Option<String>,
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
    closing_dates: BTreeMap<&'static str, usize>,
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

/// Where a closed task's closing time came from, best record first.
const CLOSING_SOURCES: [&str; 3] = ["event_log", "status_time", "last_update"];

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
        writeln!(
            out,
            "Closing dates: {} from the event log, {} from the task's status time, {} from its last update",
            self.closing_dates["event_log"],
            self.closing_dates["status_time"],
            self.closing_dates["last_update"]
        )?;
        writeln!(out, "Next task ID: T-{:03}", self.next_id)
    }
}

pub(crate) fn run(
    root: &Path,
    dir: &Path,
    events: Option<&Path>,
    all: bool,
    dry_run: bool,
) -> Result<Report> {
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
    let history = events.map(History::read).transpose()?.unwrap_or_default();
    if dry_run {
        let store = Store::open_read_only(&db)?;
        store.read_snapshot(|| summarize(&store, &config, export, &history, all, false))
    } else {
        let store = Store::open(&db)?;
        store.transaction(|store| summarize(store, &config, export, &history, all, true))
    }
}

struct Export {
    tasks: Vec<Task>,
    legacy: BTreeMap<String, Legacy>,
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
        let mut tasks = Vec::with_capacity(parsed.len());
        let mut legacy = BTreeMap::new();
        for entry in parsed {
            legacy.insert(entry.task.id.clone(), entry.legacy);
            tasks.push(entry.task);
        }
        Ok(Self {
            tasks,
            legacy,
            unparsable,
            highest_id,
        })
    }
}

struct ParsedTask {
    task: Task,
    children: Vec<String>,
    plan_parent: Option<String>,
    legacy: Legacy,
}

/// What the export records that the new task model has no field for.
struct Legacy {
    /// When the task took its final status, on exports recent enough to say.
    since: Option<Timestamp>,
    /// The commit the task landed.
    sha: Option<String>,
    /// Who created the task.
    origin: Actor,
}

/// Ends a non-empty `body` with one blank line, ready for the next paragraph.
fn paragraph(body: &mut String) {
    if body.is_empty() {
        return;
    }
    if !body.ends_with('\n') {
        body.push('\n');
    }
    if !body.ends_with("\n\n") {
        body.push('\n');
    }
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
    // A held task must not look dispatchable: the old board would not have started it.
    let held = (task.dispatch_hold || task.hold.is_some())
        && matches!(
            task.status,
            LegacyStatus::Idea | LegacyStatus::Backlog | LegacyStatus::Ready
        );
    let (status, old_status) = match task.status {
        LegacyStatus::Idea => (Status::Idea, None),
        LegacyStatus::Ready if !held => (Status::Ready, None),
        LegacyStatus::Backlog | LegacyStatus::Ready => (Status::Backlog, None),
        LegacyStatus::Running => (Status::Backlog, Some("running")),
        LegacyStatus::NeedsMe => (Status::Backlog, Some("needs_me")),
        LegacyStatus::Review => (Status::Backlog, Some("review")),
        LegacyStatus::Done => (Status::Done, None),
        LegacyStatus::Dropped => (Status::Dropped, None),
    };
    // The new model keeps a task's discussion in its body, so the old result and comments go
    // there, under headings that say where they came from.
    let mut body = body.to_owned();
    if let Some(result) = task
        .result
        .and_then(|result| result.text)
        .filter(|text| !text.trim().is_empty())
    {
        paragraph(&mut body);
        writeln!(body, "## Result\n\n{}", result.trim_end())?;
    }
    if !task.comments.is_empty() {
        paragraph(&mut body);
        body.push_str("## Comments on the old board\n");
        for comment in &task.comments {
            writeln!(
                body,
                "\n**{} · {}**\n\n{}",
                comment.author,
                comment.at.strftime("%Y-%m-%d %H:%M UTC"),
                comment.text.trim_end()
            )?;
        }
    }
    if let Some(old_status) = old_status {
        paragraph(&mut body);
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
    if held {
        paragraph(&mut body);
        match task.hold.and_then(|hold| hold.reason) {
            Some(reason) => writeln!(body, "Held on the old board: {}", reason.trim_end())?,
            None => body.push_str("Held on the old board; no reason was recorded.\n"),
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
            difficulty: task.difficulty,
            parent: task.parent,
            depends_on: task.depends_on,
            created: task.created,
            updated: task.updated,
            ..Task::default()
        },
        children: task.children,
        plan_parent,
        legacy: Legacy {
            since: task.status_since,
            sha: task.landed.map(|landed| landed.sha),
            origin: history::actor(task.origin.as_deref()),
        },
    })
}

/// When and how the old board closed a task, from the best record there is of it.
struct Closing<'a> {
    at: Timestamp,
    source: &'static str,
    actor: Actor,
    from: Option<&'a str>,
    sha: Option<&'a str>,
    base: Option<&'a str>,
}

fn closing<'a>(
    task: &Task,
    legacy: Option<&'a Legacy>,
    history: &'a History,
) -> Option<Closing<'a>> {
    if !matches!(task.status, Status::Done | Status::Dropped) {
        return None;
    }
    let recorded = history.closure(&task.id, task.status);
    let (at, source) = match (recorded, legacy.and_then(|legacy| legacy.since)) {
        (Some(closure), _) => (closure.at, "event_log"),
        (None, Some(since)) => (since, "status_time"),
        // The old board touched closed tasks afterwards, so this can be days late.
        (None, None) => (task.updated, "last_update"),
    };
    Some(Closing {
        at: at.max(task.created),
        source,
        actor: recorded.map_or(Actor::Owner, |closure| closure.actor),
        from: recorded.and_then(|closure| closure.from.as_deref()),
        sha: recorded
            .and_then(|closure| closure.sha.as_deref())
            .or_else(|| legacy.and_then(|legacy| legacy.sha.as_deref())),
        base: recorded.and_then(|closure| closure.base.as_deref()),
    })
}

fn event(task: &Task, kind: EventKind, actor: Actor, mut detail: Value) -> NewEvent {
    detail["imported"] = json!(true);
    NewEvent {
        task: Some(task.id.clone()),
        run: None,
        kind,
        actor,
        detail,
    }
}

fn summarize(
    store: &Store,
    config: &Config,
    export: Export,
    history: &History,
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
        closing_dates: CLOSING_SOURCES
            .into_iter()
            .map(|source| (source, 0))
            .collect(),
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
        if let Some(closing) = closing(task, export.legacy.get(&task.id), history) {
            *report.closing_dates.entry(closing.source).or_default() += 1;
        }
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
        let mut events = Vec::new();
        for mut task in selected {
            if report.dependencies_already_done != 0 {
                task.depends_on
                    .retain(|id| excluded_closed.get(id) != Some(&Status::Done));
            }
            let legacy = export.legacy.get(&task.id);
            let closing = closing(&task, legacy, history);
            // A closed task was not created in its final status, and the export does not say
            // which one it was created in.
            let created = match closing {
                Some(_) => json!({"title": task.title}),
                None => json!({"status": task.status, "title": task.title}),
            };
            let origin = legacy.map_or(Actor::Owner, |legacy| legacy.origin);
            events.push((
                task.created,
                event(&task, EventKind::Created, origin, created),
            ));
            if let Some(closing) = closing {
                let (kind, detail) = match (task.status, closing.sha) {
                    (Status::Done, Some(sha)) => {
                        (EventKind::Landed, json!({"sha": sha, "base": closing.base}))
                    }
                    (Status::Done, None) => (
                        EventKind::Moved,
                        json!({"from": closing.from, "to": Status::Done}),
                    ),
                    _ => (EventKind::Dropped, json!({"from": closing.from})),
                };
                // As the engine does on closing a task; the list of closed tasks sorts on it.
                task.updated = closing.at;
                events.push((closing.at, event(&task, kind, closing.actor, detail)));
            }
            store
                .insert_task(&task)
                .with_context(|| format!("import task {}", task.id))?;
        }
        // Events keep the times they happened at, and go in oldest first: sequence order is
        // what the feed pages by. The sort is stable, so a task's creation precedes its closing.
        events.sort_by_key(|(at, _)| *at);
        for (at, event) in &events {
            store.append_event_at(event, *at).with_context(|| {
                format!(
                    "record import of {}",
                    event.task.as_deref().unwrap_or_default()
                )
            })?;
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
