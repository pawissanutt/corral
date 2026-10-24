//! The crash harness: a child process runs a deterministic [`Script`] and stops at one barrier
//! point inside an operation's transaction, the parent kills it with SIGKILL, reopens the
//! database and compares it with the state an in-process reference run recorded after the last
//! committed operation. No sleeps: the child reports the barrier and blocks on stdin.

use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use anyhow::{Context, Result, anyhow, bail, ensure};
use corral_core::backend::conformance::{ViewDump, snapshot};
use corral_core::backend::*;
use corral_core::sqlite::{Barrier, SqliteBackend, SqliteOptions};
use serde_json::json;
use tempfile::TempDir;

use super::{ChildLines, Fixture, child_command, fixture_at, hear, samples, tell};

const SCRIPT_ENV: &str = "CORRAL_SQLITE_TEST_SCRIPT";
const POINT_ENV: &str = "CORRAL_SQLITE_TEST_POINT";
const COMMITTED: &str = "committed";

/// Deterministic filler operations, then the conformance sample of `kind` and its operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Script {
    pub kind: OpKind,
    pub fillers: u64,
}

impl Script {
    fn encode(&self) -> Result<String> {
        Ok(format!(
            "{}:{}",
            serde_json::to_string(&self.kind)?,
            self.fillers
        ))
    }

    fn decode(text: &str) -> Result<Self> {
        let (kind, fillers) = text
            .rsplit_once(':')
            .ok_or_else(|| anyhow!("script {text:?}"))?;
        Ok(Self {
            kind: serde_json::from_str(kind)?,
            fillers: fillers.parse()?,
        })
    }

    fn run(&self, fx: &Fixture) -> Result<()> {
        let apply = |op: Operation| {
            let kind = op.kind();
            fx.backend
                .apply(op)
                .map(drop)
                .map_err(|error| anyhow!("{kind:?}: {error}"))
        };
        for i in 0..self.fillers {
            let now = fx.clock.now();
            apply(if i % 2 == 0 {
                Operation::AppendEvents(AppendEvents {
                    events: vec![samples::event(fx, None, "note", json!({"filler": i}))],
                    reservations: Vec::new(),
                })
            } else {
                samples::put_records(
                    vec![samples::record(
                        "filler",
                        &i.to_string(),
                        None,
                        Some(json!({"i": i})),
                        now,
                    )],
                    Vec::new(),
                )
            })?;
        }
        apply(samples::sample(self.kind, fx)?)
    }
}

/// What a reference run of a script did: every in-transaction barrier point with the index of
/// the operation it belongs to, and the whole store after each number of committed operations
/// (`states[0]` is the empty store).
pub struct Plan {
    pub points: Vec<(usize, OpKind, String)>,
    pub states: Vec<ViewDump>,
}

impl Plan {
    /// The points inside the script's last operation.
    pub fn final_points(&self) -> Vec<usize> {
        let last = self.states.len().saturating_sub(2);
        (0..self.points.len())
            .filter(|&i| self.points[i].0 == last)
            .collect()
    }
}

type Recorded = Arc<Mutex<(Vec<(usize, OpKind, String)>, Vec<Result<ViewDump, String>>)>>;

pub fn plan(script: Script) -> Result<Plan> {
    let dir = TempDir::new()?;
    let corral = dir.path().join(".corral");
    let recorded: Recorded = Arc::new(Mutex::new((Vec::new(), Vec::new())));
    let observer: Arc<OnceLock<SqliteBackend>> = Arc::new(OnceLock::new());
    let barrier: Barrier = {
        let recorded = Arc::clone(&recorded);
        let observer = Arc::clone(&observer);
        Arc::new(move |point| {
            let mut recorded = recorded.lock().unwrap_or_else(PoisonError::into_inner);
            if point.what == COMMITTED {
                let state = match observer.get() {
                    Some(backend) => snapshot(backend).map_err(|error| format!("{error:#}")),
                    None => Err("no observer handle".to_owned()),
                };
                recorded.1.push(state);
            } else {
                let op_index = recorded.1.len() - 1;
                recorded.0.push((op_index, point.op, point.what.to_owned()));
            }
        })
    };
    let fx = fixture_at(&corral, None, Some(barrier))?;
    let backend = SqliteBackend::open(&corral, SqliteOptions::default())?;
    let initial = snapshot(&backend).map_err(|error| format!("{error:#}"));
    recorded
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .1
        .push(initial);
    if observer.set(backend).is_err() {
        bail!("the observer handle was set twice");
    }
    script.run(&fx)?;
    drop(fx);
    let (points, states) =
        std::mem::take(&mut *recorded.lock().unwrap_or_else(PoisonError::into_inner));
    let states = states
        .into_iter()
        .collect::<Result<Vec<_>, String>>()
        .map_err(|error| anyhow!("reference state: {error}"))?;
    Ok(Plan { points, states })
}

/// The child role: run the script and stop at in-transaction point number `POINT_ENV`.
pub fn child(dir: &std::path::Path) -> Result<()> {
    let script = Script::decode(&std::env::var(SCRIPT_ENV)?)?;
    let target: usize = std::env::var(POINT_ENV)?.parse()?;
    let seen = Arc::new(AtomicUsize::new(0));
    let barrier: Barrier = Arc::new(move |point| {
        if point.what == COMMITTED {
            return;
        }
        if seen.fetch_add(1, Ordering::SeqCst) == target {
            if tell(&format!("BARRIER {:?} {}", point.op, point.what)).is_ok() {
                let _parent_closed_stdin = hear();
            }
            std::process::abort();
        }
    });
    let fx = fixture_at(&dir.join(".corral"), None, Some(barrier))?;
    script.run(&fx)?;
    tell("FINISHED")?;
    bail!("the script finished without reaching point {target}")
}

/// Kills a child of `entry` at `point` of `script` and checks the reopened store.
pub fn kill_at(entry: &str, script: Script, plan: &Plan, point: usize) -> Result<()> {
    let (op_index, op, what) = plan
        .points
        .get(point)
        .ok_or_else(|| anyhow!("no point {point}"))?;
    let dir = TempDir::new()?;
    let mut child = child_command(entry, "kill", dir.path())?
        .env(SCRIPT_ENV, script.encode()?)
        .env(POINT_ENV, point.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawn the crash child")?;
    let stdout = child.stdout.take().context("child stdout")?;
    let reached = ChildLines::new(stdout).wait_for("BARRIER");
    child.kill().context("kill the crash child")?;
    let output = child.wait_with_output()?;
    let reached = reached.with_context(|| {
        format!(
            "{script:?} point {point}: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    })?;
    ensure!(
        reached == format!("BARRIER {op:?} {what}"),
        "{script:?} point {point}: reached {reached:?}, planned {what}"
    );
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        ensure!(
            output.status.signal() == Some(9),
            "{script:?} point {point}: child ended with {:?}",
            output.status
        );
    }
    let backend = SqliteBackend::open(&dir.path().join(".corral"), SqliteOptions::default())?;
    let report = backend.check()?;
    ensure!(
        report.is_ok(),
        "{script:?} point {point}: integrity {:?}",
        report.problems
    );
    let after = snapshot(&backend)?;
    ensure!(
        after == plan.states[*op_index],
        "{script:?} killed at {what} (point {point}) left a state other than the {op_index} \
         committed operations before it"
    );
    Ok(())
}
