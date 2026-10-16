//! Display-only stage monitoring. No value in this module is verification evidence.
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use corral_core::{GateRecord, GateResult, Run, Status, Task};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::Ctx;

#[cfg(test)]
#[path = "watch_tests.rs"]
mod tests;

#[path = "watch_review.rs"]
mod review;
pub use review::ReviewerAttempt;
pub(crate) use review::{capture_review, start_review};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressState {
    Pending,
    Running,
    Passed,
    Failed,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandProgress {
    pub command: String,
    pub state: ProgressState,
    pub started: Option<Timestamp>,
    pub ended: Option<Timestamp>,
    #[serde(default, deserialize_with = "duration_ms")]
    pub duration_ms: Option<u64>,
    pub exit_code: Option<i32>,
    pub log_start: u64,
    pub log_end: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateAttempt {
    pub run: String,
    pub attempt: String,
    pub round: u32,
    pub started: Timestamp,
    pub ended: Option<Timestamp>,
    #[serde(default, deserialize_with = "duration_ms")]
    pub duration_ms: Option<u64>,
    pub state: ProgressState,
    pub commands: Vec<CommandProgress>,
    /// Absolute path of the attempt-specific live log; never reused by another attempt.
    pub log: PathBuf,
    pub live_pane: Option<String>,
    /// Authoritative receipt, independent of the display-only state above.
    pub record: GateRecord,
}

#[derive(Debug, Clone)]
pub struct StageHistory {
    pub gates: Vec<GateAttempt>,
    pub reviews: Vec<ReviewerAttempt>,
}

// Older sidecars subtracted wall-clock readings and could persist negative values.
fn duration_ms<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<u64>, D::Error> {
    Option::<i128>::deserialize(deserializer)?
        .map(|ms| u64::try_from(ms.max(0)).map_err(serde::de::Error::custom))
        .transpose()
}

pub(crate) fn elapsed(start: Timestamp, end: Timestamp) -> u64 {
    u64::try_from(end.duration_since(start).as_millis()).unwrap_or(0)
}

fn metadata_path(ctx: &Ctx, task: &str, run: &str, attempt: &str) -> PathBuf {
    ctx.store
        .state_dir()
        .join("watch")
        .join(task)
        .join(format!("{run}-{attempt}.json"))
}

pub(crate) fn start_gate(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    record: &GateRecord,
    commands: &[String],
    log: &Path,
) -> Result<()> {
    let Some(identity) = &record.identity else {
        return Ok(());
    };
    let attempt = GateAttempt {
        run: run.id.clone(),
        attempt: identity.attempt.clone(),
        round: task.rounds,
        started: record.at,
        ended: None,
        duration_ms: None,
        state: ProgressState::Pending,
        commands: commands
            .iter()
            .map(|command| CommandProgress {
                command: command.clone(),
                state: ProgressState::Pending,
                started: None,
                ended: None,
                duration_ms: None,
                exit_code: None,
                log_start: 0,
                log_end: 0,
            })
            .collect(),
        log: log.to_path_buf(),
        live_pane: record.pane.clone(),
        record: record.clone(),
    };
    save_gate(ctx, &task.id, &attempt)
}

fn save_gate(ctx: &Ctx, task: &str, attempt: &GateAttempt) -> Result<()> {
    crate::artifact_storage::replace(
        &metadata_path(ctx, task, &attempt.run, &attempt.attempt),
        &serde_json::to_vec(attempt).context("encoding gate monitor")?,
    )
}

#[derive(Deserialize)]
struct Runner {
    pid: u32,
    process: String,
    boot: String,
    state: ProgressState,
    started: Timestamp,
    ended: Option<Timestamp>,
}

fn read_optional<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .with_context(|| format!("parsing {}", path.display()))
            .map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

fn runner_alive(runner: &Runner) -> bool {
    let process = fs::read_to_string(format!("/proc/{}/stat", runner.pid));
    let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id");
    match (process, boot) {
        (Ok(process), Ok(boot)) => {
            // comm can contain spaces and parentheses: fields after the final ')' are stable.
            let fields = process.rsplit_once(')').map(|(_, fields)| fields);
            boot.trim() == runner.boot
                && fields.and_then(|s| s.split_whitespace().nth(19))
                    == Some(runner.process.as_str())
                && fields.and_then(|s| s.split_whitespace().next()) != Some("Z")
        }
        _ => false,
    }
}

fn observe(ctx: &Ctx, task: &Task, attempt: &mut GateAttempt) -> Result<()> {
    let current = task
        .current_run()
        .filter(|r| r.id == attempt.run && !matches!(task.status, Status::Done | Status::Dropped));
    let gate = current.and_then(|r| r.gate.as_ref()).filter(|g| {
        g.identity
            .as_ref()
            .is_some_and(|i| i.attempt == attempt.attempt)
    });
    if let Some(gate) = gate {
        attempt.record = gate.clone();
    }
    let runner: Option<Runner> = read_optional(&attempt.log.with_extension("runner.json"))?;
    if let Some(runner) = &runner {
        attempt.started = runner.started;
        if attempt.state != ProgressState::Interrupted || runner.ended.is_some() {
            attempt.state = runner.state;
            attempt.ended = runner.ended;
        }
        if runner.state == ProgressState::Running && !runner_alive(runner) {
            attempt.state = ProgressState::Interrupted;
            attempt.ended.get_or_insert_with(Timestamp::now);
        }
    }
    match attempt.record.result {
        GateResult::Pass => attempt.state = ProgressState::Passed,
        GateResult::Fail => attempt.state = ProgressState::Failed,
        GateResult::Running => {
            if attempt.record.process_lost || gate.is_none() {
                if matches!(
                    attempt.state,
                    ProgressState::Pending | ProgressState::Running
                ) {
                    attempt.state = ProgressState::Interrupted;
                    attempt.ended.get_or_insert_with(Timestamp::now);
                }
            } else if runner.is_none() && attempt.state == ProgressState::Pending {
                // The launcher can die before the runner exists. A live pane is not proof
                // forever: launch reservation recovery marks process_lost on sync.
                if current.is_some_and(|r| r.ended.is_some()) {
                    attempt.state = ProgressState::Interrupted;
                    attempt.ended.get_or_insert_with(Timestamp::now);
                }
            }
        }
    }
    if matches!(attempt.state, ProgressState::Passed | ProgressState::Failed) {
        attempt.ended.get_or_insert(attempt.record.at);
    }
    let bytes = fs::metadata(&attempt.log).map_or(0, |m| m.len());
    for (index, command) in attempt.commands.iter_mut().enumerate() {
        if let Some(progress) =
            read_optional(&attempt.log.with_extension(format!("command-{index}.json")))?
        {
            *command = progress;
        }
        if command.state == ProgressState::Running {
            command.log_end = bytes;
            if attempt.state == ProgressState::Interrupted {
                command.state = ProgressState::Interrupted;
                command.ended = attempt.ended;
            }
        }
        if command.state == ProgressState::Pending {
            command.log_start = bytes;
            command.log_end = bytes;
        }
        if command.duration_ms.is_none() || command.state == ProgressState::Running {
            command.duration_ms = command
                .started
                .map(|start| elapsed(start, command.ended.unwrap_or_else(Timestamp::now)));
        }
    }
    attempt.duration_ms = Some(elapsed(
        attempt.started,
        attempt.ended.unwrap_or_else(Timestamp::now),
    ));
    attempt.live_pane = gate
        .filter(|_| attempt.state == ProgressState::Running)
        .and_then(|g| g.pane.clone());
    save_gate(ctx, &task.id, attempt)
}

/// Loads oldest-first history and live progress. Reconciles monitor sidecars only;
/// callers still use the normal gate/review collectors for verification.
pub fn load(ctx: &Ctx, task: &str) -> Result<StageHistory> {
    let task = ctx.store.get(task)?;
    let owned = crate::session_scope::owns_inventory(ctx)?;
    let dir = ctx.store.state_dir().join("watch").join(&task.id);
    let mut gates = Vec::new();
    match fs::read_dir(&dir) {
        Ok(entries) => {
            for entry in entries {
                let path = entry.context("reading gate monitor directory")?.path();
                if path.extension().is_none_or(|e| e != "json") {
                    continue;
                }
                let mut attempt: GateAttempt =
                    read_optional(&path)?.context("gate monitor disappeared during load")?;
                if owned {
                    observe(ctx, &task, &mut attempt)?;
                } else {
                    attempt.live_pane = None;
                }
                gates.push(attempt);
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).context("reading gate monitor history"),
    }
    gates.sort_by(|a, b| a.started.cmp(&b.started).then(a.attempt.cmp(&b.attempt)));
    Ok(StageHistory {
        gates,
        reviews: review::load(ctx, &task, owned)?,
    })
}

/// Reads a bounded byte window, including output written while the command is running.
pub fn read_gate_log(
    ctx: &Ctx,
    attempt: &GateAttempt,
    start: u64,
    max_bytes: usize,
) -> Result<Vec<u8>> {
    if attempt.log.is_file() {
        let mut file = fs::File::open(&attempt.log).context("opening gate log")?;
        file.seek(SeekFrom::Start(start))
            .context("seeking gate log")?;
        let mut bytes = Vec::new();
        file.take(u64::try_from(max_bytes).context("gate read length overflow")?)
            .read_to_end(&mut bytes)
            .context("reading gate log")?;
        return Ok(bytes);
    }
    let task = ctx.store.get(
        &attempt
            .record
            .identity
            .as_ref()
            .context("gate has no identity")?
            .task,
    )?;
    let source = attempt.log.to_string_lossy();
    let artifact = task
        .artifacts
        .iter()
        .rev()
        .find(|a| a.source == source)
        .context("retained gate log not found")?;
    let bytes = crate::artifact_read(ctx, artifact)?;
    let start = usize::try_from(start)
        .context("gate offset overflow")?
        .min(bytes.len());
    Ok(bytes[start..start.saturating_add(max_bytes).min(bytes.len())].to_vec())
}
