use std::{
    fs::{self, File, TryLockError},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use corral_types::{GateAttempt, GateAttemptPage, GateAttemptState};
use jiff::Timestamp;
use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::{
    Actor, Check, CheckKind, CheckResult, Engine, Run, Store,
    artifacts::{ArtifactOwner, ArtifactTarget},
    config::EffectiveProject,
    events::EventKind,
    store::encode,
};

pub(crate) const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS gate_attempts (
        seq INTEGER PRIMARY KEY,
        task TEXT NOT NULL REFERENCES tasks(id),
        id TEXT NOT NULL,
        document TEXT NOT NULL,
        UNIQUE(task, id)
    );
    CREATE INDEX IF NOT EXISTS gate_attempts_latest ON gate_attempts(task, seq);
";

const LOG_WINDOW: u64 = 8 * 1024;
const LOG_LINES: usize = 200;
// Leave room for both the command and a failed check's notes in worst-case JSON.
const COMMAND_LIMIT: usize = 768;
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);
const STALE_SECONDS: i64 = 5;

#[derive(Clone, Serialize, Deserialize)]
struct Progress {
    heartbeat: Timestamp,
    command: Option<String>,
    step: Option<u32>,
}

struct GateRunner {
    attempt: GateAttempt,
    // Separate from stdout/stderr: a surviving gate child must not keep the lease alive.
    _lease: File,
    path: PathBuf,
    progress: Progress,
    published: Instant,
}

impl Engine {
    pub(crate) fn run_gate(
        &self,
        run: &Run,
        round: u32,
        project: Option<&EffectiveProject>,
        actor: Actor,
        effects_started: &mut bool,
    ) -> Result<Check> {
        self.require_worktree(run)?;
        let sha = corral_git::rev_sha(&run.worktree, "HEAD").context("read gate HEAD")?;
        let steps = u32::try_from(project.map_or(0, |project| project.gate.len()))
            .context("too many gate commands")?;
        *effects_started = true;
        let (mut runner, mut log) = GateRunner::start(self, run, sha, round, steps)?;
        let result = (|| {
            let notes = runner.commands(run, project, &mut log)?;
            self.record_gate_result(&mut runner, run, notes, actor)
        })();
        match result {
            Ok(check) => {
                // The immutable artifact now backs this attempt's log read.
                let _ = fs::remove_dir_all(&runner.path);
                Ok(check)
            }
            Err(error) => {
                let _ = writeln!(log, "\nGate error: {error:#}");
                runner.finish(GateAttemptState::Error, None, last_output(&log));
                self.store
                    .transaction(|store| store.finish_gate_attempt(&runner.attempt))?;
                Err(error)
            }
        }
    }

    fn record_gate_result(
        &self,
        runner: &mut GateRunner,
        run: &Run,
        notes: Option<String>,
        actor: Actor,
    ) -> Result<Check> {
        let log = File::open(runner.path.join("output.log")).context("open completed gate log")?;
        let output_at = last_output(&log);
        let bytes = log.metadata().context("stat completed gate log")?.len();
        let blob = self.prepare_gate_log(log, bytes, || runner.beat())?;
        ensure!(
            corral_git::rev_sha(&run.worktree, "HEAD").context("read HEAD after gate")?
                == runner.attempt.sha,
            "HEAD changed while gate was running; run corral gate {} again",
            run.task
        );
        let pass = notes.is_none();
        let prefix = runner.attempt.sha.get(..8).context("invalid gate SHA")?;
        let title = format!("gate-{prefix}-{}.log", runner.attempt.id);
        self.store.transaction(|store| {
            let artifact = self.add_gate_log_in(
                store,
                ArtifactTarget {
                    owner: ArtifactOwner::Task(&run.task),
                    run: Some(&run.id),
                    actor,
                    replaces: None,
                },
                blob,
                &title,
            )?;
            let check = Check {
                task: run.task.clone(),
                kind: CheckKind::Gate,
                key: runner.attempt.sha.clone(),
                result: if pass {
                    CheckResult::Pass
                } else {
                    CheckResult::Fail
                },
                notes,
                log: Some(artifact.reference()),
                by: actor,
                at: Timestamp::now(),
                round: Some(runner.attempt.round),
                run: Some(run.id.clone()),
            };
            runner.finish(
                if pass {
                    GateAttemptState::Passed
                } else {
                    GateAttemptState::Failed
                },
                Some(check.clone()),
                output_at,
            );
            store.finish_gate_attempt(&runner.attempt)?;
            store.insert_check(&check)?;
            let mut event = serde_json::to_value(&check).context("encode gate event")?;
            event["attempt_id"] = serde_json::Value::String(runner.attempt.id.clone());
            self.event(
                store,
                Some(&run.task),
                Some(&run.id),
                EventKind::Gate,
                actor,
                event,
            )?;
            Ok(check)
        })
    }

    pub fn latest_gate_attempt(&self, task: &str) -> Result<Option<GateAttempt>> {
        self.store
            .optional_document(
                "SELECT document FROM gate_attempts WHERE task=?1 ORDER BY seq DESC LIMIT 1",
                [task],
            )?
            .map(|attempt| self.observe_gate(attempt))
            .transpose()
    }

    /// A selector can only address an attempt registered under the specified task.
    /// Offsets are exclusive byte boundaries, not paths or line numbers.
    pub fn gate_attempt_page(
        &self,
        task: &str,
        attempt: &str,
        before: Option<u64>,
    ) -> Result<Option<GateAttemptPage>> {
        let Some(stored) = self.store.gate_attempt(task, attempt)? else {
            return Ok(None);
        };
        let mut attempt = self.observe_gate(stored)?;
        let mut log = self.open_gate_log(&attempt)?;
        if log.is_none() && attempt.finished.is_none() {
            // Completion may have removed the live file between observation and open.
            if let Some(finished) = self.store.gate_attempt(task, &attempt.id)?
                && finished.finished.is_some()
            {
                log = self.open_gate_log(&finished)?;
                attempt = finished;
            }
        }
        let (start, end, total, text) = log.map_or_else(
            || Ok((0, 0, 0, String::new())),
            |log| read_window(log, before),
        )?;
        Ok(Some(GateAttemptPage {
            attempt,
            start,
            end,
            total,
            text,
        }))
    }

    fn open_gate_log(&self, attempt: &GateAttempt) -> Result<Option<File>> {
        if let Some(reference) = attempt
            .check
            .as_ref()
            .and_then(|check| check.log.as_deref())
        {
            let artifact = self.require_artifact(reference)?;
            return self.open_artifact_blob(&artifact).map(Some);
        }
        match File::open(
            self.gate_path(&attempt.task, &attempt.id)
                .join("output.log"),
        ) {
            Ok(file) => Ok(Some(file)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error).context("open live gate log"),
        }
    }

    fn gate_path(&self, task: &str, id: &str) -> PathBuf {
        self.state_dir
            .join("runs")
            .join(task)
            .join("gates")
            .join(id)
    }

    fn observe_gate(&self, mut attempt: GateAttempt) -> Result<GateAttempt> {
        if attempt.finished.is_some() {
            return Ok(attempt);
        }
        let path = self.gate_path(&attempt.task, &attempt.id);
        let alive = lease_held(&path);
        if alive != Some(true)
            && let Some(finished) = self.store.gate_attempt(&attempt.task, &attempt.id)?
            && finished.finished.is_some()
        {
            // A normal exit publishes its result before releasing its lease.
            return Ok(finished);
        }
        if let Some(progress) = read_progress(&path) {
            attempt.heartbeat = Some(progress.heartbeat);
            attempt.command = progress.command;
            attempt.step = progress.step;
        }
        attempt.last_output = File::open(path.join("output.log"))
            .ok()
            .as_ref()
            .and_then(last_output);
        let fresh = attempt.heartbeat.is_some_and(|at| {
            Timestamp::now().as_second().saturating_sub(at.as_second()) < STALE_SECONDS
        });
        attempt.state = match alive {
            Some(false) => GateAttemptState::Interrupted,
            Some(true) if fresh => GateAttemptState::Running,
            _ => GateAttemptState::Stale,
        };
        Ok(attempt)
    }
}

impl Store {
    fn gate_attempt(&self, task: &str, id: &str) -> Result<Option<GateAttempt>> {
        self.optional_document(
            "SELECT document FROM gate_attempts WHERE task=?1 AND id=?2",
            params![task, id],
        )
    }

    fn finish_gate_attempt(&self, attempt: &GateAttempt) -> Result<()> {
        self.connection
            .execute(
                "UPDATE gate_attempts SET document=?3 WHERE task=?1 AND id=?2",
                params![attempt.task, attempt.id, encode(attempt)?],
            )
            .context("finish gate attempt")?;
        Ok(())
    }
}

impl GateRunner {
    fn start(
        engine: &Engine,
        run: &Run,
        sha: String,
        round: u32,
        steps: u32,
    ) -> Result<(Self, File)> {
        let parent = engine.gate_path(&run.task, "");
        fs::create_dir_all(&parent).context("create gate attempts directory")?;
        let dir = tempfile::Builder::new()
            .prefix("gate-")
            .rand_bytes(16)
            .tempdir_in(parent)
            .context("allocate unique gate attempt")?;
        let path = dir.path().to_owned();
        let id = path
            .file_name()
            .and_then(|name| name.to_str())
            .context("read gate attempt id")?;
        let lease = File::create(path.join("runner.lock")).context("create gate runner lease")?;
        lease.lock().context("acquire gate runner lease")?;
        let log = File::create(path.join("output.log")).context("create gate log")?;
        let started = Timestamp::now();
        let mut runner = Self {
            attempt: GateAttempt {
                id: id.to_owned(),
                task: run.task.clone(),
                run: run.id.clone(),
                sha,
                round,
                started,
                heartbeat: None,
                last_output: None,
                command: None,
                step: None,
                steps,
                state: GateAttemptState::Running,
                finished: None,
                check: None,
            },
            _lease: lease,
            path,
            progress: Progress {
                heartbeat: started,
                command: None,
                step: None,
            },
            published: Instant::now(),
        };
        runner.publish()?;
        engine
            .store
            .connection
            .execute(
                "INSERT INTO gate_attempts(task, id, document) VALUES(?1, ?2, ?3)",
                params![run.task, runner.attempt.id, encode(&runner.attempt)?],
            )
            .context("start gate attempt")?;
        let _ = dir.keep();
        Ok((runner, log))
    }

    fn finish(
        &mut self,
        state: GateAttemptState,
        check: Option<Check>,
        last_output: Option<Timestamp>,
    ) {
        self.attempt.state = state;
        self.attempt.finished = Some(Timestamp::now());
        self.attempt.check = check;
        self.attempt.last_output = last_output;
        self.attempt.heartbeat = Some(self.progress.heartbeat);
        self.attempt.command.clone_from(&self.progress.command);
        self.attempt.step = self.progress.step;
    }

    fn publish(&mut self) -> Result<()> {
        let mut update =
            tempfile::NamedTempFile::new_in(&self.path).context("stage gate heartbeat")?;
        let now = Timestamp::now();
        let mut progress = self.progress.clone();
        progress.heartbeat = now;
        serde_json::to_writer(&mut update, &progress).context("encode gate heartbeat")?;
        update
            .persist(self.path.join("progress.json"))
            .map_err(|err| err.error)
            .context("publish gate heartbeat")?;
        self.progress = progress;
        self.published = Instant::now();
        Ok(())
    }

    fn beat(&mut self) {
        if self.published.elapsed() >= HEARTBEAT_INTERVAL {
            // A failed observation write cannot change the shell's gate result.
            self.published = Instant::now();
            let _ = self.publish();
        }
    }

    fn commands(
        &mut self,
        run: &Run,
        project: Option<&EffectiveProject>,
        log: &mut File,
    ) -> Result<Option<String>> {
        if let Some(project) = project {
            for (index, line) in project.gate.iter().enumerate() {
                self.progress.command = Some(command_title(line));
                self.progress.step =
                    Some(u32::try_from(index + 1).context("gate step index overflow")?);
                self.publish()?;
                writeln!(log, "$ {line}").context("write gate command to log")?;
                let status = self.run_command(run, project, log, line)?;
                if !status.success() {
                    writeln!(log, "\n{status}").context("write gate failure to log")?;
                    return Ok(Some(command_title(line)));
                }
            }
        }
        Ok(None)
    }

    fn run_command(
        &mut self,
        run: &Run,
        project: &EffectiveProject,
        log: &File,
        line: &str,
    ) -> Result<ExitStatus> {
        let mut child = Command::new("sh")
            .args(["-c", line])
            .current_dir(&run.worktree)
            .envs(&project.env)
            .stdin(Stdio::null())
            .stdout(log.try_clone().context("clone gate stdout log")?)
            .stderr(log.try_clone().context("clone gate stderr log")?)
            .spawn()
            .with_context(|| format!("start gate command {:?}", command_title(line)))?;
        loop {
            if let Some(status) = child.try_wait().context("wait for gate command")? {
                return Ok(status);
            }
            self.beat();
            thread::sleep(Duration::from_millis(100));
        }
    }
}

fn command_title(line: &str) -> String {
    if line.len() <= COMMAND_LIMIT {
        line.to_owned()
    } else {
        format!("{}…", &line[..line.floor_char_boundary(COMMAND_LIMIT)])
    }
}

fn lease_held(path: &Path) -> Option<bool> {
    let lease = File::open(path.join("runner.lock")).ok()?;
    match lease.try_lock_shared() {
        Ok(()) => Some(false),
        Err(TryLockError::WouldBlock) => Some(true),
        Err(TryLockError::Error(_)) => None,
    }
}

fn read_progress(path: &Path) -> Option<Progress> {
    let file = File::open(path.join("progress.json")).ok()?;
    serde_json::from_reader(file.take(8 * 1024)).ok()
}

fn last_output(file: &File) -> Option<Timestamp> {
    let metadata = file.metadata().ok()?;
    if metadata.len() == 0 {
        return None;
    }
    Timestamp::try_from(metadata.modified().ok()?).ok()
}

fn read_window(mut log: File, before: Option<u64>) -> Result<(u64, u64, u64, String)> {
    let metadata = log.metadata().context("stat gate log")?;
    ensure!(metadata.is_file(), "gate log is not a regular file");
    let total = metadata.len();
    let end = before.unwrap_or(total).min(total);
    let start = end.saturating_sub(LOG_WINDOW);
    log.seek(SeekFrom::Start(start))
        .context("seek gate log window")?;
    let mut bytes = Vec::with_capacity((end - start) as usize);
    log.take(end - start)
        .read_to_end(&mut bytes)
        .context("read gate log window")?;
    // Limit rendered rows as well as bytes. Keep an exact byte cursor for what was omitted.
    let content_end = bytes.len() - usize::from(bytes.last() == Some(&b'\n'));
    let first = bytes[..content_end]
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, byte)| **byte == b'\n')
        .nth(LOG_LINES - 1)
        .map_or(0, |(at, _)| at + 1);
    Ok((
        start + first as u64,
        start + bytes.len() as u64,
        total,
        String::from_utf8_lossy(&bytes[first..]).into_owned(),
    ))
}
