//! Validate and file a cross-project board graph as one operation, without dispatch.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    path::Path,
};

use anyhow::{Context, Result, ensure};
use corral_types::{
    TASK_SIZES,
    board_plan::{BoardPlan, BoardPlanFiledTask, BoardPlanParent, BoardPlanReceipt, BoardPlanTask},
    normalize_tags,
};
use jiff::Timestamp;
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{
    Config, Engine, Status, Store, Task, actor_from_env,
    events::{EventKind, NewEvent},
};

pub use corral_types::board_plan::MAX_PLAN_BYTES;

const MAX_TASKS: usize = 20;
const MAX_DEPS: usize = 32;
const MAX_BODY_BYTES: usize = 32 * 1024;
const MAX_WHY_BYTES: usize = 8 * 1024;
const MAX_TAGS_BYTES: usize = 8 * 1024;
const MAX_DEPS_BYTES: usize = 8 * 1024;
const MAX_PROJECT_BYTES: usize = 128;
const MAX_KEY_BYTES: usize = 64;
const MAX_TITLE_BYTES: usize = 240;
const MAX_LABEL_BYTES: usize = 512;

/// Only expected rejections prior to writing the plan use this error. In
/// particular, a caller must not infer rollback from an I/O or commit error.
#[derive(Debug, Serialize)]
pub struct BoardPlanRefusal {
    code: &'static str,
    refusal: String,
}

impl fmt::Display for BoardPlanRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.refusal)
    }
}

impl std::error::Error for BoardPlanRefusal {}

/// Every parent a board plan was filed under, read from the durable filing receipts. The
/// receipt, not having children, marks a summary: an ordinary task with children is not one.
/// Callers first check that the receipt table exists.
pub(crate) const FILED_PARENTS: &str = "SELECT DISTINCT json_extract(parent.value,'$.id')
    FROM board_plan_receipts, json_each(board_plan_receipts.receipt,'$.parents') AS parent";

impl Store {
    /// Whether `id` was filed as a board plan's parent: a summary of its children, not work.
    pub fn is_filed_plan_parent(&self, id: &str) -> Result<bool> {
        if !self.has_board_plan_receipts()? {
            return Ok(false);
        }
        self.connection
            .query_row(&format!("SELECT ?1 IN ({FILED_PARENTS})"), [id], |row| {
                row.get(0)
            })
            .context("look up filed plan parent")
    }

    /// Every filed plan parent that still exists, with its children's counts by status; a
    /// parent without children has none.
    pub fn filed_plan_children(&self) -> Result<BTreeMap<String, Vec<(Status, u64)>>> {
        let mut plans = BTreeMap::<String, Vec<(Status, u64)>>::new();
        if !self.has_board_plan_receipts()? {
            return Ok(plans);
        }
        let mut statement = self
            .connection
            .prepare(&format!(
                "WITH parents(id) AS ({FILED_PARENTS}),
                 children(parent,status,n) AS (
                    SELECT json_extract(document,'$.parent'),status,count(*) FROM tasks
                    WHERE json_extract(document,'$.parent') IN (SELECT id FROM parents)
                    GROUP BY 1,2)
                 SELECT parents.id,children.status,children.n FROM parents
                 JOIN tasks ON tasks.id=parents.id
                 LEFT JOIN children ON children.parent=parents.id"
            ))
            .context("prepare filed plan children")?;
        let mut rows = statement.query([]).context("query filed plan children")?;
        while let Some(row) = rows.next().context("read filed plan child count")? {
            let counts = plans.entry(row.get(0)?).or_default();
            if let Some(status) = row.get::<_, Option<String>>(1)? {
                let count: i64 = row.get(2)?;
                counts.push((
                    Status::parse(&status).context("parse stored task status")?,
                    count
                        .try_into()
                        .context("child count must not be negative")?,
                ));
            }
        }
        Ok(plans)
    }
}

fn invalid(reason: impl Into<String>) -> anyhow::Error {
    BoardPlanRefusal {
        code: "invalid_plan",
        refusal: reason.into(),
    }
    .into()
}

impl Engine {
    /// Validate on a coherent read-only snapshot. Filing revalidates on a writer
    /// snapshot unless the exact request already has a receipt.
    pub fn check_board_plan(root: &Path, payload: &[u8]) -> Result<()> {
        let state_dir = root.join(".corral-next");
        let config = Config::load(&state_dir.join("corral.toml"))?;
        let store = Store::open_read_only(&state_dir.join("corral.db"))?;
        store.read_snapshot(|| validate(&store, &config, payload).map(|_| ()))
    }

    /// The exact stdin bytes and start flag identify the request bound to `id`.
    /// The persisted receipt records the original result, not a live task view.
    pub fn file_board_plan(
        root: &Path,
        id: &str,
        start: bool,
        payload: &[u8],
    ) -> Result<BoardPlanReceipt> {
        let id = normalize_uuid(id)?;
        let state_dir = root.join(".corral-next");
        let db = state_dir.join("corral.db");
        ensure!(
            db.is_file(),
            "workspace is not initialized; run corral init"
        );
        let store = Store::open(&db)?;
        // Oversized stdin may be truncated by the caller. It cannot match any
        // receipt and must still go through the UUID lookup before rejection.
        let hash = (payload.len() <= MAX_PLAN_BYTES).then(|| {
            let mut digest = Sha256::new();
            digest.update([u8::from(start)]);
            digest.update(payload);
            format!("{:x}", digest.finalize())
        });
        store.transaction(|store| {
            if let Some((stored_hash, receipt)) = store.board_plan_receipt(&id)? {
                if hash.as_deref() != Some(stored_hash.as_str()) {
                    return Err(BoardPlanRefusal {
                        code: "plan_conflict",
                        refusal: "this board plan UUID is already bound to a different request"
                            .into(),
                    }
                    .into());
                }
                return serde_json::from_str(&receipt).context("decode stored board plan receipt");
            }

            // Replay must work before checking current config or board state:
            // projects, profiles, and dependencies may have changed since commit.
            let config = Config::load(&state_dir.join("corral.toml"))?;
            let plan = validate(store, &config, payload)?;
            let receipt = file(store, plan, start)?;
            store.record_board_plan_receipt(
                &id,
                hash.as_deref()
                    .context("validated plan has no request hash")?,
                &receipt,
            )?;
            Ok(receipt)
        })
    }
}

fn normalize_uuid(id: &str) -> Result<String> {
    if id.len() != 36
        || !id.bytes().enumerate().all(|(i, b)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
    {
        return Err(invalid("board plan --id must be a hyphenated UUID"));
    }
    Ok(id.to_ascii_lowercase())
}

fn validate(store: &Store, config: &Config, payload: &[u8]) -> Result<BoardPlan> {
    if payload.len() > MAX_PLAN_BYTES {
        return Err(invalid("board plan payload exceeds 128 KiB"));
    }
    let mut plan: BoardPlan = serde_json::from_slice(payload).map_err(|error| {
        let detail: String = error.to_string().chars().take(512).collect();
        invalid(format!("invalid board plan JSON: {detail}"))
    })?;
    header(&plan.title, MAX_TITLE_BYTES, "plan title")?;
    project(config, &plan.project)?;
    if plan.why.len() > MAX_WHY_BYTES {
        return Err(invalid("board plan explanation exceeds 8 KiB"));
    }
    if !(1..=MAX_TASKS).contains(&plan.tasks.len()) {
        return Err(invalid("board plan requires between 1 and 20 tasks"));
    }
    for task in &mut plan.tasks {
        validate_task(config, task)?;
    }
    if serde_json::to_vec(&plan.tasks.iter().map(|task| &task.tags).collect::<Vec<_>>())?.len()
        > MAX_TAGS_BYTES
    {
        return Err(invalid("board plan tags exceed the 8 KiB JSON budget"));
    }
    if serde_json::to_vec(
        &plan
            .tasks
            .iter()
            .map(|task| &task.depends)
            .collect::<Vec<_>>(),
    )?
    .len()
        > MAX_DEPS_BYTES
    {
        return Err(invalid(
            "board plan dependencies exceed the 8 KiB JSON budget",
        ));
    }
    let mut keys = BTreeMap::new();
    for task in &plan.tasks {
        if keys.insert(task.key.as_str(), task).is_some() {
            return Err(invalid(format!("duplicate board task key {:?}", task.key)));
        }
    }
    validate_graph(store, &keys)?;
    Ok(plan)
}

fn header(value: &str, limit: usize, label: &str) -> Result<()> {
    if value.len() > limit || value.trim().is_empty() || value.chars().any(char::is_control) {
        return Err(invalid(format!(
            "{label} must be nonblank, free of control characters, and at most {limit} bytes"
        )));
    }
    Ok(())
}

fn project(config: &Config, name: &str) -> Result<()> {
    header(name, MAX_PROJECT_BYTES, "project name")?;
    if !config.projects.contains_key(name) {
        return Err(invalid(format!("unknown project {name:?}")));
    }
    Ok(())
}

fn is_task_id(value: &str) -> bool {
    value.strip_prefix("T-").is_some_and(|suffix| {
        !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
    })
}

fn validate_task(config: &Config, task: &mut BoardPlanTask) -> Result<()> {
    header(&task.key, MAX_KEY_BYTES, "task key")?;
    if is_task_id(&task.key) {
        return Err(invalid(format!(
            "board task key {:?} looks like a task ID",
            task.key
        )));
    }
    header(&task.title, MAX_TITLE_BYTES, "task title")?;
    if let Some(name) = &task.project {
        project(config, name)?;
    }
    if let Some(kind) = &task.kind {
        header(kind, MAX_LABEL_BYTES, "task kind")?;
    }
    if let Some(profile) = &task.profile {
        header(profile, MAX_LABEL_BYTES, "worker profile")?;
        if !config.profiles.contains_key(profile) {
            return Err(invalid(format!("unknown worker profile {profile:?}")));
        }
    }
    if let Some(size) = task.size.as_deref()
        && !TASK_SIZES.contains(&size)
    {
        return Err(invalid(format!(
            "task {:?} has an invalid size; use xs, s, m, l, or xl",
            task.key
        )));
    }
    for tag in &task.tags {
        header(tag, MAX_LABEL_BYTES, "task tag")?;
    }
    task.tags = normalize_tags(std::mem::take(&mut task.tags)).map_err(invalid)?;
    if task.body.len() > MAX_BODY_BYTES {
        return Err(invalid(format!(
            "body of task {:?} exceeds 32 KiB",
            task.key
        )));
    }
    if task.depends.len() > MAX_DEPS {
        return Err(invalid(format!(
            "task {:?} exceeds 32 dependencies",
            task.key
        )));
    }
    let mut dependencies = BTreeSet::new();
    for dependency in &task.depends {
        header(dependency, MAX_KEY_BYTES, "dependency")?;
        if !dependencies.insert(dependency) {
            return Err(invalid(format!(
                "duplicate dependency {dependency:?} for task {:?}",
                task.key
            )));
        }
    }
    Ok(())
}

fn validate_graph(store: &Store, tasks: &BTreeMap<&str, &BoardPlanTask>) -> Result<()> {
    // A reference in an existing task can only refer to another existing task.
    // Reject any reachable dangling edge before allocating IDs: it otherwise
    // could start pointing to one of the new IDs and complete a hidden cycle.
    // Use an explicit stack because existing dependencies aren't depth-limited.
    enum Visit {
        Enter(String),
        Leave(String),
    }
    let mut stack: Vec<_> = tasks
        .keys()
        .map(|key| Visit::Enter((*key).to_owned()))
        .collect();
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    while let Some(visit) = stack.pop() {
        match visit {
            Visit::Enter(key) => {
                if visited.contains(&key) {
                    continue;
                }
                if !visiting.insert(key.clone()) {
                    return Err(invalid(format!(
                        "board dependencies contain a cycle at {key:?}"
                    )));
                }
                let dependencies = if let Some(task) = tasks.get(key.as_str()) {
                    task.depends.clone()
                } else if is_task_id(&key) {
                    let dependencies = store.task_dependencies(&key)?.ok_or_else(|| {
                        invalid(format!("board dependency task {key:?} does not exist"))
                    })?;
                    if dependencies.iter().any(|id| !is_task_id(id)) {
                        return Err(invalid(format!(
                            "existing task {key:?} has an invalid dependency ID"
                        )));
                    }
                    dependencies
                } else {
                    return Err(invalid(format!("unknown board dependency {key:?}")));
                };
                stack.push(Visit::Leave(key));
                stack.extend(dependencies.into_iter().rev().map(Visit::Enter));
            }
            Visit::Leave(key) => {
                visiting.remove(&key);
                visited.insert(key);
            }
        }
    }
    Ok(())
}

fn file(store: &Store, plan: BoardPlan, start: bool) -> Result<BoardPlanReceipt> {
    let now = Timestamp::now();
    let actor = actor_from_env();
    let mut parents = Vec::new();
    let mut parent_ids = BTreeMap::new();
    for task in &plan.tasks {
        let project = task.project.as_deref().unwrap_or(&plan.project);
        if !parent_ids.contains_key(project) {
            let id = store.next_task_id()?;
            parent_ids.insert(project.to_owned(), id.clone());
            parents.push(BoardPlanParent {
                project: project.to_owned(),
                id,
            });
        }
    }
    let mut ids = BTreeMap::new();
    for task in &plan.tasks {
        ids.insert(task.key.clone(), store.next_task_id()?);
    }
    let insert = |task: Task| -> Result<()> {
        store.insert_task(&task)?;
        store.append_event(&NewEvent {
            task: Some(task.id),
            run: None,
            kind: EventKind::Created,
            actor,
            detail: json!({"status":task.status,"title":task.title}),
        })?;
        Ok(())
    };
    for parent in &parents {
        insert(Task {
            id: parent.id.clone(),
            title: plan.title.clone(),
            body: plan.why.clone(),
            status: Status::Backlog,
            project: Some(parent.project.clone()),
            created: now,
            updated: now,
            ..Task::default()
        })?;
    }
    let status = if start {
        Status::Ready
    } else {
        Status::Backlog
    };
    let mut tasks = Vec::with_capacity(plan.tasks.len());
    for task in plan.tasks {
        let project = task.project.unwrap_or_else(|| plan.project.clone());
        let id = ids[&task.key].clone();
        let depends_on: Vec<_> = task
            .depends
            .into_iter()
            .map(|dependency| ids.get(&dependency).cloned().unwrap_or(dependency))
            .collect();
        insert(Task {
            id: id.clone(),
            title: task.title,
            body: task.body,
            status,
            project: Some(project.clone()),
            kind: task.kind,
            size: task.size,
            difficulty: task.difficulty,
            tags: task.tags,
            profile: task.profile,
            parent: Some(parent_ids[&project].clone()),
            depends_on: depends_on.clone(),
            created: now,
            updated: now,
            ..Task::default()
        })?;
        tasks.push(BoardPlanFiledTask {
            key: task.key,
            project,
            id,
            status,
            depends_on,
        });
    }
    Ok(BoardPlanReceipt { parents, tasks })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::TaskFilter;

    #[test]
    fn filing_failure_rolls_back_tasks_events_and_ids() -> Result<()> {
        let root = tempfile::tempdir()?;
        let engine = Engine::init(root.path(), Some(100))?;
        std::fs::write(
            engine.state_dir.join("corral.toml"),
            "[projects.example]\npath = '.'\n",
        )?;
        let store = &engine.store;
        store.connection.execute_batch(
            "CREATE TRIGGER fail_second_child BEFORE INSERT ON events
             WHEN json_extract(NEW.detail, '$.title') = 'Second child'
             BEGIN SELECT RAISE(ABORT, 'test filing failure'); END;",
        )?;
        let id = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
        let payload = br#"{
            "title":"Parent", "project":"example", "why":"Proposal rationale",
            "tasks":[
                {"key":"first", "title":"First child", "depends":[], "body":"Brief"},
                {"key":"second", "title":"Second child", "depends":["first"], "body":"Brief"}
            ]
        }"#;

        let error = Engine::file_board_plan(root.path(), id, true, payload).unwrap_err();
        assert!(format!("{error:#}").contains("test filing failure"));
        assert!(error.downcast_ref::<BoardPlanRefusal>().is_none());
        assert!(store.board_plan_receipt(id)?.is_none());
        assert!(store.list_tasks(&TaskFilter::default())?.is_empty());
        assert_eq!(store.latest_seq()?, 0);
        assert_eq!(store.next_task_number()?, 100);

        store
            .connection
            .execute_batch("DROP TRIGGER fail_second_child")?;
        let receipt = Engine::file_board_plan(root.path(), id, true, payload)?;
        assert_eq!(receipt.parents[0].id, "T-100");
        assert_eq!(receipt.tasks[1].id, "T-102");
        assert_eq!(receipt.tasks[1].depends_on, ["T-101"]);
        assert_eq!(
            receipt,
            Engine::file_board_plan(root.path(), id, true, payload)?
        );
        assert_eq!(store.latest_seq()?, 3);
        assert_eq!(store.next_task_number()?, 103);
        Ok(())
    }

    /// A filed parent sat in Backlog as runnable work: its receipt marks it as a summary
    /// whose child counts follow its children, and dispatch refuses it even when forced,
    /// while an ordinary task that happens to have a child is neither.
    #[test]
    fn filed_plan_parents_are_summaries_that_dispatch_refuses() -> Result<()> {
        let root = tempfile::tempdir()?;
        let engine = Engine::init(root.path(), Some(100))?;
        std::fs::write(
            engine.state_dir.join("corral.toml"),
            "[projects.example]\npath = '.'\n",
        )?;
        let store = &engine.store;
        assert!(store.filed_plan_children()?.is_empty());
        assert!(!store.is_filed_plan_parent("T-100")?);
        let payload = br#"{
            "title":"Parent", "project":"example", "why":"Proposal rationale",
            "tasks":[
                {"key":"first", "title":"First child", "depends":[], "body":"Brief"},
                {"key":"second", "title":"Second child", "depends":[], "body":"Brief"}
            ]
        }"#;
        Engine::file_board_plan(
            root.path(),
            "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
            false,
            payload,
        )?;
        let ordinary = Task {
            id: store.next_task_id()?,
            title: "Ordinary".into(),
            project: Some("example".into()),
            ..Task::default()
        };
        store.insert_task(&ordinary)?;
        store.insert_task(&Task {
            id: store.next_task_id()?,
            title: "Its child".into(),
            parent: Some(ordinary.id.clone()),
            ..Task::default()
        })?;
        let set = |id: &str, status: Status| -> Result<()> {
            let mut task = store.require_task(id)?;
            task.status = status;
            store.update_task(&task)
        };
        set("T-101", Status::Done)?;
        set("T-102", Status::Dropped)?;
        assert!(store.is_filed_plan_parent("T-100")?);
        assert!(!store.is_filed_plan_parent(&ordinary.id)?);
        assert_eq!(
            store.filed_plan_children()?,
            BTreeMap::from([(
                "T-100".to_owned(),
                vec![(Status::Done, 1), (Status::Dropped, 1)]
            )])
        );
        // Summaries are not open work in their project's counts.
        assert_eq!(
            store.open_project_status_counts()?["example"],
            [(Status::Backlog, 1)]
        );

        for force in [false, true] {
            let error = engine.dispatch("T-100", None, None, force).unwrap_err();
            let refusal = crate::error::as_refusal(&error).expect("dispatch refusal");
            assert!(
                refusal.reason.contains("filed board plan parent"),
                "{error:#}"
            );
        }
        assert_eq!(store.require_task("T-100")?.status, Status::Backlog);
        let error = engine.dispatch(&ordinary.id, None, None, true).unwrap_err();
        assert!(!format!("{error:#}").contains("filed board plan parent"));
        Ok(())
    }
}
