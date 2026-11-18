//! Read models backed by engine facts, not a second task or session store.
use std::{
    borrow::Cow,
    collections::BTreeMap,
    ops::Range,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use corral_engine::{
    Check, CheckKind, Engine, Role, Run, SessionStatus, Status, Task, store::TaskFilter,
    usage::UsageSummary,
};
use corral_protocol::{
    Health,
    rows::*,
    subscription::{BoardCursor, Topic},
};
use serde::Serialize;

use crate::state::Shared;

#[derive(PartialEq)]
pub(crate) enum Rows {
    Health(Vec<Health>),
    Tasks(Vec<TaskRow>),
    Plans(Vec<PlanRow>),
    Projects(Vec<ProjectRow>),
    Agents(Vec<AgentRow>),
    Usage(Vec<UsageSummary>),
    Settings(Vec<SettingsSnapshot>),
}

macro_rules! each {
    ($self:expr, $rows:ident, $body:expr) => {
        match $self {
            Rows::Health($rows) => $body,
            Rows::Tasks($rows) => $body,
            Rows::Plans($rows) => $body,
            Rows::Projects($rows) => $body,
            Rows::Agents($rows) => $body,
            Rows::Usage($rows) => $body,
            Rows::Settings($rows) => $body,
        }
    };
}

impl Rows {
    pub fn len(&self) -> usize {
        each!(self, rows, rows.len())
    }

    pub fn id(&self, index: usize) -> Cow<'_, str> {
        match self {
            Self::Health(_) => "health".into(),
            Self::Tasks(rows) => rows[index].task.id.as_str().into(),
            Self::Plans(rows) => rows[index].parent.id.as_str().into(),
            Self::Projects(rows) => rows[index].id.as_str().into(),
            Self::Agents(rows) => {
                let run = &rows[index].run;
                format!("{}_{}", run.task, run.id).into()
            }
            Self::Usage(_) => "usage".into(),
            Self::Settings(_) => "settings".into(),
        }
    }

    pub fn frame(
        &self,
        topic: Topic,
        epoch: u64,
        cursor: BoardCursor,
        range: Range<usize>,
        left: &[String],
        more: bool,
    ) -> Result<corral_protocol::frame::Frame> {
        #[derive(Serialize)]
        struct Page<'a, T> {
            topic: Topic,
            epoch: u64,
            rows: &'a [T],
            left: &'a [String],
            cursor: BoardCursor,
            more: bool,
        }
        each!(
            self,
            rows,
            crate::topics::notification(
                corral_protocol::SUB_DELTA,
                Page {
                    topic,
                    epoch,
                    rows: &rows[range],
                    left,
                    cursor,
                    more,
                }
            )
        )
    }
}

const EXTERNAL_POLL: Duration = Duration::from_secs(2);

/// One snapshot per topic shared by all clients. Each topic has its own lock,
/// so probing agents never holds the cache lock for health or SQLite-only reads.
#[derive(Default)]
pub(crate) struct RowCache {
    entries: [Mutex<Option<Observation>>; 7],
}

struct Observation {
    revision: u64,
    at: Instant,
    result: std::result::Result<Arc<Rows>, String>,
}

impl RowCache {
    pub(crate) fn get(&self, shared: &Shared, topic: Topic) -> Result<Arc<Rows>> {
        let index = match topic {
            Topic::Health => 0,
            Topic::Tasks => 1,
            Topic::Plans => 2,
            Topic::Projects => 3,
            Topic::Agents => 4,
            Topic::Usage => 5,
            Topic::Settings => 6,
            _ => bail!("unsupported row topic"),
        };
        let mut slot = self.entries[index]
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let revision = shared.revision();
        let refresh = slot.as_ref().is_none_or(|old| {
            let poll = matches!(topic, Topic::Agents | Topic::Projects | Topic::Settings)
                || old.result.is_err();
            old.revision != revision || (poll && old.at.elapsed() >= EXTERNAL_POLL)
        });
        if refresh {
            // Stamp the generation from before reading. A publish during the
            // read stays visible on the next access; never mark its data as read.
            let result = read(shared, topic)
                .map(|new| {
                    if let Some(Ok(old)) = slot.as_ref().map(|old| &old.result)
                        && **old == new
                    {
                        Arc::clone(old)
                    } else {
                        Arc::new(new)
                    }
                })
                .map_err(|error| format!("{error:#}"));
            *slot = Some(Observation {
                revision,
                at: Instant::now(),
                result,
            });
        }
        match &slot.as_ref().context("row observation unavailable")?.result {
            Ok(rows) => Ok(Arc::clone(rows)),
            Err(error) => Err(anyhow!("{error}")),
        }
    }
}

fn read(shared: &Shared, topic: Topic) -> Result<Rows> {
    if topic == Topic::Health {
        return Ok(Rows::Health(vec![shared.health()]));
    }
    let engine = shared.engine()?;
    match topic {
        Topic::Tasks => engine
            .store
            .read_snapshot(|| Ok(Rows::Tasks(tasks(&engine)?))),
        Topic::Plans => engine
            .store
            .read_snapshot(|| Ok(Rows::Plans(plans(&engine)?))),
        Topic::Projects => Ok(Rows::Projects(projects(&engine)?)),
        Topic::Agents => Ok(Rows::Agents(agents(&engine)?)),
        Topic::Usage => Ok(Rows::Usage(vec![engine.usage_summary(None)?])),
        Topic::Settings => Ok(Rows::Settings(vec![SettingsSnapshot {
            config: object(&engine.config)?,
            listener: shared.devices().listener(),
            hostname: crate::devices::reach::hostname(),
            devices: shared.devices().list()?,
        }])),
        _ => bail!("unsupported topic"),
    }
}

fn object(value: &impl Serialize) -> Result<serde_json::Map<String, serde_json::Value>> {
    match serde_json::to_value(value)? {
        serde_json::Value::Object(map) => Ok(map),
        _ => bail!("engine config is not an object"),
    }
}

fn tasks(engine: &Engine) -> Result<Vec<TaskRow>> {
    task_rows(
        engine,
        engine.store.list_tasks(&TaskFilter {
            statuses: vec![
                Status::Idea,
                Status::Backlog,
                Status::Ready,
                Status::Running,
                Status::Blocked,
                Status::Review,
            ],
            ..Default::default()
        })?,
    )
}

/// The caller reads the tasks and their related facts in one store snapshot.
pub(super) fn task_rows(engine: &Engine, tasks: Vec<Task>) -> Result<Vec<TaskRow>> {
    if tasks.is_empty() {
        return Ok(Vec::new());
    }
    let dependencies = tasks
        .iter()
        .flat_map(|task| task.depends_on.iter().map(String::as_str))
        .collect();
    let statuses = engine.store.task_statuses(&dependencies)?;
    let mut active = BTreeMap::<String, (Option<Run>, Option<Run>)>::new();
    for run in engine.store.live_runs(None)? {
        if run.session.is_some() {
            let (worker, reviewer) = active.entry(run.task.clone()).or_default();
            match run.role {
                Role::Worker => *worker = Some(run),
                Role::Reviewer => *reviewer = Some(run),
            }
        }
    }
    tasks
        .into_iter()
        .map(|task| {
            let (worker, reviewer) = active.remove(&task.id).unwrap_or_default();
            // Preserve the legacy preference for the worker when both are live.
            let run = worker.or_else(|| reviewer.clone());
            let (gate, review) = current_checks(engine, &task, run.as_ref())?;
            let ready =
                task.status == Status::Review && engine.store.has_ready_since_done(&task.id)?;
            let max_rounds = engine.config.task_max_rounds(&task);
            let dependencies = task
                .depends_on
                .iter()
                .map(|id| DependencyStatus {
                    status: statuses.get(id).copied(),
                    id: id.clone(),
                })
                .collect();
            Ok(TaskRow {
                task,
                run,
                gate,
                review,
                ready,
                max_rounds: Some(max_rounds),
                reviewer,
                dependencies: Some(dependencies),
            })
        })
        .collect()
}

pub(super) fn current_checks(
    engine: &Engine,
    task: &Task,
    live: Option<&Run>,
) -> Result<(Option<Check>, Option<Check>)> {
    let gate = if engine.store.has_gate_checks(&task.id)? {
        let last_worker;
        let worker = if let Some(run) = live.filter(|run| run.role == Role::Worker) {
            Some(run)
        } else {
            last_worker = engine.store.latest_run(&task.id, Role::Worker)?;
            last_worker.as_ref()
        };
        worker
            .map(|run| engine.observed_head(run))
            .transpose()?
            .flatten()
            .map(|head| engine.store.latest_check(&task.id, CheckKind::Gate, &head))
            .transpose()?
            .flatten()
    } else {
        None
    };
    let review = if engine.store.has_verdict_since_done(&task.id, task.round)? {
        engine
            .store
            .latest_check(&task.id, CheckKind::Review, &task.round.to_string())?
    } else {
        None
    };
    Ok((gate, review))
}

const STATUSES: [Status; 8] = [
    Status::Idea,
    Status::Backlog,
    Status::Ready,
    Status::Running,
    Status::Blocked,
    Status::Review,
    Status::Done,
    Status::Dropped,
];

fn plans(engine: &Engine) -> Result<Vec<PlanRow>> {
    let tasks = engine.store.list_tasks(&TaskFilter::default())?;
    let mut counts = BTreeMap::<&str, [u64; 8]>::new();
    for child in &tasks {
        if let Some(parent) = &child.parent {
            counts.entry(parent).or_default()[child.status as usize] += 1;
        }
    }
    let mut plans = Vec::with_capacity(counts.len());
    for parent in &tasks {
        if let Some(counts) = counts.get(parent.id.as_str()) {
            plans.push(PlanRow {
                parent: parent.clone(),
                child_counts: STATUSES
                    .into_iter()
                    .zip(*counts)
                    .filter(|(_, count)| *count != 0)
                    .map(|(status, count)| StatusCount { status, count })
                    .collect(),
            });
        }
    }
    Ok(plans)
}

fn projects(engine: &Engine) -> Result<Vec<ProjectRow>> {
    let mut counts = engine.store.open_project_status_counts()?;
    engine
        .config
        .projects
        .iter()
        .map(|(id, project)| {
            let by_status = counts.remove(id).unwrap_or_default();
            let open = by_status.iter().map(|(_, count)| count).sum();
            // An unavailable repository only loses its inferred base, not the rest
            // of the effective config or the project topic.
            let effective = engine
                .project_settings(id)
                .or_else(|_| engine.config.effective_project(id))?;
            Ok(ProjectRow {
                id: id.clone(),
                config: object(project)?,
                open,
                open_by_status: Some(
                    by_status
                        .into_iter()
                        .map(|(status, count)| StatusCount { status, count })
                        .collect(),
                ),
                effective: Some(EffectiveProjectSettings {
                    path: effective.path,
                    base: effective.base,
                    reviewer: effective.reviewer,
                    profile: effective.profile,
                    gate: effective.gate,
                    running: effective.running.map(|limit| limit as u64),
                    env: effective.env,
                    auto_dispatch: effective.auto_dispatch,
                    auto_land: effective.auto_land,
                    max_rounds: effective.max_rounds,
                    branch_prefix: effective.branch_prefix,
                }),
            })
        })
        .collect()
}

fn agents(engine: &Engine) -> Result<Vec<AgentRow>> {
    let mut agents = Vec::new();
    for run in engine.store.live_runs(None)? {
        if let Some(session) = &run.session {
            let status = engine
                .runner
                .status(session)
                .with_context(|| format!("observe session {} {}", run.task, run.id))?;
            if status != SessionStatus::Gone {
                agents.push(AgentRow { run, status });
            }
        }
    }
    Ok(agents)
}
