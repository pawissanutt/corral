//! Policy replacement is mandatory coordination, not an optional board-event push.
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use corral_core::records::RecordArea;
use corral_core::{OperatingActionKind, OperatingAuthority, OperatingMode, OperatingScope};
use corral_herdr::Delivery;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::Ctx;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PolicySnapshot {
    mode: OperatingMode,
    paused: bool,
    revision: String,
    authorization: Option<String>,
    scope: OperatingScope,
    authority: OperatingAuthority,
    permissions: BTreeMap<String, BTreeSet<OperatingActionKind>>,
    refusals: BTreeMap<String, BTreeMap<OperatingActionKind, Vec<String>>>,
    reasons: Vec<String>,
}

const ACTIONS: [OperatingActionKind; 8] = [
    OperatingActionKind::Dispatch,
    OperatingActionKind::Plan,
    OperatingActionKind::Gate,
    OperatingActionKind::Review,
    OperatingActionKind::PlanReview,
    OperatingActionKind::Repair,
    OperatingActionKind::Accept,
    OperatingActionKind::AcceptPlan,
];

/// Caller holds the dispatch boundary so config, receipt and effective state agree.
pub(crate) fn snapshot(ctx: &Ctx) -> Result<PolicySnapshot> {
    let layers = ctx.store.config_layers()?;
    layers.validate(ctx.store.root())?;
    let config = layers.config()?;
    let effective = ctx.store.effective_operating_policy()?;
    let authority = if effective.authorization.is_some() {
        config.automation.authority
    } else {
        OperatingAuthority::default()
    };
    let permissions = effective
        .tasks
        .keys()
        .map(|id| {
            (
                id.clone(),
                ACTIONS
                    .into_iter()
                    .filter(|action| effective.allows(id, *action))
                    .collect(),
            )
        })
        .collect();
    Ok(PolicySnapshot {
        mode: effective.mode,
        paused: effective.paused,
        revision: effective.revision,
        authorization: effective.authorization,
        scope: config.automation.scope,
        authority,
        permissions,
        refusals: effective
            .tasks
            .into_iter()
            .map(|(id, task)| (id, task.refusals))
            .collect(),
        reasons: effective.reasons,
    })
}

/// Launch context is orientation, not a task permission cache. Keep its size independent
/// of task count; fresh typed reads supply task permissions, refusals and scope members.
pub(crate) fn native_summary(policy: &PolicySnapshot) -> Value {
    json!({
        "mode": policy.mode,
        "paused": policy.paused,
        "revision": policy.revision,
        "authorization": policy.authorization,
        "authority": policy.authority,
        "scope_counts": {
            "projects": policy.scope.projects.len(),
            "tasks": policy.scope.tasks.len(),
            "observed_tasks": policy.permissions.len(),
        },
        "task_details": "Read fresh corral_brief, corral_next and corral_show; launch summary omits per-task permissions, refusals and scope members.",
    })
}

/// The PM's standing authority. Every PM prompt layer quotes this one text, so the policy
/// replacement, the generic runbook and the role context cannot disagree about it.
pub(crate) const PM_AUTHORITY: &str = "\
## PM authority\n\n\
You operate this board inside the configured workspace projects. When work is stuck, you unblock it: act, \
then record what you did and why in a task comment. Waiting for the owner is correct only for \
the owner-only list below.\n\n\
You may run, through typed tools or the corral CLI: dispatch and queue changes, gate and re-gate, \
review start, send-back, plan verdicts and acceptance, hold and dispatch-hold release, your own \
parks, re-authorizing stale queued consent (`dispatch <id>`), gate-failure attribution \
(`repair triage`, with `--owner` to revise one), one `repair renew` per task up to the starter \
allowance, capacity and ad-hoc reserve changes, and filing then dispatching follow-up tasks. You \
land with `approve <id> --merge ff` only on a passing configured gate and a fresh independent \
approval; the command refuses you otherwise. The policy JSON describes only what Corral's \
executor does on its own. When the executor refuses a step for a pause or a missing grant, run \
the explicit command yourself: a card option that is not owner-required and has no action ID is \
yours to run.\n\n\
Owner-only, asked once with options and a recommendation while other work continues: paid or \
live-provider spend beyond the configured budget; credentials and privacy; destructive actions on \
unfinished work (discard, dropping a task with a live branch, lost-run recovery); project scope and \
priority (admitting a project, promoting an Idea, `automation confirm`, `mode autopilot`, \
`mode turbo`, resuming a paused policy, owner parks, owner-stopped runs, scope exclusions); landing \
where `accept = \"owner\"` is set; overriding a failed gate or a reviewer's changes verdict; \
re-arming a consumed diagnostic retry; recovering an uncertain delivery (`inbox recover`); \
config under `review`, `projects`, reviewer routing or project admission; `owner_only` and `permission` \
questions; waiving independent review.\n\n\
Checks stay: never act as the independent reviewer, never invent a receipt or evidence, and read \
the receipt of an uncertain effect before repeating it.\n\n\
Red integration base: when the base branch itself fails a gate check and blocks candidates, get it \
fixed now. Tell a blocked worker to fix a mechanical failure in a separate `baseline:` commit, or \
file a `red-main` task and dispatch it at once. Formatter or generated output you may commit on the \
base yourself, touching only those files and proving it with the failing gate command.\n";

pub(crate) fn instructions(policy: &PolicySnapshot) -> Result<String> {
    Ok(format!(
        "[corral-policy]\n\n## Effective operating policy replacement\n\n\
         This replaces conflicting older startup, consent and review-switch instructions. The JSON \
         below is what Corral's executor may do without you: only its finite scope and current \
         per-task permissions delegate automatic work, and its refusals, pause, unresolved \
         decisions, required gates and independent review bind the executor. A policy wake is not \
         a reason to restart workers or to replay an old command. Read fresh brief and next (Pi: \
         corral_brief and corral_next; other harnesses: `corral pm brief --json` and \
         `corral pm next --json`); recompute after each action, continuing independent work \
         despite pending questions.\n\n{PM_AUTHORITY}\n```json\n{}\n```\n",
        serde_json::to_string_pretty(policy)?
    ))
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Disposition {
    Confirmed,
    Deferred,
    NotSubmitted,
    Submitting,
    Uncertain,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Record {
    snapshot: PolicySnapshot,
    fingerprint: String,
    disposition: Disposition,
    delivery: Option<Delivery>,
    error: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
struct Records {
    panes: BTreeMap<String, Record>,
}

const STATE: RecordArea<Records> = RecordArea::new("pm-policy");

fn fingerprint(policy: &PolicySnapshot) -> Result<String> {
    // Per-task permissions, refusals and diagnostic reasons are live observations,
    // not a replacement of the owner's material policy or finite delegation.
    Ok(crate::verification::digest(&serde_json::to_vec(&(
        policy.mode,
        policy.paused,
        &policy.revision,
        &policy.authorization,
        &policy.scope,
        &policy.authority,
    ))?))
}

fn load(ctx: &Ctx) -> Result<Records> {
    let mut records = STATE.load(&ctx.store, "state")?.unwrap_or_default();
    // Re-key saved evidence without writing during health reads or waking a live PM.
    for record in records.panes.values_mut() {
        record.fingerprint = fingerprint(&record.snapshot)?;
    }
    Ok(records)
}

pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("PM state path has no parent")?;
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let tmp = path.with_extension("tmp");
    let mut file =
        std::fs::File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("writing {}", tmp.display()))?;
    file.sync_all()
        .with_context(|| format!("syncing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("publishing {}", path.display()))?;
    std::fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .with_context(|| format!("syncing {}", parent.display()))
}

fn save(ctx: &Ctx, records: &Records) -> Result<()> {
    STATE.save(&ctx.store, "state", records)
}

/// Begin before startup IPC; a crash must not turn a reused shell into a second launch.
/// Caller holds pm-policy and dispatch locks throughout startup submission.
pub(crate) fn begin_startup(ctx: &Ctx, pane: &str, policy: &PolicySnapshot) -> Result<()> {
    let mut records = load(ctx)?;
    records.panes.insert(
        pane.into(),
        Record {
            snapshot: policy.clone(),
            fingerprint: fingerprint(policy)?,
            disposition: Disposition::Submitting,
            delivery: None,
            error: None,
        },
    );
    save(ctx, &records)
}

pub(crate) fn confirm_startup(ctx: &Ctx, pane: &str) -> Result<()> {
    let mut records = load(ctx)?;
    let record = records
        .panes
        .get_mut(pane)
        .context("missing PM startup intent")?;
    ensure!(
        record.disposition == Disposition::Submitting,
        "PM startup intent changed"
    );
    record.disposition = Disposition::Confirmed;
    save(ctx, &records)
}

/// Caller holds pm-policy and dispatch locks; unchanged delivery preserves startup context.
pub(crate) fn deliver(ctx: &Ctx, pane: &str, policy: &PolicySnapshot) -> Result<()> {
    let mut records = load(ctx)?;
    let key = fingerprint(policy)?;
    if let Some(record) = records.panes.get(pane) {
        match record.disposition {
            // Uncertainty belongs to the pane, even after another policy revision.
            Disposition::Submitting | Disposition::Uncertain | Disposition::NotSubmitted => {
                return Ok(());
            }
            Disposition::Confirmed if record.fingerprint == key => return Ok(()),
            Disposition::Confirmed | Disposition::Deferred => {}
        }
    }
    crate::pm::save_prompt(ctx, policy)?;
    let mut record = Record {
        snapshot: policy.clone(),
        fingerprint: key,
        disposition: Disposition::Submitting,
        delivery: None,
        error: None,
    };
    // Some callers already hold dispatch: never wait for delivery -> dispatch in reverse.
    let inbox = corral_core::inbox::Inbox::new(&ctx.store, corral_core::inbox::Recipient::Pm)?;
    let delivery = ctx.store.try_lock(&format!("deliver-{}", inbox.key()))?;
    if delivery.is_none()
        || crate::session_control::automated_input_must_wait(
            ctx,
            &corral_core::inbox::Recipient::Pm,
        )?
    {
        record.disposition = Disposition::Deferred;
        records.panes.insert(pane.into(), record);
        return save(ctx, &records);
    }
    records.panes.insert(pane.into(), record.clone());
    save(ctx, &records)?;
    let text = instructions(policy)?;
    match ctx.herdr.agent_prompt(pane, &text) {
        Ok(delivery) => {
            record.disposition = if delivery.submitted {
                Disposition::Confirmed
            } else {
                // Zero Enter attempts do not prove that nothing was pasted or
                // submitted. A collapsed composer or lost IPC response is opaque.
                Disposition::Uncertain
            };
            record.error.clone_from(&delivery.error);
            record.delivery = Some(delivery);
        }
        Err(error) => {
            record.disposition = Disposition::Uncertain;
            record.error = Some(format!("{error:#}"));
        }
    }
    records.panes.insert(pane.into(), record);
    save(ctx, &records)
}

/// Owning sync calls this independently of push_events and before optional event delivery.
/// Unavailable transport keeps all delivery evidence; it never checkpoints policy away.
pub fn reconcile_pm_policy(ctx: &Ctx, herdr_available: bool) -> Result<()> {
    let _span = corral_core::timing::span("sync.reconcile_pm_policy");
    let Some(_delivery) = ctx.store.try_lock("pm-policy")? else {
        return Ok(());
    };
    let Some(_board) = ctx.store.try_lock("dispatch")? else {
        return Ok(());
    };
    let policy = snapshot(ctx)?;
    crate::pm::save_prompt(ctx, &policy)?;
    if herdr_available && let Some(pane) = crate::pm::live_pm_pane(ctx)? {
        deliver(ctx, &pane, &policy)?;
    }
    Ok(())
}

/// Read-only and transport-free: brief can report failures without causing another delivery.
pub fn pm_policy_health(ctx: &Ctx) -> Result<Value> {
    let Some(_board) = ctx.store.try_lock("dispatch")? else {
        anyhow::bail!("operating policy is being updated");
    };
    let policy = snapshot(ctx)?;
    let current = fingerprint(&policy)?;
    let records = load(ctx)?;
    let panes: BTreeMap<_, _> = records
        .panes
        .iter()
        .map(|(pane, record)| {
            (
                pane,
                json!({
                    "current": record.fingerprint == current
                        && record.disposition == Disposition::Confirmed,
                    "uncertain": matches!(
                        record.disposition,
                        Disposition::Submitting | Disposition::Uncertain | Disposition::NotSubmitted
                    ),
                    "record": record,
                }),
            )
        })
        .collect();
    Ok(json!({"policy": policy, "fingerprint": current, "panes": panes}))
}
