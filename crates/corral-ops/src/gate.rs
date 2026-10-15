use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use corral_core::{
    ArtifactKind, CandidateIdentity, CommandReceipt, GateRecord, GateResult, Run, Task,
};
use jiff::Timestamp;
use serde::Deserialize;
use serde_json::json;

use crate::Ctx;
use crate::artifacts::add_artifact;
use crate::gate_script::{q, render};
use crate::verification;

/// Files of one gate run under `.corral/state/gates/`, all named `<task>-<run>.*`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatePaths {
    /// The bash script the gate pane runs.
    pub script: PathBuf,
    /// Machine-readable result, written last: `{"result":"pass"|"fail","exit_code":N}`.
    pub result: PathBuf,
    /// The command that failed, as plain text (fail only).
    pub failed: PathBuf,
    /// Combined output of every command.
    pub log: PathBuf,
}

pub fn gate_paths(ctx: &Ctx, task: &str, run: &str) -> GatePaths {
    let attempt = ctx
        .store
        .get(task)
        .ok()
        .and_then(|t| t.runs.into_iter().find(|r| r.id == run))
        .and_then(|r| r.gate.and_then(|g| g.identity))
        .map(|i| i.attempt);
    attempt_paths(ctx, task, run, attempt.as_deref())
}

fn attempt_paths(ctx: &Ctx, task: &str, run: &str, attempt: Option<&str>) -> GatePaths {
    let dir = ctx.store.state_dir().join("gates");
    let stem = match attempt {
        Some(a) => format!("{task}-{run}-{a}"),
        None => format!("{task}-{run}"),
    };
    let file = |ext: &str| dir.join(format!("{stem}.{ext}"));
    GatePaths {
        script: file("sh"),
        result: file("json"),
        failed: file("failed"),
        log: file("log"),
    }
}

#[derive(Deserialize)]
struct ResultFile {
    schema_version: u32,
    owner_token: String,
    identity: CandidateIdentity,
    result: String,
    exit_code: i32,
    commands: Vec<CommandReceipt>,
}

/// Current HEAD of the run's worktree, or None when the worktree is gone or unreadable.
pub fn worktree_head(run: &Run) -> Option<String> {
    if !run.worktree.is_dir() {
        return None;
    }
    corral_git::rev_sha(&run.worktree, "HEAD").ok()
}

fn current_run(task: &Task) -> Result<Run> {
    task.current_run()
        .cloned()
        .with_context(|| format!("task {} has no run", task.id))
}

fn pane_alive(ctx: &Ctx, pane: Option<&str>) -> Result<bool> {
    match pane {
        Some(p) => Ok(ctx.herdr.pane_get(p)?.is_some()),
        None => Ok(false),
    }
}

fn remove_if_exists(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("removing {}", path.display())),
    }
}

/// Opens a `label` tab in the run's herdr workspace when it still exists, else a new workspace "<id> <label>";
/// both start in the run's worktree.
pub(crate) fn open_run_pane(ctx: &Ctx, task: &Task, run: &Run, label: &str) -> Result<String> {
    let workspace = run.herdr.as_ref().map(|h| h.workspace_id.clone());
    if let Some(ws) = workspace
        && ctx.herdr.workspace_list()?.iter().any(|(id, _)| *id == ws)
    {
        return Ok(ctx
            .herdr
            .tab_create(&ws, &run.worktree, label, false)?
            .pane_id);
    }
    let label: String = format!("{} {label}", task.id).chars().take(40).collect();
    let created = ctx.herdr.workspace_create(&run.worktree, &label, false)?;
    ctx.store.log_event(
        Some(&task.id),
        "run_workspace_created",
        json!({ "run": run.id, "workspace": created.workspace_id, "pane": created.pane_id }),
    )?;
    Ok(created.pane_id)
}

/// Starts the effective gate (`Config::review_policy`) for the task's current run in a herdr pane and records
/// `gate: {result: running, sha, pane}` on the run. The worktree must be clean so the result belongs to HEAD.
/// Errors: invalid review config, no gate commands, no run/worktree, uncommitted changes, a gate still running.
/// A finished previous gate's pane is closed first.
pub fn gate_start(ctx: &Ctx, id: &str) -> Result<GateRecord> {
    ctx.store.check_runtime()?;
    if let Some(inherited) = std::env::var_os("CORRAL_GATE_ATTEMPT")
        && ctx.store.list()?.iter().any(|task| {
            task.current_run()
                .and_then(|run| run.gate.as_ref())
                .and_then(|gate| gate.identity.as_ref())
                .is_some_and(|identity| std::ffi::OsStr::new(&identity.attempt) == inherited)
        })
    {
        bail!("cannot start a nested gate from inside a gate attempt");
    }
    let _lock = ctx
        .store
        .try_lock(&format!("gate-{id}"))?
        .context("another gate start is in progress")?;
    ctx.store.config.validate_review()?;
    let task = ctx.store.get(id)?;
    let run = current_run(&task)?;
    let policy = verification::policy(ctx, &task)?;
    if policy.gate.is_empty() {
        bail!(
            "no gate commands configured for {} (set `gate` under [review] or [projects.<p>.review])",
            task.id
        );
    }
    if !run.worktree.is_dir() {
        bail!("{}: worktree {} is gone", task.id, run.worktree.display());
    }
    if !corral_git::is_clean(&run.worktree)? {
        bail!(
            "{}: worktree {} has uncommitted changes; commit them so the gate result belongs to a commit",
            task.id,
            run.worktree.display()
        );
    }
    if let Some(prev) = &run.gate {
        if prev.result == GateResult::Running
            && prev.identity.is_some()
            && gate_collect(ctx, &task.id)?.is_none()
            && pane_alive(ctx, prev.pane.as_deref())?
        {
            bail!(
                "{}: a gate is already running in pane {}",
                task.id,
                prev.pane.as_deref().unwrap_or("?")
            );
        }
        if let Some(p) = prev.pane.as_deref()
            && pane_alive(ctx, Some(p))?
            && let Err(e) = ctx.herdr.pane_close(p)
        {
            crate::messages::report(
                Some(&task.id),
                "cleanup",
                format!("could not close previous gate pane {p} (ignored): {e:#}"),
            );
        }
    }

    let sha = corral_git::rev_sha(&run.worktree, "HEAD")?;
    let attempt = format!(
        "{}-{}",
        std::process::id(),
        Timestamp::now().as_nanosecond()
    );
    let identity = verification::identity(ctx, &task, &run, &attempt)?;
    let paths = attempt_paths(ctx, &task.id, &run.id, Some(&attempt));
    let dir = ctx.store.state_dir().join("gates");
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    // An old collector looks only at this unversioned path. Remove any stale pass
    // before launch, including the window before the versioned task is saved.
    remove_if_exists(&attempt_paths(ctx, &task.id, &run.id, None).result)?;
    for p in [&paths.result, &paths.failed, &paths.log] {
        remove_if_exists(p)?;
    }
    let mut random = [0_u8; 32];
    std::fs::File::open("/dev/urandom")
        .context("opening gate ownership entropy")?
        .read_exact(&mut random)
        .context("reading gate ownership entropy")?;
    let token = verification::digest(&random);
    let owner_path = paths.result.with_extension("owner");
    let mut owner = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&owner_path)
        .with_context(|| format!("creating {}", owner_path.display()))?;
    owner
        .write_all(token.as_bytes())
        .with_context(|| format!("writing {}", owner_path.display()))?;
    let script = render(&identity, &run.worktree, &policy.gate, &paths)?;
    std::fs::write(&paths.script, script)
        .with_context(|| format!("writing {}", paths.script.display()))?;

    let pane = open_run_pane(ctx, &task, &run, "gate")?;
    let command = format!("bash {} </dev/null", q(&paths.script.display().to_string()));
    let title = format!("{} gate", task.id);
    let launched = ctx.herdr.pane_run(&pane, &command).and_then(|()| {
        ctx.herdr
            .pane_report_metadata(&pane, &title, &[("task", task.id.as_str())])
    });
    if let Err(err) = launched {
        if let Err(close_err) = ctx.herdr.pane_close(&pane) {
            crate::messages::report(
                Some(&task.id),
                "cleanup",
                format!("closing gate pane {pane} after failed launch: {close_err:#}"),
            );
        }
        return Err(err);
    }

    let record = GateRecord {
        identity: Some(identity),
        commands: Vec::new(),
        log_sha256: None,
        result: GateResult::Running,
        failed_cmd: None,
        at: Timestamp::now(),
        sha: sha.clone(),
        pane: Some(pane.clone()),
    };
    let run_id = run.id.clone();
    ctx.store.update(&task.id, |t| {
        if verification::identity(ctx, t, &run, &attempt)?
            != *record.identity.as_ref().context("missing gate identity")?
        {
            bail!("candidate changed while starting gate");
        }
        let r = t
            .current_run_mut()
            .filter(|r| r.id == run_id)
            .ok_or_else(|| anyhow!("{}: run {run_id} is no longer current", task.id))?;
        r.gate = Some(record.clone());
        r.review = None;
        Ok(())
    })?;
    ctx.store.log_event(
        Some(&task.id),
        "gate_started",
        json!({ "run": run.id, "sha": sha, "pane": pane, "commands": policy.gate, "identity": record.identity }),
    )?;
    Ok(record)
}

/// Records the result of the current run's running gate once its result file exists: sets result, failed_cmd and
/// `at`, snapshots the log as a Log artifact "Gate: pass|fail", logs "gate". Returns the finished record (also
/// when it was already finished), or None while the gate is still running. Error when no gate was started.
pub fn gate_collect(ctx: &Ctx, id: &str) -> Result<Option<GateRecord>> {
    gate_collect_at(ctx, id, Timestamp::now())
}

fn gate_collect_at(ctx: &Ctx, id: &str, now: Timestamp) -> Result<Option<GateRecord>> {
    ctx.store.check_runtime()?;
    let task = ctx.store.get(id)?;
    let run = current_run(&task)?;
    let mut record = run
        .gate
        .clone()
        .with_context(|| format!("{}: no gate has been started for run {}", task.id, run.id))?;
    if record.result != GateResult::Running {
        if record.result == GateResult::Pass
            && !verification::gate_fresh(ctx, &task, &run, &record)?
        {
            let old = record.clone();
            record.result = GateResult::Fail;
            record.failed_cmd =
                Some("stale_result: candidate, spec, base or policy changed".into());
            ctx.store.update(&task.id, |t| {
                if let Some(r) = t
                    .current_run_mut()
                    .filter(|r| r.id == run.id && r.gate.as_ref() == Some(&old))
                {
                    r.gate = Some(record.clone());
                } else {
                    bail!("gate changed during stale-result invalidation");
                }
                Ok(())
            })?;
            ctx.store.log_event(
                Some(&task.id),
                "gate_invalidated",
                json!({"identity":record.identity, "reason":record.failed_cmd}),
            )?;
        }
        return Ok(Some(record));
    }
    record
        .identity
        .as_ref()
        .context("legacy gate has no candidate identity; start a new gate")?;
    let paths = gate_paths(ctx, &task.id, &run.id);
    let text = match std::fs::read_to_string(&paths.result) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("reading {}", paths.result.display())),
    };
    let parsed: ResultFile = serde_json::from_str(text.trim())
        .with_context(|| format!("parsing {}", paths.result.display()))?;
    if parsed.schema_version != 2 {
        bail!("unsupported gate result schema {}", parsed.schema_version);
    }
    let owner_path = paths.result.with_extension("owner");
    let owner_token = std::fs::read_to_string(&owner_path)
        .with_context(|| format!("reading gate ownership {}", owner_path.display()))?;
    if owner_token.is_empty() || parsed.owner_token != owner_token {
        bail!("gate result ownership token does not match this attempt");
    }
    let log =
        std::fs::read(&paths.log).with_context(|| format!("reading {}", paths.log.display()))?;
    let policy = verification::policy(ctx, &task)?;
    let receipts_valid = parsed.commands.len() <= policy.gate.len()
        && parsed.commands.iter().zip(&policy.gate).all(|(r, c)| {
            r.command == *c && r.log_start <= r.log_end && r.log_end <= log.len() as u64
        });
    let stale = record.identity.as_ref() != Some(&parsed.identity)
        || !verification::gate_fresh(ctx, &task, &run, &record)?;
    let (mut result, mut failed_cmd) = if stale {
        (
            GateResult::Fail,
            Some("stale_result: candidate, attempt, spec, base or policy changed".into()),
        )
    } else if !receipts_valid
        || (parsed.result == "pass"
            && (parsed.exit_code != 0
                || parsed.commands.len() != policy.gate.len()
                || parsed.commands.iter().any(|r| r.exit_code != 0)))
    {
        (GateResult::Fail, Some("invalid_command_receipts".into()))
    } else {
        match parsed.result.as_str() {
            "pass" => (GateResult::Pass, None),
            "fail" => {
                let cmd = std::fs::read_to_string(&paths.failed)
                    .with_context(|| format!("reading {}", paths.failed.display()))?;
                (GateResult::Fail, Some(cmd))
            }
            other => bail!("{}: unknown gate result {other:?}", paths.result.display()),
        }
    };

    let mut finished: Option<GateRecord> = None;
    let run_id = run.id.clone();
    let task = ctx.store.update(&task.id, |t| {
        if result == GateResult::Pass
            && let Some(current) = t.current_run().filter(|r| r.id == run_id)
            && !verification::gate_fresh(ctx, t, current, &record)?
        {
            result = GateResult::Fail;
            failed_cmd =
                Some("stale_result: verification inputs changed at collection boundary".into());
        }
        if let Some(r) = t.current_run_mut().filter(|r| r.id == run_id)
            && let Some(g) = r.gate.as_mut().filter(|g| **g == record)
        {
            g.result = result;
            g.failed_cmd = failed_cmd.clone();
            g.at = now;
            g.commands = parsed.commands.clone();
            g.log_sha256 = Some(verification::digest(&log));
            finished = Some(g.clone());
            if result == GateResult::Fail {
                r.failures = r.failures.saturating_add(1);
            }
        }
        Ok(())
    })?;
    let Some(record) = finished else {
        // Another caller recorded it between our read and our update.
        return Ok(task.current_run().and_then(|r| r.gate.clone()));
    };
    let title = format!("Gate: {}", record.result.as_str());
    if paths.log.is_file() {
        add_artifact(
            ctx,
            &task.id,
            &paths.log,
            Some(ArtifactKind::Log),
            Some(&title),
        )?;
    }
    add_artifact(
        ctx,
        &task.id,
        &paths.result,
        Some(ArtifactKind::Evidence),
        Some("Gate command receipts"),
    )?;
    ctx.store.log_event(
        Some(&task.id),
        "gate",
        json!({ "run": run_id, "result": record.result.as_str(), "failed_cmd": record.failed_cmd, "sha": record.sha, "identity": record.identity, "commands": record.commands, "log_sha256": record.log_sha256 }),
    )?;
    Ok(Some(record))
}

/// Blocks until the current run's running gate finishes (polling its result file), then records it. Error when
/// the gate pane disappears before a result is written.
pub fn gate_wait(ctx: &Ctx, id: &str) -> Result<GateRecord> {
    loop {
        if let Some(record) = gate_collect(ctx, id)? {
            return Ok(record);
        }
        let task = ctx.store.get(id)?;
        let pane = task
            .current_run()
            .and_then(|r| r.gate.as_ref())
            .and_then(|g| g.pane.clone());
        if !pane_alive(ctx, pane.as_deref())? {
            // The script may have finished just before the pane closed.
            if let Some(record) = gate_collect(ctx, id)? {
                return Ok(record);
            }
            bail!(
                "{}: gate pane {} closed before the gate finished",
                task.id,
                pane.as_deref().unwrap_or("?")
            );
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// Collects every finished gate whose task still records it as running (called by sync). Returns the task ids
/// whose gate was recorded.
pub(crate) fn collect_finished(ctx: &Ctx, now: Timestamp) -> Result<Vec<String>> {
    let mut done = Vec::new();
    for task in ctx.store.list()? {
        let running = task
            .current_run()
            .and_then(|r| r.gate.as_ref())
            .is_some_and(|g| g.result == GateResult::Running);
        if !running {
            continue;
        }
        match gate_collect_at(ctx, &task.id, now) {
            Ok(Some(_)) => done.push(task.id.clone()),
            Ok(None) => {}
            Err(e) => crate::messages::report(
                Some(&task.id),
                "gate",
                format!("collecting gate of {} failed: {e:#}", task.id),
            ),
        }
    }
    Ok(done)
}
