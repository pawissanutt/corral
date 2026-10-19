//! Policy replacement is mandatory coordination, not an optional board-event push.
use std::collections::{BTreeMap, BTreeSet};
use std::io::{ErrorKind, Write};
use std::path::Path;

use anyhow::{Context, Result, ensure};
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

pub(crate) fn instructions(policy: &PolicySnapshot) -> Result<String> {
    Ok(format!(
        "[corral-policy]\n\n## Effective operating policy replacement\n\n\
         This replaces conflicting older startup, consent and review-switch instructions. \
         Only the finite scope and current per-task permissions below delegate automatic work. \
         Scope labels and configured authority never override refusals, pause, owner exceptions, \
         unresolved decisions, required gates or independent review. Assist requires explicit \
         owner action for starts and acceptance. Autopilot does not require renewed routine \
         consent for currently delegated actions. Never enlarge scope, clear a hold, waive a \
         check, act as the independent reviewer, or restart workers on a policy wake. \
         Read `corral pm brief --json` and `corral pm next --json` for fresh state; recompute \
         after each authorized action, continuing independent work despite pending questions. \
         Never replay an old command from this update.\n\n```json\n{}\n```\n",
        serde_json::to_string_pretty(policy)?
    ))
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Disposition {
    Confirmed,
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
    let path = ctx.store.state_dir().join("pm-policy.json");
    match std::fs::read(&path) {
        Ok(bytes) => {
            let mut records: Records = serde_json::from_slice(&bytes)
                .with_context(|| format!("parsing {}", path.display()))?;
            // Older receipts hashed the whole snapshot. Re-key from the saved
            // evidence without writing during health reads or waking a live PM.
            for record in records.panes.values_mut() {
                record.fingerprint = fingerprint(&record.snapshot)?;
            }
            Ok(records)
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(Records::default()),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
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
    write_atomic(
        &ctx.store.state_dir().join("pm-policy.json"),
        &serde_json::to_vec(records)?,
    )
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
            Disposition::Confirmed => {}
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
