//! The drain check (plan section 14): open work a migration must not move, read from the
//! legacy task files and state journals.

use std::fs;
use std::io::{self, BufRead};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde::Serialize;

use crate::{AuthorizedSubmission, PlanRunState, SubmissionDisposition, TaskRecord};

/// One piece of open work, or a history record reported next to the blockers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Blocker {
    pub family: &'static str,
    pub task: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pane: Option<String>,
    pub detail: String,
}

impl std::fmt::Display for Blocker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.family, self.task)?;
        if let Some(run) = &self.run {
            write!(f, " {run}")?;
        }
        if let Some(pane) = &self.pane {
            write!(f, " (pane {pane})")?;
        }
        write!(f, ": {}", self.detail)
    }
}

/// One parsed legacy task file.
#[derive(Debug, Clone)]
pub struct LegacyTask {
    pub file: PathBuf,
    pub record: TaskRecord,
}

impl std::ops::Deref for LegacyTask {
    type Target = TaskRecord;
    fn deref(&self) -> &TaskRecord {
        &self.record
    }
}

/// The legacy board as the migration reads it: every task file parsed, by id.
#[derive(Debug, Default)]
pub struct LegacyBoard {
    /// Each task with its declared schema (`flat`, `2` or `3`), ordered by task number.
    pub tasks: Vec<(LegacyTask, String)>,
    pub event_lines: u64,
    pub assistants: Vec<crate::assistant::AssistantSession>,
}

fn schema_of(text: &str) -> &'static str {
    let mut lines = text.lines().skip(1);
    let versioned = lines
        .by_ref()
        .take_while(|line| line.trim_end() != "---")
        .find_map(|line| line.strip_prefix("schema_version:"))
        .map(str::trim);
    match versioned {
        Some("3") => "3",
        Some(_) => "2",
        None => "flat",
    }
}

impl LegacyBoard {
    /// Parses every `tasks/*.md`; refuses an unparsable file or a duplicate task id.
    pub fn read(corral: &Path) -> Result<LegacyBoard> {
        let mut board = LegacyBoard::default();
        let dir = corral.join("tasks");
        let mut files = match fs::read_dir(&dir) {
            Ok(entries) => entries
                .map(|entry| entry.map(|e| e.path()))
                .collect::<io::Result<Vec<_>>>()
                .with_context(|| format!("reading {}", dir.display()))?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error).with_context(|| format!("reading {}", dir.display())),
        };
        files.retain(|path| path.extension().is_some_and(|ext| ext == "md"));
        files.sort();
        for file in files {
            let text =
                fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
            let mut record = crate::legacy::parse_task_record(&text, &file)?;
            super::workspace::relocate_task(corral, &mut record.task);
            let schema = schema_of(&text).to_owned();
            board.tasks.push((LegacyTask { file, record }, schema));
        }
        board
            .tasks
            .sort_by_key(|(task, _)| crate::store::id_number(&task.task.id));
        for pair in board.tasks.windows(2) {
            ensure!(
                pair[0].0.task.id != pair[1].0.task.id,
                "two legacy task files hold {}: {} and {}",
                pair[0].0.task.id,
                pair[0].0.file.display(),
                pair[1].0.file.display()
            );
        }
        board.assistants = super::workspace::assistants(corral)?;
        board.event_lines = count_lines(&corral.join("events.jsonl"))?;
        Ok(board)
    }

    /// The live blockers and the history records reported beside them.
    /// Live (non-purged) registered artifacts whose snapshot is not a regular file.
    pub fn missing_snapshots(&self, corral: &Path) -> Vec<String> {
        self.tasks
            .iter()
            .flat_map(|(t, _)| t.task.artifacts.iter().map(move |a| (t, a)))
            .map(|(t, a)| (t.task.id.as_str(), a))
            .chain(
                self.assistants
                    .iter()
                    .flat_map(|s| s.artifacts.iter().map(move |a| (s.id.as_str(), a))),
            )
            .filter(|(_, a)| a.purged_at.is_none() && !corral.join(&a.path).is_file())
            .map(|(t, a)| format!("{} {} artifact {:?} ({})", t, a.run, a.title, a.path))
            .collect()
    }

    pub fn blockers(
        &self,
        corral: &Path,
        probe: Option<&super::RuntimeProbe>,
    ) -> Result<(Vec<Blocker>, Vec<Blocker>)> {
        let mut live = Vec::new();
        let mut history = Vec::new();
        for (task, _) in &self.tasks {
            let start = live.len();
            task_blockers(task, &mut live, &mut history);
            let absent = if live.len() > start
                && matches!(
                    task.task.status,
                    crate::Status::Done | crate::Status::Dropped
                )
                && let Some(probe) = probe
            {
                match probe(&task.task) {
                    Ok(absent) => absent,
                    Err(error) => {
                        for blocker in &mut live[start..] {
                            blocker
                                .detail
                                .push_str(&format!("; runtime absence unconfirmed: {error:#}"));
                        }
                        false
                    }
                }
            } else {
                false
            };
            if absent {
                let pending = live.split_off(start);
                for mut blocker in pending {
                    if matches!(
                        blocker.family,
                        "open_coding_run"
                            | "open_review_run"
                            | "open_plan_run"
                            | "pending_plan_review"
                            | "pending_aggregate_review"
                            | "live_gate"
                            | "review_wait"
                    ) {
                        blocker.detail.push_str("; terminal task, panes and processes absent; lost history imported unchanged");
                        history.push(blocker);
                    } else {
                        live.push(blocker);
                    }
                }
            }
        }
        self.state_blockers(corral, &mut live)?;
        Ok((live, history))
    }

    fn task(&self, id: &str) -> Option<&LegacyTask> {
        self.tasks
            .iter()
            .map(|(task, _)| task)
            .find(|task| task.task.id == id)
    }

    fn state_blockers(&self, corral: &Path, live: &mut Vec<Blocker>) -> Result<()> {
        let state = corral.join("state"); // path-guard: allow (migration reads the legacy tree)
        let launch = state.join("review-launch.json");
        if launch.is_file() {
            let pending = serde_json::from_slice::<serde_json::Value>(&fs::read(&launch)?)
                .with_context(|| format!("parsing {}", launch.display()))?;
            let field = |name: &str| {
                pending
                    .get(name)
                    .and_then(|v| v.as_str())
                    .map(str::to_owned)
            };
            live.push(Blocker {
                family: "review_launch_journal",
                task: field("task").unwrap_or_default(),
                run: field("run"),
                pane: field("pane"),
                detail: "an auxiliary review or gate launch is in progress".into(),
            });
        }
        for (path, bytes) in json_files(&state.join("action-authorizations"))? {
            let record: AuthorizedSubmission = serde_json::from_slice(&bytes)
                .with_context(|| format!("parsing {}", path.display()))?;
            if record.disposition == SubmissionDisposition::Submitting {
                live.push(Blocker {
                    family: "submitting_authorization",
                    task: record.authorization.action.task.clone(),
                    run: None,
                    pane: None,
                    detail: format!("authorization {} is submitting", record.authorization.id),
                });
            }
        }
        for (path, bytes) in json_files(&state.join("review-prepared"))? {
            let value: serde_json::Value = serde_json::from_slice(&bytes)
                .with_context(|| format!("parsing {}", path.display()))?;
            let field = |name: &str| value.get(name).and_then(|v| v.as_str()).unwrap_or("");
            let (task, run) = (field("task"), field("run"));
            let open = self.task(task).is_none_or(|t| {
                t.task
                    .runs
                    .iter()
                    .find(|r| r.id == run)
                    .is_none_or(|r| r.ended.is_none())
            });
            if open {
                live.push(Blocker {
                    family: "prepared_review_journal",
                    task: task.to_owned(),
                    run: Some(run.to_owned()),
                    pane: None,
                    detail: format!("{} names a run that has not ended", path.display()),
                });
            }
        }
        for (path, bytes) in json_files(&state)? {
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some(stem) = name
                .strip_prefix("plan-review-")
                .and_then(|n| n.strip_suffix(".json"))
            else {
                continue;
            };
            let value: serde_json::Value = serde_json::from_slice(&bytes)
                .with_context(|| format!("parsing {}", path.display()))?;
            let plan = value.get("plan").and_then(|v| v.as_str()).unwrap_or("");
            let task = stem.strip_suffix(&format!("-{plan}")).unwrap_or(stem);
            let review = value.get("review");
            let review_id = review.and_then(|r| r.get("id")).and_then(|v| v.as_str());
            let terminal = review
                .and_then(|r| r.get("verdict"))
                .is_some_and(|v| !v.is_null())
                || self.task(task).is_some_and(|t| {
                    t.task.plans.iter().find(|p| p.id == plan).is_none_or(|p| {
                        p.approval.is_some()
                            || p.reviews
                                .iter()
                                .any(|r| Some(r.id.as_str()) == review_id && r.verdict.is_some())
                            || t.task.plans.last().is_some_and(|last| last.id != p.id)
                    }) || matches!(t.task.status, crate::Status::Done | crate::Status::Dropped)
                });
            if !terminal {
                live.push(Blocker {
                    family: "prepared_plan_review_journal",
                    task: task.to_owned(),
                    run: Some(plan.to_owned()),
                    pane: None,
                    detail: format!(
                        "{} holds plan review {} without a verdict",
                        path.display(),
                        review_id.unwrap_or("?")
                    ),
                });
            }
        }
        Ok(())
    }
}

fn pane(herdr: Option<&crate::HerdrRef>) -> Option<String> {
    herdr.map(|h| h.pane_id.clone())
}

fn task_blockers(task: &LegacyTask, live: &mut Vec<Blocker>, history: &mut Vec<Blocker>) {
    let t = &task.task;
    let push = |list: &mut Vec<Blocker>, family, run: Option<&str>, pane, detail: String| {
        list.push(Blocker {
            family,
            task: t.id.clone(),
            run: run.map(str::to_owned),
            pane,
            detail,
        });
    };
    for run in &t.runs {
        let run_pane = pane(run.herdr.as_ref());
        if run.ended.is_none() {
            push(
                live,
                "open_coding_run",
                Some(&run.id),
                run_pane.clone(),
                "the coding run has not ended".into(),
            );
        }
        for child in &run.review_runs {
            if child.ended.is_none() {
                push(
                    live,
                    "open_review_run",
                    Some(&child.id),
                    pane(child.herdr.as_ref()),
                    format!("review run of {} has not ended", run.id),
                );
            }
        }
        if let Some(gate) = &run.gate {
            let detail = format!("gate at {} on {} is running", gate.at, gate.sha);
            if gate.holds_slot() {
                push(live, "live_gate", Some(&run.id), gate.pane.clone(), detail);
            } else if gate.result == crate::GateResult::Running {
                push(
                    history,
                    "process_lost_gate",
                    Some(&run.id),
                    gate.pane.clone(),
                    format!("{detail}; process lost, imported unchanged"),
                );
            }
        }
    }
    for run in &t.plan_runs {
        if run.ended.is_none() && run.state != PlanRunState::Approved {
            push(
                live,
                "open_plan_run",
                Some(&run.id),
                pane(run.herdr.as_ref()),
                format!("plan run is {:?}", run.state),
            );
        }
    }
    for plan in &t.plans {
        for review in plan
            .reviews
            .iter()
            .filter(|r| r.verdict.is_none() && !r.cancelled)
        {
            push(
                live,
                "pending_plan_review",
                Some(&review.id),
                pane(review.herdr.as_ref()),
                format!("review of plan {} has no verdict", plan.id),
            );
        }
    }
    for attempt in &t.aggregate_attempts {
        for review in attempt.reviews.iter() {
            if review.verdict.is_none() && review.abandoned.is_none() {
                push(
                    live,
                    "pending_aggregate_review",
                    Some(&review.id),
                    pane(review.herdr.as_ref()),
                    format!("aggregate review of {} has no verdict", attempt.id),
                );
            }
        }
        if let Some(gate) = attempt.gate.as_ref().filter(|g| g.holds_slot()) {
            push(
                live,
                "live_gate",
                Some(&attempt.id),
                gate.pane.clone(),
                format!("aggregate gate at {} is running", gate.at),
            );
        }
    }
    if let Some(launch) = &t.launch {
        push(
            live,
            "launch_reservation",
            None,
            None,
            format!("launch reserved at stage {:?}", launch.stage),
        );
    }
    if let Some(work) = &t.review_reservation {
        push(
            live,
            "review_reservation",
            None,
            None,
            format!("{work:?} reservation"),
        );
    }
    if let Some(work) = &t.review_wait {
        push(live, "review_wait", None, None, format!("{work:?} wait"));
    }
}

fn json_files(dir: &Path) -> Result<Vec<(PathBuf, Vec<u8>)>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).with_context(|| format!("reading {}", dir.display())),
    };
    let mut out = Vec::new();
    for entry in entries {
        let path = entry
            .with_context(|| format!("reading {}", dir.display()))?
            .path();
        if path.extension().is_some_and(|ext| ext == "json") && path.is_file() {
            let bytes = fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
            out.push((path, bytes));
        }
    }
    out.sort();
    Ok(out)
}

pub(crate) fn count_lines(path: &Path) -> Result<u64> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error).with_context(|| format!("opening {}", path.display())),
    };
    let mut reader = io::BufReader::new(file);
    let mut count = 0;
    let mut line = Vec::new();
    loop {
        line.clear();
        let n = reader
            .read_until(b'\n', &mut line)
            .with_context(|| format!("reading {}", path.display()))?;
        if n == 0 {
            return Ok(count);
        }
        count += 1;
    }
}
