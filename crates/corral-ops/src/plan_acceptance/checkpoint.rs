//! Advisory full-tier checkpoints on an owned Plan integration target. A checkpoint gates the
//! target head between landings with the full tier and an empty expected-failure set. It is
//! journaled apart from aggregate attempts, so it never counts as aggregate PASS; it moves no
//! ref, and a red result only prepares a `corral new` command for the owner.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use corral_core::{
    CandidateIdentity, Event, GateRecord, GateResult, OperatingActionKind, OperatingMode,
    PlanDeliveryTarget, ReviewPolicy, Status, Store, Task, operating_contract_revision,
};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{capture, gate};
use crate::Ctx;
use crate::gate_script::q;
use crate::verification::digest;

/// The identity run label of a checkpoint; never the aggregate run.
const CHECKPOINT_RUN: &str = "checkpoint";
/// The operating resolver's tracker-wide Gate refusal; a checkpoint is the one tracker gate.
const TRACKER_NOT_EXECUTABLE: &str = "tracker Plan is not executable";
const JOURNAL_VERSION: u32 = 1;

/// One advisory full-tier gate of a Plan target head.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanCheckpoint {
    /// `C<n>`, numbered apart from aggregate attempts.
    pub id: String,
    /// Always true: a checkpoint never counts as aggregate PASS and moves no ref.
    pub advisory: bool,
    /// Started by autopilot under `[review] plan_checkpoint_every`.
    pub automatic: bool,
    pub target: String,
    pub head: String,
    /// Landings on the target included in `head`.
    pub landings: usize,
    pub started: Timestamp,
    pub identity: CandidateIdentity,
    pub gate: GateRecord,
    /// The last gate step the log announced, when the checkpoint is red.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_step: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log: Option<PathBuf>,
    /// Prepared for the owner when red; corral never files or dispatches it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix_command: Option<String>,
}

/// Landings since the last checkpoint, as `corral plan progress` shows them.
#[derive(Debug, Serialize)]
pub struct CheckpointProgress {
    /// `[review] plan_checkpoint_every`; None means checkpoints only on demand.
    pub every: Option<u32>,
    pub landings: usize,
    pub landings_since_checkpoint: usize,
    pub last: Option<String>,
    pub last_result: Option<GateResult>,
    /// Autopilot starts one advisory checkpoint on its next sync.
    pub due: bool,
}

#[derive(Default, Serialize, Deserialize)]
struct Journal {
    version: u32,
    attempts: Vec<PlanCheckpoint>,
}

/// The record area of the per-Plan checkpoint journals, keyed by Plan id.
pub(super) const JOURNAL_AREA: &str = "plan-checkpoints";

impl Journal {
    fn load(store: &Store, plan: &str) -> Result<Self> {
        let Some(journal) = store.record::<Self>(JOURNAL_AREA, plan)? else {
            return Ok(Self {
                version: JOURNAL_VERSION,
                attempts: Vec::new(),
            });
        };
        ensure!(
            journal.version == JOURNAL_VERSION,
            "unsupported Plan checkpoint journal version {}",
            journal.version
        );
        Ok(journal)
    }

    /// Callers hold the Plan's verification lock, the only writer of this record. Inside a
    /// task update the write joins it, so it commits with the attempt's gate row.
    fn save(&self, store: &Store, plan: &str) -> Result<()> {
        store.save_record(JOURNAL_AREA, plan, self)
    }
}

/// The integration branch of a tracker Plan with acceptance criteria, else why it has none.
fn target_branch(tracker: &Task) -> Result<String> {
    ensure!(
        tracker.plan_acceptance_required(),
        "{} is not a tracker Plan with approved acceptance criteria",
        tracker.id
    );
    match tracker.approved_scope.as_ref().map(|s| &s.delivery_target) {
        Some(PlanDeliveryTarget::Integration { branch }) => Ok(branch.clone()),
        _ => bail!(
            "{} delivers to main; checkpoints run only on a Plan integration target",
            tracker.id
        ),
    }
}

/// Published landings on the Plan's target, in publication order.
fn landings(events: &[Event], plan: &str) -> usize {
    events
        .iter()
        .filter(|event| {
            event.kind == "approved" && event.detail["target_plan"]["plan"].as_str() == Some(plan)
        })
        .count()
}

fn progress_of(store: &Store, tracker: &Task) -> Result<CheckpointProgress> {
    let journal = Journal::load(store, &tracker.id)?;
    let landings = landings(&store.events_shared()?, &tracker.id);
    let last = journal.attempts.last();
    let completed = journal
        .attempts
        .iter()
        .rev()
        .find(|a| a.gate.result != GateResult::Cancelled);
    let since = landings.saturating_sub(completed.map_or(0, |attempt| attempt.landings));
    let every = store
        .config
        .plan_checkpoint_every(tracker.project.as_deref());
    Ok(CheckpointProgress {
        every,
        landings,
        landings_since_checkpoint: since,
        last: last.map(|attempt| attempt.id.clone()),
        last_result: last.map(|attempt| attempt.gate.result),
        due: every.is_some_and(|every| since > 0 && since >= every as usize)
            && !matches!(tracker.status, Status::Done | Status::Dropped),
    })
}

/// Checkpoint state of an integration-target Plan; None for main-delivery Plans.
pub(crate) fn progress(ctx: &Ctx, tracker: &Task) -> Result<Option<CheckpointProgress>> {
    if target_branch(tracker).is_err() {
        return Ok(None);
    }
    progress_of(&ctx.store, tracker).map(Some)
}

/// Run one advisory full-tier checkpoint on the Plan target head in the verification
/// checkout. Blocks until the gate finishes; a red result is returned, not an error.
///
/// # Errors
/// Refuses main-delivery or accepted Plans, an uninitialized target, a missing gate and a
/// concurrent verification or checkpoint.
pub fn plan_checkpoint(ctx: &Ctx, id: &str) -> Result<PlanCheckpoint> {
    checkpoint(ctx, id, false)?.context("Plan checkpoint did not start")
}

/// Autopilot's background entry: runs only while the Plan is still due. A busy or no longer
/// due Plan is a no-op, so concurrent syncs start one checkpoint.
///
/// # Errors
/// As [`plan_checkpoint`], once the checkpoint is due.
pub fn plan_checkpoint_automatic(ctx: &Ctx, id: &str) -> Result<Option<PlanCheckpoint>> {
    checkpoint(ctx, id, true)
}

fn checkpoint(ctx: &Ctx, id: &str, automatic: bool) -> Result<Option<PlanCheckpoint>> {
    ctx.store.check_runtime()?;
    // The verification checkout is shared with `plan verify`.
    let Some(_lock) = ctx.store.try_lock(&format!("plan-verify-{id}"))? else {
        if automatic {
            return Ok(None);
        }
        bail!("another Plan verification or checkpoint is in progress");
    };
    let store = ctx.store.reopen()?;
    let tracker = store.get(id)?;
    crate::plan_recovery::require_active(&store, &tracker)?;
    crate::hold::require_clear(&store, &tracker)?;
    let branch = target_branch(&tracker)?;
    ensure!(
        !matches!(tracker.status, Status::Done | Status::Dropped),
        "{} is {}",
        tracker.id,
        tracker.status.label()
    );
    if automatic && !progress_of(&store, &tracker)?.due {
        return Ok(None);
    }
    // Re-read at the execution boundary: a pause or revoked grant since scheduling wins.
    if automatic && let Some(reason) = automatic_refusal(&store, &tracker)? {
        crate::messages::report(
            Some(id),
            "plan-checkpoint",
            format!("automatic Plan checkpoint for {id} not started: {reason}"),
        );
        return Ok(None);
    }
    let mut journal = Journal::load(&store, id)?;
    // Holding the lock proves no checkpoint process still owns a running attempt. Each lost
    // attempt's journal entry, gate row and scratch closure commit together.
    let lost: Vec<usize> = (0..journal.attempts.len())
        .filter(|&index| journal.attempts[index].gate.result == GateResult::Running)
        .collect();
    for index in lost {
        let staged = gate::put_lost(ctx, id, gate::CHECKPOINT_SCOPE, &journal.attempts[index].id)?;
        let gate = &mut journal.attempts[index].gate;
        gate.result = GateResult::Fail;
        gate.process_lost = true;
        gate.failed_cmd =
            Some("process_lost: the checkpoint ended before its gate finished".into());
        let outcome = (gate.result, gate.failed_cmd.clone());
        store.update(id, |_| {
            journal.save(&store, id)?;
            staged.as_ref().map_or(Ok(()), |staged| {
                staged.stage(&store, (outcome.0, outcome.1.as_deref()))
            })
        })?;
    }
    let policy = crate::verification::policy(ctx, &tracker)?;
    crate::verification::require_gate(&policy, &tracker)?;
    let attempt_id = format!("C{}", journal.attempts.len() + 1);
    let repo = capture::repo(&store, &tracker)?;
    let verify = repo
        .join(".worktrees")
        .join(format!("{}-verify", tracker.id));
    let (identity, landed) = {
        let _landing = corral_git::landing_lock(&repo)?;
        let (head, _, initial) = crate::plan_target::plan_head(&store, &tracker, &branch)?
            .with_context(|| format!("Plan target {branch} has no landing yet"))?;
        let identity = identity(
            ctx,
            &store,
            &tracker,
            &repo,
            &policy,
            &attempt_id,
            &head,
            &initial,
        )?;
        corral_git::verification_checkout(&repo, &verify, &head)?;
        (identity, landings(&store.events_shared()?, id))
    };
    let mut attempt = PlanCheckpoint {
        id: attempt_id,
        advisory: true,
        automatic,
        target: branch,
        head: identity.head.clone(),
        landings: landed,
        started: corral_core::clock::now(),
        gate: GateRecord {
            identity: Some(identity.clone()),
            commands: Vec::new(),
            log_sha256: None,
            result: GateResult::Running,
            process_lost: false,
            failed_cmd: None,
            at: corral_core::clock::now(),
            sha: identity.head.clone(),
            pane: None,
            socket: None,
            tier: None,
        },
        identity,
        failed_step: None,
        log: None,
        fix_command: None,
    };
    let previous = journal.attempts.last().map(|previous| previous.id.clone());
    let run = gate::PlanGate {
        attempt: &attempt.id,
        scope: gate::CHECKPOINT_SCOPE,
        previous: previous.as_deref(),
    };
    let budget = crate::review_budget::hold_guard(ctx, id)?;
    let contract = crate::gate_tier::aggregate(&store.config, &tracker, &policy);
    let prepared = gate::prepare(ctx, &attempt.identity, run, &verify, &contract)?;
    journal.attempts.push(attempt.clone());
    store.update(id, |task| {
        crate::plan_recovery::require_active(&store, task)?;
        crate::hold::require_clear(&store, task)?;
        task.review_wait = None;
        journal.save(&store, id)?;
        store.log_event(
            Some(id),
            "plan_checkpoint_started",
            json!({"attempt": attempt.id, "target": attempt.target, "head": attempt.head,
                   "landings": attempt.landings, "automatic": automatic, "advisory": true}),
        )?;
        prepared.stage_start(&store)
    })?;
    drop(budget);
    crate::gate::evidence::failpoint("plan_checkpoint_running");
    let mut record = match prepared.execute(ctx, &attempt.identity, &contract.commands) {
        Ok(record) => record,
        Err(error) => GateRecord {
            result: GateResult::Fail,
            failed_cmd: Some(format!("gate_error: {error:#}")),
            at: corral_core::clock::now(),
            ..attempt.gate.clone()
        },
    };
    if let Some(cancelled) = Journal::load(&store, id)?
        .attempts
        .last()
        .filter(|a| a.id == attempt.id && a.gate.result == GateResult::Cancelled)
    {
        record = cancelled.gate.clone();
    }
    if record.result == GateResult::Pass
        && let Some(reason) = gate::checkout_changed(&verify, &attempt.head)?
    {
        record.result = GateResult::Fail;
        record.failed_cmd = Some(format!("verification_checkout_changed: {reason}"));
    }
    let log = prepared.paths.log.clone();
    if log.is_file() {
        crate::artifacts::add_artifact(
            ctx,
            id,
            &log,
            Some(corral_core::ArtifactKind::Log),
            Some(&format!(
                "Plan checkpoint {} (advisory): {}",
                attempt.id,
                record.result.as_str()
            )),
        )?;
        attempt.log = Some(log.clone());
    }
    attempt.gate = record;
    if attempt.gate.result == GateResult::Fail {
        attempt.failed_step = failed_step(&log)?;
        attempt.fix_command = Some(prepare_fix(&store, &tracker, &attempt)?);
    }
    let stored = journal
        .attempts
        .last_mut()
        .context("checkpoint attempt missing")?;
    *stored = attempt.clone();
    let staged = prepared.put_result(ctx)?;
    store.update(id, |_| {
        let canonical = Journal::load(&store, id)?;
        if let Some(cancelled) = canonical
            .attempts
            .last()
            .filter(|a| a.id == attempt.id && a.gate.result == GateResult::Cancelled)
        {
            attempt = cancelled.clone();
            journal = canonical;
        }
        journal.save(&store, id)?;
        store.log_event(
            Some(id),
            "plan_checkpoint",
            json!({"attempt": attempt.id, "result": attempt.gate.result.as_str(),
               "failed_cmd": attempt.gate.failed_cmd, "failed_step": attempt.failed_step,
               "log": attempt.log, "target": attempt.target, "head": attempt.head,
               "landings": attempt.landings, "automatic": automatic, "advisory": true,
               "tier": attempt.gate.tier.as_ref().map(|tier| &tier.tier),
               "fix_command": attempt.fix_command, "identity": attempt.identity}),
        )?;
        // The row ends with exactly the outcome this journal entry records.
        let outcome = (attempt.gate.result, attempt.gate.failed_cmd.as_deref());
        staged
            .as_ref()
            .map_or(Ok(()), |staged| staged.stage(&store, outcome))
    })?;
    Ok(Some(attempt))
}

pub(crate) fn automatic(store: &Store, id: &str) -> Result<bool> {
    Ok(Journal::load(store, id)?
        .attempts
        .last()
        .is_some_and(|a| a.automatic))
}

pub(crate) fn cancelled(store: &Store, id: &str) -> Result<bool> {
    Ok(Journal::load(store, id)?
        .attempts
        .last()
        .is_some_and(|a| a.gate.result == GateResult::Cancelled))
}

pub(crate) fn cancel(ctx: &Ctx, id: &str, reason: &str) -> Result<bool> {
    let journal = Journal::load(&ctx.store, id)?;
    let Some(attempt) = journal
        .attempts
        .last()
        .filter(|a| a.gate.result == GateResult::Running)
    else {
        return Ok(false);
    };
    crate::gate::stop_descendants(&gate::paths(ctx, id, &attempt.id).script)?;
    let staged = gate::put_lost(ctx, id, gate::CHECKPOINT_SCOPE, &attempt.id)?;
    let changed = ctx.store.try_update_head(id, |task| {
        let mut journal = Journal::load(&ctx.store, id)?;
        let Some(current) = journal.attempts.last_mut().filter(|a| a.id == attempt.id && a.gate.result == GateResult::Running) else { return Ok(false); };
        current.gate.result = GateResult::Cancelled;
        current.gate.failed_cmd = None;
        current.gate.at = ctx.store.now();
        current.fix_command = None;
        current.failed_step = None;
        task.review_reservation = None;
        journal.save(&ctx.store, id)?;
        if let Some(staged) = &staged { staged.stage(&ctx.store, (GateResult::Cancelled, None))?; }
        ctx.store.log_event(Some(id), "gate_cancelled", json!({"target":"checkpoint","attempt":attempt.id,"result":"cancelled","reason":reason}))?;
        Ok(true)
    })?;
    Ok(changed.is_some())
}

#[allow(clippy::too_many_arguments)]
fn identity(
    ctx: &Ctx,
    store: &Store,
    tracker: &Task,
    repo: &Path,
    policy: &ReviewPolicy,
    attempt: &str,
    head: &str,
    initial: &str,
) -> Result<CandidateIdentity> {
    let environment = capture::verifier_environment(ctx, tracker, repo, policy)?;
    Ok(CandidateIdentity {
        version: 2,
        task: tracker.id.clone(),
        run: CHECKPOINT_RUN.into(),
        attempt: attempt.into(),
        head: head.into(),
        tree: corral_git::rev_sha(repo, &format!("{head}^{{tree}}"))?,
        integration_base: corral_git::rev_sha(repo, "refs/heads/main")?,
        dispatch_base: initial.into(),
        spec_digest: digest(operating_contract_revision(tracker)?.as_bytes()),
        policy_digest: digest(&serde_json::to_vec(&(
            policy,
            store.effective_operating_policy()?.revision,
        ))?),
        inputs_digest: digest(&serde_json::to_vec(&(
            &environment,
            crate::verification::config_inputs(&store.config)?,
            capture::prompt_inputs(store)?,
            include_str!("../gate_script.rs"),
            env!("CARGO_PKG_VERSION"),
        ))?),
        environment,
        gate_tier: None,
    })
}

/// The last step the log announced (`==> <step>`); gate.sh closes a failed step and stops.
fn failed_step(log: &Path) -> Result<Option<String>> {
    let text = match std::fs::read(log) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("reading {}", log.display())),
    };
    Ok(text
        .lines()
        .rev()
        .filter_map(|line| line.strip_prefix("==> "))
        .find(|step| !step.starts_with("tier "))
        .map(|step| step.trim().to_owned()))
}

/// Write the fix spec beside the journal and return the owner's `corral new` command.
fn prepare_fix(store: &Store, tracker: &Task, attempt: &PlanCheckpoint) -> Result<String> {
    let failed = attempt.gate.failed_cmd.as_deref().unwrap_or("see the log");
    let step = attempt
        .failed_step
        .as_deref()
        .unwrap_or_else(|| failed.lines().next().unwrap_or(failed));
    let step: String = step.chars().take(80).collect();
    let log = attempt
        .log
        .as_ref()
        .map_or_else(|| "none".to_owned(), |log| log.display().to_string());
    let body = format!(
        "## Goal\nRestore a green full gate on Plan target {target} ({plan}).\n\n\
         ## Evidence\nAdvisory checkpoint {id} ran the full tier with an empty expected-failure \
         set on {head} and failed at step `{step}`.\n\nFailed command: {failed}\n\nLog: {log}\n\n\
         ## Acceptance\nThe full gate passes on the Plan target head, and `corral plan checkpoint \
         {plan}` is green.\n",
        target = attempt.target,
        plan = tracker.id,
        id = attempt.id,
        head = attempt.head,
    );
    let path = store
        .scratch_spool(
            corral_core::backend::ScratchKind::TmpStaging,
            "plan-checkpoints",
        )?
        .join(format!("{}-{}-fix.md", tracker.id, attempt.id));
    let dir = path.parent().context("fix spec has no directory")?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
    let title = format!("Fix {} checkpoint {}: {step}", attempt.target, attempt.id);
    let project = tracker.project.as_deref().context("Plan has no project")?;
    Ok(format!(
        "corral new {} -p {} --kind bug --body-file {}",
        q(&title),
        q(project),
        q(&path.display().to_string())
    ))
}

/// The PM card text for a Plan whose latest checkpoint is red; nothing is filed or dispatched.
pub(crate) fn red_card(ctx: &Ctx, tracker: &Task) -> Result<Option<String>> {
    if target_branch(tracker).is_err() {
        return Ok(None);
    }
    let journal = Journal::load(&ctx.store, &tracker.id)?;
    // A running or lost retry resolves nothing; only a later completed pass clears the card.
    let Some(attempt) = journal
        .attempts
        .iter()
        .rev()
        .find(|attempt| attempt.gate.result != GateResult::Running && !attempt.gate.process_lost)
        .filter(|attempt| attempt.gate.result == GateResult::Fail)
    else {
        return Ok(None);
    };
    Ok(Some(format!(
        "Advisory Plan checkpoint {} on {} at {} is red: failed step `{}`; failed command: {}; log {}. \
         Nothing was filed or dispatched. Prepared command for the owner: {}",
        attempt.id,
        attempt.target,
        attempt.head,
        attempt.failed_step.as_deref().unwrap_or("unknown"),
        attempt.gate.failed_cmd.as_deref().unwrap_or("unknown"),
        attempt
            .log
            .as_ref()
            .map_or_else(|| "missing".to_owned(), |log| log.display().to_string()),
        attempt.fix_command.as_deref().unwrap_or("none"),
    )))
}

/// Autopilot: start one background checkpoint for every Plan target that is due. Failures are
/// reported per Plan and never stop the sync.
pub(crate) fn start_due(ctx: &Ctx) {
    start_due_with(ctx, &mut |id| launch(ctx, id));
}

pub(crate) fn start_due_with(ctx: &Ctx, launch: &mut dyn FnMut(&str) -> Result<()>) {
    // A Done or Dropped Plan has no checkpoint left to take.
    let open = ctx
        .store
        .heads(&corral_core::backend::HeadQuery::Open { project: None })
        .and_then(|heads| heads.iter().map(|head| head.to_shell_task()).collect());
    let tasks: Vec<corral_core::Task> = match open {
        Ok(tasks) => tasks,
        Err(error) => {
            crate::messages::report(None, "plan-checkpoint", format!("{error:#}"));
            return;
        }
    };
    for tracker in tasks.iter().filter(|task| task.tracker) {
        let due = (|| -> Result<bool> {
            let due = progress(ctx, tracker)?.is_some_and(|progress| progress.due)
                && automatic_refusal(&ctx.store, tracker)?.is_none();
            // A running verification or checkpoint owns the checkout; look again next sync.
            Ok(due
                && ctx
                    .store
                    .try_lock(&format!("plan-verify-{}", tracker.id))?
                    .is_some())
        })();
        let started = due.and_then(|due| if due { launch(&tracker.id) } else { Ok(()) });
        if let Err(error) = started {
            crate::messages::report(
                Some(&tracker.id),
                "plan-checkpoint",
                format!("automatic Plan checkpoint for {}: {error:#}", tracker.id),
            );
        }
    }
}

/// Why the confirmed operating policy does not authorize an automatic checkpoint, if it does
/// not. It is the resolver's automatic Gate decision for the tracker (pause, Assist, scope,
/// delegated authority, lifecycle, owner decisions, legacy autopilot off), waiving only the
/// rule that a tracker is not executable. Explicit `corral plan checkpoint` needs no grant.
fn automatic_refusal(store: &Store, tracker: &Task) -> Result<Option<String>> {
    let effective = store.effective_operating_policy()?;
    if effective.paused {
        return Ok(Some("operating policy is paused".into()));
    }
    if effective.mode != OperatingMode::Autopilot {
        return Ok(Some("no confirmed Autopilot grant".into()));
    }
    let Some(policy) = effective.tasks.get(&tracker.id) else {
        return Ok(Some("Plan is outside the confirmed workspace grant".into()));
    };
    let reasons: Vec<&str> = policy
        .refusals
        .get(&OperatingActionKind::Gate)
        .into_iter()
        .flatten()
        .map(String::as_str)
        .filter(|reason| *reason != TRACKER_NOT_EXECUTABLE)
        .collect();
    Ok((!reasons.is_empty()).then(|| reasons.join("; ")))
}

/// A detached `corral plan checkpoint --automatic`, so a full gate never blocks the sync.
fn launch(ctx: &Ctx, id: &str) -> Result<()> {
    let mut command = std::process::Command::new(crate::prompt::corral_bin()?);
    command
        .arg("--root")
        .arg(ctx.store.root())
        .args(["plan", "checkpoint", id, "--automatic"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = command
        .spawn()
        .context("starting the automatic Plan checkpoint")?;
    let plan = id.to_owned();
    std::thread::spawn(move || {
        if let Err(error) = child.wait() {
            crate::messages::report(
                Some(&plan),
                "plan-checkpoint",
                format!("waiting for the automatic Plan checkpoint: {error}"),
            );
        }
    });
    Ok(())
}

#[cfg(all(test, unix))]
#[path = "checkpoint_tests.rs"]
mod tests;
