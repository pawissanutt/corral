use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use corral_core::{
    ArtifactKind, CandidateIdentity, CommandReceipt, GateRecord, GateResult, GateTierReceipt,
    HerdrRef, Run, Status, Task,
};
use jiff::Timestamp;
use serde::Deserialize;
use serde_json::json;

use crate::Ctx;
use crate::artifacts::add_artifact;
use crate::gate_script::{qp, render};
use crate::verification;
mod stale;

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

/// Read-only log proof; candidate freshness remains `verification::gate_passed`'s responsibility.
pub(crate) fn gate_log_passed(ctx: &Ctx, task: &Task, run: &Run) -> Result<bool> {
    if run
        .gate
        .as_ref()
        .is_none_or(|g| g.result != GateResult::Pass)
    {
        return Ok(false);
    }
    gate_log_valid(ctx, task, run)
}

/// A completed, unchanged log is required even when the owner overrides a failed check.
pub(crate) fn gate_log_valid(ctx: &Ctx, task: &Task, run: &Run) -> Result<bool> {
    let Some(gate) = &run.gate else {
        return Ok(false);
    };
    let Some(identity) = &gate.identity else {
        return Ok(false);
    };
    let path = attempt_paths(ctx, &task.id, &run.id, Some(&identity.attempt)).log;
    let log = match std::fs::read(&path) {
        Ok(log) => log,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let commands = crate::gate_tier::receipt_commands(&verification::policy(ctx, task)?, gate);
    Ok(
        gate.log_sha256.as_deref() == Some(verification::digest(&log).as_str())
            && command_log_valid(&log, &commands, &gate.commands, gate.result),
    )
}

pub(crate) fn command_log_valid(
    log: &[u8],
    commands: &[String],
    receipts: &[CommandReceipt],
    result: GateResult,
) -> bool {
    let marker: &[u8] = match result {
        GateResult::Pass => b"corral gate: pass",
        GateResult::Fail => b"corral gate: fail",
        GateResult::Running | GateResult::Stale => return false,
    };
    if receipts.is_empty()
        || receipts.len() > commands.len()
        || (result == GateResult::Pass && receipts.len() != commands.len())
    {
        return false;
    }
    let mut end = 0;
    for (index, (receipt, command)) in receipts.iter().zip(commands).enumerate() {
        let (Ok(start), Ok(next)) = (
            usize::try_from(receipt.log_start),
            usize::try_from(receipt.log_end),
        ) else {
            return false;
        };
        let Some(range) = log.get(start..next) else {
            return false;
        };
        let header = format!("\n$ {command}\n");
        if receipt.command != *command
            || start != end
            || !range.starts_with(header.as_bytes())
            || (receipt.exit_code != 0
                && (result != GateResult::Fail || index + 1 != receipts.len()))
        {
            return false;
        }
        end = next;
    }
    if result == GateResult::Fail && receipts.last().is_none_or(|r| r.exit_code == 0) {
        return false;
    }
    // The final command's range must end immediately before the runner's footer.
    // No command output may supply an additional result, even a passing one.
    let mut results = log
        .split(|b| *b == b'\n')
        .map(<[u8]>::trim_ascii)
        .filter(|line| line.starts_with(b"corral gate:"));
    log.get(end..)
        .is_some_and(|tail| tail.trim_ascii() == marker)
        && results.next() == Some(marker)
        && results.next().is_none()
}

#[derive(Deserialize)]
pub(crate) struct ResultFile {
    pub(crate) schema_version: u32,
    pub(crate) owner_token: String,
    pub(crate) identity: CandidateIdentity,
    pub(crate) result: String,
    pub(crate) exit_code: i32,
    pub(crate) commands: Vec<CommandReceipt>,
}

#[derive(Debug)]
struct LegacyGate;

impl std::fmt::Display for LegacyGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("legacy gate has no candidate identity or schema_version; start a new gate")
    }
}

impl std::error::Error for LegacyGate {}

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

pub(crate) fn prepare_owner(path: &Path) -> Result<()> {
    match std::fs::File::open(path) {
        Ok(file) => {
            let metadata = file.metadata().context("inspecting prepared gate owner")?;
            anyhow::ensure!(
                metadata.is_file() && metadata.len() == 64,
                "invalid prepared gate owner"
            );
            let mut token = String::new();
            file.take(65)
                .read_to_string(&mut token)
                .context("reading prepared gate owner")?;
            anyhow::ensure!(
                token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit()),
                "invalid prepared gate owner token"
            );
            return Ok(());
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("opening prepared gate owner"),
    }
    let mut random = [0_u8; 32];
    getrandom::fill(&mut random)
        .map_err(|error| anyhow!("reading gate ownership entropy: {error}"))?;
    let token = verification::digest(&random);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    options
        .open(path)
        .with_context(|| format!("creating {}", path.display()))?
        .write_all(token.as_bytes())
        .context("writing gate owner token")
}

/// Opens a `label` tab in the run's herdr workspace when it still exists, else a new workspace "<id> <label>";
/// both start in the run's worktree.
pub(crate) fn open_run_pane(ctx: &Ctx, task: &Task, run: &Run, label: &str) -> Result<HerdrRef> {
    let workspace = run.herdr.as_ref().map(|h| h.workspace_id.clone());
    if let Some(ws) = workspace
        && ctx.herdr.workspace_list()?.iter().any(|(id, _)| *id == ws)
    {
        let created = ctx.herdr.tab_create(&ws, &run.worktree, label, false)?;
        return Ok(HerdrRef {
            workspace_id: created.workspace_id,
            pane_id: created.pane_id,
            socket: Some(ctx.herdr.socket_path()?),
        });
    }
    let label: String = format!("{} {label}", task.id).chars().take(40).collect();
    let created = ctx.herdr.workspace_create(&run.worktree, &label, false)?;
    ctx.store.log_event(
        Some(&task.id),
        "run_workspace_created",
        json!({ "run": run.id, "workspace": created.workspace_id, "pane": created.pane_id }),
    )?;
    Ok(HerdrRef {
        workspace_id: created.workspace_id,
        pane_id: created.pane_id,
        socket: Some(ctx.herdr.socket_path()?),
    })
}

/// Starts the effective gate (`Config::review_policy`) for the task's current run in a herdr pane and records
/// `gate: {result: running, sha, pane}` on the run. The worktree must be clean so the result belongs to HEAD.
/// Errors: invalid review config, no gate commands, no run/worktree, uncommitted changes, a gate still running.
/// A finished previous gate's pane is closed first.
pub fn gate_start(ctx: &Ctx, id: &str) -> Result<GateRecord> {
    gate_start_with_force(ctx, id, false)
}

/// Manual gate admission; `force` overrides only machine resource pressure.
pub fn gate_start_with_force(ctx: &Ctx, id: &str, force: bool) -> Result<GateRecord> {
    gate_start_origin(ctx, id, force, crate::operating::Origin::Owner, None, None)
}

/// A Pi-native `corral_gate`: the attempt derives from the request unless a retained
/// preparation binds one, and both submit paths are fenced on the request's binding.
pub(crate) fn gate_start_request(
    ctx: &Ctx,
    id: &str,
    request: crate::review_budget::GateRequest,
) -> Result<GateRecord> {
    let owner = crate::operating::Origin::Owner;
    gate_start_origin(ctx, id, false, owner, None, Some(request))
}

/// Start evidence of one attempt: its runner file, written before the first command, or its log.
pub(crate) fn attempt_started(ctx: &Ctx, task: &str, run: &str, attempt: &str) -> bool {
    let log = attempt_paths(ctx, task, run, Some(attempt)).log;
    log.with_extension("runner.json").is_file() || log.is_file()
}

pub(crate) fn gate_start_automatic(ctx: &Ctx, id: &str) -> Result<GateRecord> {
    gate_start_origin(
        ctx,
        id,
        false,
        crate::operating::Origin::Automatic,
        None,
        None,
    )
}

/// A landing boundary's gate: the landing-phase contract even before an approval exists, so
/// a landing on main never runs a reduced tier.
pub(crate) fn gate_start_landing(
    ctx: &Ctx,
    id: &str,
    origin: crate::operating::Origin,
) -> Result<GateRecord> {
    let landing = crate::gate_tier::GatePhase::Landing;
    gate_start_origin(ctx, id, false, origin, Some(landing), None)
}

fn gate_start_origin(
    ctx: &Ctx,
    id: &str,
    force: bool,
    origin: crate::operating::Origin,
    phase: Option<crate::gate_tier::GatePhase>,
    request: Option<crate::review_budget::GateRequest>,
) -> Result<GateRecord> {
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
    let lock =
        ctx.store
            .try_lock(&format!("gate-{id}"))?
            .ok_or(crate::review_budget::LaunchBusy(
                "another gate start is in progress",
            ))?;
    let mut fresh = ctx.clone();
    fresh.store.reload_config_if_changed()?;
    let ctx = &fresh;
    ctx.store.config.validate_review()?;
    let task = ctx.store.get(id)?;
    if crate::plan_pilot::handles(&task) {
        bail!("plan runs use plan review, not a code gate");
    }
    let policy = verification::policy(ctx, &task)?;
    verification::require_gate(&policy, &task)?;
    let run = current_run(&task)?;
    let phase = phase.unwrap_or_else(|| crate::gate_tier::current_phase(&run));
    let contract = crate::gate_tier::select(ctx, &task, &run, phase)?;
    let mut launch = crate::review_budget::ReviewLaunch::reserve_origin(
        ctx,
        id,
        crate::review_budget::Kind::Gate,
        force,
        origin,
    )?;
    let requested = request.as_ref().map(|r| r.attempt.clone());
    if let Some(request) = request {
        launch.bind_request(request);
    }
    if launch.restored() {
        launch.resume()?;
        let record = current_run(&ctx.store.get(id)?)?
            .gate
            .context("resumed gate missing")?;
        launch.commit()?;
        return Ok(record);
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
    let sha = corral_git::rev_sha(&run.worktree, "HEAD")?;
    let attempt = launch.bound_attempt().or(requested).unwrap_or_else(|| {
        format!(
            "{}-{}",
            std::process::id(),
            Timestamp::now().as_nanosecond()
        )
    });
    let origin = crate::gate_tier::origin_step(&run, &contract);
    let identity = match (launch.bound_candidate(), origin.promoted_from()) {
        (Some(candidate), _) => candidate,
        (None, Some(origin)) => verification::identity_with(
            ctx,
            &task,
            &run,
            &attempt,
            origin.identity.environment.clone(),
            contract.binding(),
        )?,
        (None, None) => {
            verification::capture_identity_with(ctx, &task, &run, &attempt, contract.binding())?
        }
    };
    if identity.gate_tier != contract.binding()
        || origin
            .promoted_from()
            .is_some_and(|origin| origin.identity.environment != identity.environment)
    {
        bail!("candidate changed while starting gate");
    }
    launch.bind(&identity, &attempt, None)?;
    if let Some(prev) = &run.gate {
        crate::watch::load(ctx, &task.id)?;
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
        {
            ctx.herdr
                .pane_close(p)
                .with_context(|| format!("closing previous gate pane {p}"))?;
        }
    }

    let paths = attempt_paths(ctx, &task.id, &run.id, Some(&attempt));
    let dir = ctx.store.state_dir().join("gates");
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    // An old collector looks only at this unversioned path. Remove any stale pass
    // before launch, including the window before the versioned task is saved.
    remove_if_exists(&attempt_paths(ctx, &task.id, &run.id, None).result)?;
    let context_path = crate::gate_tier::context_path(&paths.log);
    for p in [&paths.result, &paths.failed, &paths.log, &context_path] {
        remove_if_exists(p)?;
    }
    let owner_path = paths.result.with_extension("owner");
    prepare_owner(&owner_path)?;
    let context = (!contract.rules.is_legacy())
        .then(|| {
            crate::gate_tier::write_context(&context_path, &contract, &identity, &run.worktree)
        })
        .transpose()?;
    let script = render(
        &identity,
        &run.worktree,
        &contract.commands,
        &paths,
        context.as_ref(),
    )?;
    std::fs::write(&paths.script, script)
        .with_context(|| format!("writing {}", paths.script.display()))?;

    let launched = open_run_pane(ctx, &task, &run, "gate")?;
    let pane = launched.pane_id;
    launch.attach(&pane, true)?;
    let command = format!("bash {} </dev/null", qp(&paths.script));
    let title = format!("{} gate", task.id);
    let record = GateRecord {
        identity: Some(identity),
        commands: Vec::new(),
        log_sha256: None,
        result: GateResult::Running,
        process_lost: false,
        failed_cmd: None,
        at: corral_core::clock::now(),
        sha: sha.clone(),
        pane: Some(pane.clone()),
        socket: launched.socket,
        tier: context.map(|context| {
            Box::new(GateTierReceipt {
                tier: context.tier,
                context_sha256: context.sha256,
                carried: None,
                promoted_from: origin.promoted_from().cloned(),
            })
        }),
    };
    let run_id = run.id.clone();
    ctx.store.update(&task.id, |t| {
        let expected = record.identity.as_ref().context("missing gate identity")?;
        if verification::identity(ctx, t, &run, expected)? != *expected {
            bail!("candidate changed while starting gate");
        }
        let r = t
            .current_run_mut()
            .filter(|r| r.id == run_id)
            .ok_or_else(|| anyhow!("{}: run {run_id} is no longer current", task.id))?;
        // The origin is recorded, kept or cleared in the update that replaces the receipt.
        if crate::gate_tier::origin_step(r, &contract) != origin {
            bail!("candidate changed while starting gate");
        }
        origin.apply(r);
        r.gate = Some(record.clone());
        // Retain a completed approval as stale evidence for clean-rebase carry.
        // It still cannot authorize this gate's candidate until explicitly rebound.
        if r.review.as_ref().and_then(|r| r.verdict) != Some(corral_core::Verdict::Approve) {
            r.review = None;
        }
        Ok(())
    })?;
    crate::watch::start_gate(ctx, &task, &run, &record, &contract.commands, &paths.log)?;
    let expected_status = task.status;
    ctx.herdr
        .pane_report_metadata(&pane, &title, &[("task", task.id.as_str())])?;
    launch.submit(
        |store, task| {
            let mut fresh = ctx.clone();
            fresh.store = store.clone();
            let current = current_run(task)?;
            let expected = record.identity.as_ref().context("missing gate identity")?;
            if task.status != expected_status
                || current.id != run.id
                || current.gate.as_ref() != Some(&record)
                || verification::identity(&fresh, task, &current, expected)? != *expected
                || !corral_git::is_clean(&current.worktree)?
            {
                bail!("gate run or candidate changed before submission");
            }
            verification::require_gate(&verification::policy(&fresh, task)?, task)?;
            Ok(())
        },
        || ctx.herdr.pane_run(&pane, &command),
    )?;
    launch.commit()?;
    let mut started = json!({ "run": run.id, "sha": sha, "pane": pane, "commands": contract.commands, "identity": record.identity });
    if let Some(tier) = &record.tier {
        started["tier"] = json!(tier.tier);
    }
    ctx.store
        .log_event(Some(&task.id), "gate_started", started)?;
    // A concurrent fork can briefly retain the file description after this
    // process drops it. Release the advisory lock before reporting success.
    drop(lock);
    Ok(record)
}

/// Records the result of the current run's running gate once its result file exists: sets result, failed_cmd and
/// `at`, snapshots the log as a Log artifact "Gate: pass|fail", logs "gate". Returns the finished record (also
/// when it was already finished), or None while the gate is still running. Error when no gate was started.
pub fn gate_collect(ctx: &Ctx, id: &str) -> Result<Option<GateRecord>> {
    gate_collect_at(ctx, id, corral_core::clock::now())
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
        // A finished receipt is stale once no phase would accept it; the review phase accepts
        // every binding a landing gate may carry.
        let review = crate::gate_tier::GatePhase::Review;
        if record.result == GateResult::Pass
            && !(gate_log_passed(ctx, &task, &run)?
                && verification::gate_fresh_for(
                    ctx,
                    &task,
                    &run,
                    &record,
                    &crate::gate_tier::requirement_for(ctx, &task, &run, review)?,
                )?)
        {
            let old = record.clone();
            let log_passed = stale::log_passed(ctx, &task, &run, &record)?;
            record.result = if log_passed {
                GateResult::Stale
            } else {
                GateResult::Fail
            };
            record.failed_cmd = Some(if log_passed {
                stale::reason(ctx, &task, &run, &record, None)?
            } else {
                "invalid_command_receipts: gate log missing, changed or inconsistent".into()
            });
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
    record.identity.as_ref().ok_or(LegacyGate)?;
    let paths = gate_paths(ctx, &task.id, &run.id);
    let text = match std::fs::read_to_string(&paths.result) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("reading {}", paths.result.display())),
    };
    let parsed: serde_json::Value = serde_json::from_str(text.trim())
        .with_context(|| format!("parsing {}", paths.result.display()))?;
    if parsed.is_object() && parsed.get("schema_version").is_none() {
        return Err(LegacyGate.into());
    }
    let parsed: ResultFile = serde_json::from_value(parsed)
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
    let commands = crate::gate_tier::receipt_commands(&verification::policy(ctx, &task)?, &record);
    let receipts_valid = command_log_valid(
        &log,
        &commands,
        &parsed.commands,
        match parsed.result.as_str() {
            "pass" => GateResult::Pass,
            "fail" => GateResult::Fail,
            other => bail!("{}: unknown gate result {other:?}", paths.result.display()),
        },
    );
    let mut command_failed = false;
    let stale = record.identity.as_ref() != Some(&parsed.identity)
        || !verification::gate_fresh(ctx, &task, &run, &record)?;
    let reported_commands: Vec<_> = parsed.commands.iter().map(|r| r.command.clone()).collect();
    let (mut result, mut failed_cmd) = if stale
        && parsed.result == "pass"
        && parsed.exit_code == 0
        && command_log_valid(&log, &reported_commands, &parsed.commands, GateResult::Pass)
    {
        (
            GateResult::Stale,
            Some(stale::reason(
                ctx,
                &task,
                &run,
                &record,
                Some(&parsed.identity),
            )?),
        )
    } else if !receipts_valid
        || (parsed.result == "pass"
            && (parsed.exit_code != 0
                || parsed.commands.len() != commands.len()
                || parsed.commands.iter().any(|r| r.exit_code != 0)))
    {
        (GateResult::Fail, Some("invalid_command_receipts".into()))
    } else {
        match parsed.result.as_str() {
            "pass" => (GateResult::Pass, None),
            "fail" => {
                let cmd = std::fs::read_to_string(&paths.failed)
                    .with_context(|| format!("reading {}", paths.failed.display()))?;
                command_failed = true;
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
            result = GateResult::Stale;
            failed_cmd = Some(stale::reason(ctx, t, current, &record, None)?);
        }
        if let Some(r) = t.current_run_mut().filter(|r| r.id == run_id)
            && let Some(g) = r.gate.as_mut().filter(|g| **g == record)
        {
            g.result = result;
            g.process_lost = false;
            g.failed_cmd = failed_cmd.clone();
            g.at = now;
            g.commands = parsed.commands.clone();
            g.log_sha256 = Some(verification::digest(&log));
            finished = Some(g.clone());
            if matches!(result, GateResult::Fail | GateResult::Stale) {
                r.failures = r.failures.saturating_add(1);
            }
            // A quick approval is never promoted after a full command failure, even onto a
            // later PASS. Staleness, invalid receipts and lost panes keep the origin.
            if command_failed
                && record
                    .identity
                    .as_ref()
                    .is_some_and(|i| i.gate_tier.is_none())
            {
                r.promotion_origin = None;
            }
        }
        Ok(())
    })?;
    let Some(record) = finished else {
        // Another caller recorded it between our read and our update.
        return Ok(task.current_run().and_then(|r| r.gate.clone()));
    };
    crate::watch::load(ctx, &task.id)?;
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
    let mut detail = json!({ "run": run_id, "result": record.result.as_str(), "failed_cmd": record.failed_cmd, "sha": record.sha, "identity": record.identity, "commands": record.commands, "log_sha256": record.log_sha256 });
    if let Some(tier) = &record.tier {
        detail["tier"] = json!(tier.tier);
        detail["context_sha256"] = json!(tier.context_sha256);
    }
    ctx.store.log_event(Some(&task.id), "gate", detail)?;
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
        let gate = task
            .current_run()
            .and_then(|r| r.gate.as_ref())
            .context("task has no running gate")?;
        let pane = gate.pane.as_deref();
        let listed = crate::pane_liveness::inventory(ctx);
        let gone = listed.is_some()
            && crate::pane_liveness::gone(
                ctx,
                crate::pane_liveness::Sample {
                    task: &task,
                    pane,
                    started: gate.at,
                    seen: false,
                    present: pane.is_some_and(|id| {
                        listed.as_ref().is_some_and(|inventory| {
                            inventory.panes.iter().any(|p| p.pane_id == id)
                        })
                    }),
                    at: Timestamp::now(),
                    ticket: listed.as_ref().map(|inventory| inventory.ticket),
                    count_miss: true,
                },
            )?;
        // Collecting attaches artifacts and may wait for storage: release the lease first (T-509).
        drop(listed);
        if gone {
            crate::agent::effects::failpoint("gate_wait_collect");
            // The script may have finished just before the pane closed.
            if let Some(record) = gate_collect(ctx, id)? {
                return Ok(record);
            }
            bail!(
                "{}: gate pane {} closed before the gate finished",
                task.id,
                pane.unwrap_or("?")
            );
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// Collects finished gates of nonterminal tasks (called by sync). Legacy gates cannot authorize a pass
/// and are skipped with one summary diagnostic. Returns the task ids whose gate was recorded.
pub(crate) fn collect_finished(ctx: &Ctx, now: Timestamp) -> Result<Vec<String>> {
    let mut done = Vec::new();
    let mut legacy = 0;
    for task in ctx.store.list()? {
        if !crate::session_scope::owns_task(ctx, &task)? {
            continue;
        }
        if matches!(task.status, Status::Done | Status::Dropped) {
            continue;
        }
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
            Err(e) if e.is::<LegacyGate>() => legacy += 1,
            Err(e) => crate::messages::report(
                Some(&task.id),
                "gate",
                format!("collecting gate of {} failed: {e:#}", task.id),
            ),
        }
    }
    if legacy > 0 {
        crate::messages::report(
            None,
            "gate",
            format!(
                "skipped {legacy} legacy gate record(s); start new gates to verify active tasks"
            ),
        );
    }
    Ok(done)
}
