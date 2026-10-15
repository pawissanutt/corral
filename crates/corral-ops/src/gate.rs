use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use corral_core::{ArtifactKind, GateRecord, GateResult, Run, Task};
use jiff::Timestamp;
use serde::Deserialize;
use serde_json::json;

use crate::Ctx;
use crate::artifacts::add_artifact;
use crate::prompt::sh_escape;

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
    let dir = ctx.store.state_dir().join("gates");
    let file = |ext: &str| dir.join(format!("{task}-{run}.{ext}"));
    GatePaths {
        script: file("sh"),
        result: file("json"),
        failed: file("failed"),
        log: file("log"),
    }
}

#[derive(Deserialize)]
struct ResultFile {
    result: String,
}

fn q(value: &str) -> String {
    format!("'{}'", sh_escape(value))
}

/// Runs `commands` in order in `worktree`, teeing into the log, stopping at the first failure; writes the failed
/// command, then the result file (via rename, so a reader never sees a partial file).
fn render_script(
    task: &str,
    run: &str,
    sha: &str,
    worktree: &Path,
    commands: &[String],
    paths: &GatePaths,
) -> String {
    let cmds: Vec<String> = commands.iter().map(|c| format!("  {}\n", q(c))).collect();
    let wt = worktree.display().to_string();
    format!(
        r#"#!/usr/bin/env bash
# corral gate for {task} {run} at {sha}
log={log}
result={result}
failed={failed}
finish() {{
  printf '\ncorral gate: %s\n' "$1" | tee -a "$log"
  printf '{{"result":"%s","exit_code":%d}}\n' "$1" "$2" > "$result.tmp" && mv -f "$result.tmp" "$result"
}}
cmds=(
{cmds})
: > "$log"
if ! cd {wt}; then
  printf '%s' {cd_cmd} > "$failed"
  finish fail 1
  exit 1
fi
for c in "${{cmds[@]}}"; do
  printf '\n$ %s\n' "$c" | tee -a "$log"
  bash -c "$c" 2>&1 | tee -a "$log"
  code=${{PIPESTATUS[0]}}
  if [ "$code" -ne 0 ]; then
    printf '%s' "$c" > "$failed"
    finish fail "$code"
    exit "$code"
  fi
done
finish pass 0
"#,
        log = q(&paths.log.display().to_string()),
        result = q(&paths.result.display().to_string()),
        failed = q(&paths.failed.display().to_string()),
        cmds = cmds.concat(),
        wt = q(&wt),
        cd_cmd = q(&format!("cd {wt}")),
    )
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
    Ok(ctx
        .herdr
        .workspace_create(&run.worktree, &label, false)?
        .pane_id)
}

/// Starts the effective gate (`Config::review_policy`) for the task's current run in a herdr pane and records
/// `gate: {result: running, sha, pane}` on the run. The worktree must be clean so the result belongs to HEAD.
/// Errors: invalid review config, no gate commands, no run/worktree, uncommitted changes, a gate still running.
/// A finished previous gate's pane is closed first.
pub fn gate_start(ctx: &Ctx, id: &str) -> Result<GateRecord> {
    ctx.store.config.validate_review()?;
    let task = ctx.store.get(id)?;
    let run = current_run(&task)?;
    let policy = ctx
        .store
        .config
        .review_policy(task.project.as_deref(), task.review);
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
    let paths = gate_paths(ctx, &task.id, &run.id);
    if let Some(prev) = &run.gate {
        if prev.result == GateResult::Running
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
            eprintln!("corral: could not close previous gate pane {p} (ignored): {e:#}");
        }
    }

    let sha = corral_git::rev_sha(&run.worktree, "HEAD")?;
    let dir = ctx.store.state_dir().join("gates");
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    for p in [&paths.result, &paths.failed, &paths.log] {
        remove_if_exists(p)?;
    }
    let script = render_script(&task.id, &run.id, &sha, &run.worktree, &policy.gate, &paths);
    std::fs::write(&paths.script, script)
        .with_context(|| format!("writing {}", paths.script.display()))?;

    let pane = open_run_pane(ctx, &task, &run, "gate")?;
    let command = format!("bash {}", q(&paths.script.display().to_string()));
    let title = format!("{} gate", task.id);
    let launched = ctx.herdr.pane_run(&pane, &command).and_then(|()| {
        ctx.herdr
            .pane_report_metadata(&pane, &title, &[("task", task.id.as_str())])
    });
    if let Err(err) = launched {
        if let Err(close_err) = ctx.herdr.pane_close(&pane) {
            eprintln!("corral: closing gate pane {pane} after failed launch: {close_err:#}");
        }
        return Err(err);
    }

    let record = GateRecord {
        result: GateResult::Running,
        failed_cmd: None,
        at: Timestamp::now(),
        sha: sha.clone(),
        pane: Some(pane.clone()),
    };
    let run_id = run.id.clone();
    ctx.store.update(&task.id, |t| {
        let r = t
            .current_run_mut()
            .filter(|r| r.id == run_id)
            .ok_or_else(|| anyhow!("{}: run {run_id} is no longer current", task.id))?;
        r.gate = Some(record.clone());
        Ok(())
    })?;
    ctx.store.log_event(
        Some(&task.id),
        "gate_started",
        json!({ "run": run.id, "sha": sha, "pane": pane, "commands": policy.gate }),
    )?;
    Ok(record)
}

/// Records the result of the current run's running gate once its result file exists: sets result, failed_cmd and
/// `at`, snapshots the log as a Log artifact "Gate: pass|fail", logs "gate". Returns the finished record (also
/// when it was already finished), or None while the gate is still running. Error when no gate was started.
pub fn gate_collect(ctx: &Ctx, id: &str) -> Result<Option<GateRecord>> {
    let task = ctx.store.get(id)?;
    let run = current_run(&task)?;
    let record = run
        .gate
        .clone()
        .with_context(|| format!("{}: no gate has been started for run {}", task.id, run.id))?;
    if record.result != GateResult::Running {
        return Ok(Some(record));
    }
    let paths = gate_paths(ctx, &task.id, &run.id);
    let text = match std::fs::read_to_string(&paths.result) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("reading {}", paths.result.display())),
    };
    let parsed: ResultFile = serde_json::from_str(text.trim())
        .with_context(|| format!("parsing {}", paths.result.display()))?;
    let (result, failed_cmd) = match parsed.result.as_str() {
        "pass" => (GateResult::Pass, None),
        "fail" => {
            let cmd = std::fs::read_to_string(&paths.failed)
                .with_context(|| format!("reading {}", paths.failed.display()))?;
            (GateResult::Fail, Some(cmd))
        }
        other => bail!("{}: unknown gate result {other:?}", paths.result.display()),
    };

    let mut finished: Option<GateRecord> = None;
    let run_id = run.id.clone();
    let task = ctx.store.update(&task.id, |t| {
        if let Some(r) = t.current_run_mut().filter(|r| r.id == run_id)
            && let Some(g) = r.gate.as_mut().filter(|g| g.result == GateResult::Running)
        {
            g.result = result;
            g.failed_cmd = failed_cmd.clone();
            g.at = Timestamp::now();
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
    ctx.store.log_event(
        Some(&task.id),
        "gate",
        json!({ "run": run_id, "result": record.result.as_str(), "failed_cmd": record.failed_cmd, "sha": record.sha }),
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
pub(crate) fn collect_finished(ctx: &Ctx) -> Result<Vec<String>> {
    let mut done = Vec::new();
    for task in ctx.store.list()? {
        let running = task
            .current_run()
            .and_then(|r| r.gate.as_ref())
            .is_some_and(|g| g.result == GateResult::Running);
        if !running {
            continue;
        }
        match gate_collect(ctx, &task.id) {
            Ok(Some(_)) => done.push(task.id.clone()),
            Ok(None) => {}
            Err(e) => eprintln!("corral: collecting gate of {} failed: {e:#}", task.id),
        }
    }
    Ok(done)
}
