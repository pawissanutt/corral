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
    Check, CheckKind, Config, ConfigProblem, Engine, Role, Run, SessionStatus, Status, Task,
    store::TaskFilter, usage::UsageSummary,
};
use corral_protocol::{
    Health,
    rows::*,
    subscription::{BoardCursor, Topic},
};
use corral_types::SystemRow;
use serde::Serialize;

use crate::state::Shared;

#[derive(PartialEq)]
pub(crate) enum Rows {
    Health(Vec<Health>),
    Tasks(Vec<TaskRow>),
    Plans(Vec<PlanRow>),
    Projects {
        rows: Vec<ProjectRow>,
        revision: Option<[u8; 32]>,
    },
    Agents(Vec<AgentRow>),
    Usage(Vec<UsageSummary>),
    Assistant(Vec<AssistantRow>),
    Settings(Vec<SettingsSnapshot>),
    System(Vec<SystemRow>),
    Host(Vec<corral_protocol::host::HostMetrics>),
}

macro_rules! each {
    ($self:expr, $rows:ident, $body:expr) => {
        match $self {
            Rows::Health($rows) => $body,
            Rows::Tasks($rows) => $body,
            Rows::Plans($rows) => $body,
            Rows::Projects { rows: $rows, .. } => $body,
            Rows::Agents($rows) => $body,
            Rows::Usage($rows) => $body,
            Rows::Assistant($rows) => $body,
            Rows::Settings($rows) => $body,
            Rows::System($rows) => $body,
            Rows::Host($rows) => $body,
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
            Self::Projects { rows, .. } => rows[index].id.as_str().into(),
            Self::Agents(rows) => {
                let run = &rows[index].run;
                format!("{}_{}", run.task, run.id).into()
            }
            Self::Usage(_) => "usage".into(),
            Self::Assistant(rows) => rows[index].id.as_str().into(),
            Self::Settings(_) => "settings".into(),
            Self::System(rows) => rows[index].project.as_str().into(),
            Self::Host(_) => "host".into(),
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
    entries: [Mutex<Option<Observation>>; 10],
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
            Topic::Assistant => 7,
            Topic::System => 9,
            _ => bail!("unsupported row topic"),
        };
        self.get_at(shared, topic, index, false)
    }

    pub(crate) fn assistant_with_builds(&self, shared: &Shared) -> Result<Arc<Rows>> {
        self.get_at(shared, Topic::Assistant, 8, true)
    }

    fn get_at(
        &self,
        shared: &Shared,
        topic: Topic,
        index: usize,
        builds: bool,
    ) -> Result<Arc<Rows>> {
        let mut slot = self.entries[index]
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let revision = shared.revision();
        let refresh = slot.as_ref().is_none_or(|old| {
            let poll = matches!(
                topic,
                Topic::Agents | Topic::Projects | Topic::Settings | Topic::Assistant
            ) || old.result.is_err();
            old.revision != revision || (poll && old.at.elapsed() >= EXTERNAL_POLL)
        });
        if refresh {
            // Stamp the generation from before reading. A publish during the
            // read stays visible on the next access; never mark its data as read.
            let result = read(shared, topic, builds)
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

fn read(shared: &Shared, topic: Topic, builds: bool) -> Result<Rows> {
    if topic == Topic::Health {
        return Ok(Rows::Health(vec![shared.health()]));
    }
    if topic == Topic::Assistant {
        return Ok(Rows::Assistant(crate::assistant::rows(shared, builds)?));
    }
    if topic == Topic::Settings {
        return settings(shared);
    }
    if topic == Topic::System {
        return Ok(Rows::System(shared.system.rows(shared)?));
    }
    if topic == Topic::Projects {
        let revision = shared.config_fingerprint();
        let rows = match shared.engine() {
            Ok(engine) => projects(&engine)?,
            Err(error) if error.downcast_ref::<ConfigProblem>().is_some() => Vec::new(),
            Err(error) => return Err(error),
        };
        return Ok(Rows::Projects { rows, revision });
    }
    let engine = shared.engine()?;
    match topic {
        Topic::Tasks => engine
            .store
            .read_snapshot(|| Ok(Rows::Tasks(tasks(&engine)?))),
        Topic::Plans => engine
            .store
            .read_snapshot(|| Ok(Rows::Plans(plans(&engine)?))),
        Topic::Agents => Ok(Rows::Agents(agents(&engine)?)),
        Topic::Usage => Ok(Rows::Usage(vec![engine.usage_summary(None)?])),
        _ => bail!("unsupported topic"),
    }
}

fn settings(shared: &Shared) -> Result<Rows> {
    let document = crate::config::get(shared)?;
    let (config, problem, extensions, automations) = match Config::parse(&document.toml) {
        Ok(parsed) => (
            object(&parsed)?,
            None,
            extensions(shared, &parsed),
            automations(shared, &parsed),
        ),
        Err(problem) => (Default::default(), Some(problem), Vec::new(), Vec::new()),
    };
    let built_in = Config::default();
    let (harness_facts, pi_config) = shared.harness_inventory.facts(document.revision.as_deref());
    let mut row = SettingsSnapshot {
        config,
        listener: shared.devices().listener(),
        hostname: crate::devices::reach::hostname(),
        devices: shared.devices().list()?,
        revision: document.revision,
        built_in_defaults: Some(object(&built_in)?),
        project_defaults: Some(object(&built_in.project_defaults())?),
        problem,
        harness_facts,
        pi_config,
        extensions,
        automations,
    };
    // Extra health data must not displace the editable settings or existing observations.
    if !row.automations.is_empty() && super::json_len(&row)? > super::REPLY_MAX {
        row.automations.clear();
    }
    // These optional facts must not make an otherwise serveable settings row too large.
    if (row.harness_facts.is_some() || row.pi_config.is_some())
        && super::json_len(&row)? > super::REPLY_MAX
    {
        row.harness_facts = None;
        row.pi_config = None;
    }
    Ok(Rows::Settings(vec![row]))
}

fn automations(shared: &Shared, config: &Config) -> Vec<AutomationHealth> {
    let state_dir = shared.root().join(crate::STATE_DIR);
    match crate::automation::health(&state_dir, config) {
        Ok(health) => health
            .into_iter()
            .map(|(rule, health)| AutomationHealth {
                rule,
                last_at: health.last_at,
                outcome: health.outcome,
                error: health.error,
                chat: health.chat,
            })
            .collect(),
        // A read failure must not expose paths or state-file contents, or fail Settings.
        Err(_) => config
            .automation
            .rules
            .keys()
            .map(|rule| AutomationHealth {
                rule: rule.clone(),
                last_at: None,
                outcome: None,
                error: Some("automation health unavailable".into()),
                chat: None,
            })
            .collect(),
    }
}

fn extensions(shared: &Shared, config: &Config) -> Vec<ExtensionHealth> {
    let state_dir = shared.root().join(crate::STATE_DIR);
    config
        .extensions
        .iter()
        .map(|(name, extension)| {
            let shapes = [
                (!extension.tools.is_empty(), "tool"),
                (extension.facts.is_some(), "ask"),
                (extension.run.is_some(), "react"),
            ];
            let (last_error, skipped) = match corral_engine::react::status(&state_dir, name) {
                Ok(cursor) => {
                    cursor.map_or((None, 0), |cursor| (cursor.last_error, cursor.skipped))
                }
                Err(error) => (Some(format!("{error:#}")), 0),
            };
            ExtensionHealth {
                name: name.clone(),
                shapes: shapes
                    .into_iter()
                    .filter(|(present, _)| *present)
                    .map(|(_, shape)| shape.to_owned())
                    .collect(),
                enabled: extension.enabled,
                last_error,
                skipped,
            }
        })
        .collect()
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
            let resolved_profile = engine.config.resolve_profile(&task, None).ok();
            let dependencies = task
                .depends_on
                .iter()
                .map(|id| DependencyStatus {
                    status: statuses.get(id).copied(),
                    id: id.clone(),
                })
                .collect();
            let recovery_session = engine
                .store
                .recovery_run(&task)?
                .map(|r| format!("{}_{}", r.task, r.id));
            Ok(TaskRow {
                recovery_session,
                task,
                run,
                gate,
                review,
                ready,
                max_rounds: Some(max_rounds),
                reviewer,
                dependencies: Some(dependencies),
                resolved_profile,
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
            let local_branches = engine.project_local_branches(&effective.path).ok();
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
                local_branches,
            })
        })
        .collect()
}

fn agents(engine: &Engine) -> Result<Vec<AgentRow>> {
    let mut agents = Vec::new();
    for run in engine.store.live_runs(None)? {
        if let Some(session) = &run.session {
            let (status, paused) = engine
                .session_runner()
                .observe(session)
                .with_context(|| format!("observe session {} {}", run.task, run.id))?;
            if status != SessionStatus::Gone {
                agents.push(AgentRow {
                    run,
                    status,
                    paused,
                });
            }
        }
    }
    Ok(agents)
}
