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
use corral_types::{GateAttempt, GateAttemptState, SystemRow};
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
    poll_task_attempts: bool,
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
        let previous = slot.as_ref();
        let old = previous.and_then(|old| old.result.as_ref().ok());
        let due = previous.is_some_and(|old| old.at.elapsed() >= EXTERNAL_POLL);
        let refresh = previous.is_none_or(|old| {
            let poll = matches!(
                topic,
                Topic::Agents | Topic::Projects | Topic::Settings | Topic::Assistant
            ) || old.result.is_err();
            old.revision != revision || (due && poll)
        });
        let update = if refresh {
            Some(read(shared, topic, builds).map(|new| match old {
                Some(old) if **old == new => Arc::clone(old),
                _ => Arc::new(new),
            }))
        } else if due
            && previous.is_some_and(|old| old.poll_task_attempts)
            && let Some(old) = old
            && let Rows::Tasks(rows) = old.as_ref()
        {
            Some(poll_task_attempts(shared, old, rows))
        } else {
            None
        };
        if let Some(result) = update {
            // Stamp the generation from before reading. A publish during the
            // read stays visible on the next access; never mark its data as read.
            *slot = Some(Observation {
                revision,
                at: Instant::now(),
                // No per-task scan on routine client flushes when no gate is active.
                poll_task_attempts: matches!(result.as_deref(), Ok(Rows::Tasks(rows)) if rows.iter().any(poll_attempt)),
                result: result.map_err(|error| format!("{error:#}")),
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
        Topic::Tasks => {
            let mut rows = engine.store.read_snapshot(|| tasks(&engine))?;
            observe_gate_attempts(&engine, &mut rows)?;
            Ok(Rows::Tasks(rows))
        }
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
    let (config, problem, extensions, automations, ask_chat_profile) =
        match Config::parse(&document.toml) {
            Ok(parsed) => (
                object(&parsed)?,
                None,
                extensions(shared, &parsed),
                automations(shared, &parsed),
                crate::assistant::configured_toml(&document.toml)
                    .unwrap_or(false)
                    .then(|| crate::assistant::default_profile(shared, &parsed).to_owned()),
            ),
            Err(problem) => (
                Default::default(),
                Some(problem),
                Vec::new(),
                Vec::new(),
                None,
            ),
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
        ask_chat_profile,
        extensions,
        automations,
    };
    if row.ask_chat_profile.is_some() && super::json_len(&row)? > super::REPLY_MAX {
        row.ask_chat_profile = None;
    }
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

// Judge runner liveness outside the board's SQLite snapshot so the engine can
// see a final result committed while it probes the active runner's lock.
pub(super) fn observe_gate_attempts(engine: &Engine, rows: &mut [TaskRow]) -> Result<()> {
    for row in rows {
        row.gate_attempt = board_gate_attempt(engine, &row.task.id)?;
    }
    Ok(())
}

fn live_attempt(attempt: &GateAttempt) -> bool {
    matches!(
        attempt.state,
        GateAttemptState::Running | GateAttemptState::Stale
    )
}

fn board_gate_attempt(engine: &Engine, task: &str) -> Result<Option<GateAttempt>> {
    let mut attempt = engine.latest_gate_attempt(task)?;
    if let Some(live) = attempt.as_mut().filter(|attempt| live_attempt(attempt)) {
        // Tasks are sent as complete replacement walks. Only step or lifecycle changes
        // belong on that topic; the selected Gate detail carries heartbeat and output times.
        live.heartbeat = None;
        live.last_output = None;
    }
    Ok(attempt)
}

fn poll_attempt(row: &TaskRow) -> bool {
    row.gate_attempt.as_ref().is_some_and(live_attempt)
}

fn poll_task_attempts(shared: &Shared, old: &Arc<Rows>, rows: &[TaskRow]) -> Result<Arc<Rows>> {
    let engine = shared.engine()?;
    let mut updated = None;
    for (index, row) in rows.iter().enumerate().filter(|(_, row)| poll_attempt(row)) {
        let attempt = board_gate_attempt(&engine, &row.task.id)?;
        if attempt != row.gate_attempt {
            updated.get_or_insert_with(|| rows.to_vec())[index].gate_attempt = attempt;
        }
    }
    Ok(updated.map_or_else(|| Arc::clone(old), |rows| Arc::new(Rows::Tasks(rows))))
}

/// Open tasks, and every filed plan parent whatever its stored status: a summary's shown
/// state follows its children, so one closed by hand still returns when a child reopens.
fn tasks(engine: &Engine) -> Result<Vec<TaskRow>> {
    let mut tasks = engine.store.list_tasks(&TaskFilter {
        statuses: vec![
            Status::Idea,
            Status::Backlog,
            Status::Ready,
            Status::Running,
            Status::Blocked,
            Status::Review,
        ],
        ..Default::default()
    })?;
    for id in engine.store.filed_plan_children()?.into_keys() {
        if let Some(parent) = engine.store.get_task(&id)?
            && matches!(parent.status, Status::Done | Status::Dropped)
        {
            tasks.push(parent);
        }
    }
    task_rows(engine, tasks)
}

/// The caller reads the tasks and their related facts in one store snapshot.
pub(super) fn task_rows(engine: &Engine, mut tasks: Vec<Task>) -> Result<Vec<TaskRow>> {
    if tasks.is_empty() {
        return Ok(Vec::new());
    }
    engine.store.add_observation_attention(&mut tasks)?;
    let dependencies = tasks
        .iter()
        .flat_map(|task| task.depends_on.iter().map(String::as_str))
        .collect();
    let statuses = engine.store.task_statuses(&dependencies)?;
    let mut plans = engine.store.filed_plan_children()?;
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
            let plan_children = plans.remove(&task.id).map(|counts| {
                counts
                    .into_iter()
                    .map(|(status, count)| StatusCount { status, count })
                    .collect()
            });
            Ok(TaskRow {
                plan_children,
                recovery_session,
                task,
                run,
                gate,
                gate_attempt: None,
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
        if let Some(worker) = worker
            && let Some(head) = engine.observed_head(worker)?
        {
            engine
                .store
                .latest_check(&task.id, CheckKind::Gate, &head)?
                .filter(|check| {
                    // A Running worker may reuse the same commit after a send-back or retry.
                    task.status != Status::Running
                        || (check.round.is_none_or(|round| round == task.round)
                            && check.run.as_deref().is_none_or(|run| run == worker.id))
                })
        } else {
            None
        }
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

// The project topic pages whole rows, so even its optional branch preview must leave room
// in a control frame. The branch detail endpoint supplies the remainder on demand.
const BRANCH_PREVIEW_BYTES: usize = 8 * 1024;

fn branch_preview(branches: Vec<String>, row_bytes: usize) -> Result<Vec<String>> {
    // Reserve framing and cursor metadata as well as the row's other fields. If the row alone
    // is already large, omit the optional preview rather than making the topic fail.
    let budget = corral_protocol::app::APP_CONTROL_MAX
        .saturating_sub(1024)
        .saturating_sub(row_bytes)
        .min(BRANCH_PREVIEW_BYTES);
    let mut used = 2usize; // JSON array brackets
    let mut preview = Vec::new();
    for branch in branches {
        let size = serde_json::to_vec(&branch)?.len().saturating_add(1);
        if used.saturating_add(size) > budget {
            break;
        }
        used += size;
        preview.push(branch);
    }
    Ok(preview)
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
            let mut row = ProjectRow {
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
                local_branches: None,
            };
            let row_bytes = serde_json::to_vec(&row)?.len();
            row.local_branches = local_branches
                .map(|branches| branch_preview(branches, row_bytes))
                .transpose()?
                .filter(|preview| !preview.is_empty());
            Ok(row)
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

#[cfg(test)]
mod branch_preview_tests {
    use super::*;

    #[test]
    fn long_branch_names_cannot_overflow_project_topic_rows() {
        let names = (0..100)
            .map(|n| format!("{n:03}/{}", "a".repeat(700)))
            .collect();
        let preview = branch_preview(names, 0).unwrap();
        assert!(preview.len() < 100);
        assert!(serde_json::to_vec(&preview).unwrap().len() <= BRANCH_PREVIEW_BYTES);
        let near_limit = corral_protocol::app::APP_CONTROL_MAX - 1100;
        assert!(
            branch_preview(vec!["x".repeat(200)], near_limit)
                .unwrap()
                .is_empty()
        );
    }
}
