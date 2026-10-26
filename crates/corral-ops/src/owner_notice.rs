//! Durable claim-before-send owner notifications. Publication can replay; transport cannot.

use anyhow::{Context, Result};
use corral_core::attention::{
    Condition, EntryNotice, NoticeClaim, NoticeCoverage, NoticeFact, NoticeOutcome, SourceNoticeRef,
};
use corral_core::{Store, Task};
use jiff::Timestamp;
use serde_json::json;

use crate::Ctx;

/// A send capability returned only after this invocation committed a new source claim.
/// Deliberately not Clone: a replaying outbox reader never obtains this capability.
pub(crate) struct NewlyCommittedClaim {
    task: String,
    claim: NoticeClaim,
    source: Option<(SourceNoticeRef, corral_core::StoreLock)>,
    lease: Option<std::sync::Arc<corral_core::StoreLock>>,
}

pub(crate) struct PendingNotice {
    pub batch_id: String,
    lease: std::sync::Arc<corral_core::StoreLock>,
}

impl PendingNotice {
    pub(crate) fn sibling(&self, batch_id: String) -> Self {
        Self {
            batch_id,
            lease: self.lease.clone(),
        }
    }
}
pub(crate) fn reserve_task(ctx: &Ctx, id: &str, batch_id: String) -> Result<PendingNotice> {
    let lease = ctx
        .store
        .try_lock(&format!("owner-notice-{id}"))?
        .context("notification_accounting_backpressure: task notice publication is in flight")?;
    Ok(PendingNotice {
        batch_id,
        lease: std::sync::Arc::new(lease),
    })
}

pub(crate) struct SourceRegistration {
    reference: SourceNoticeRef,
    _lease: corral_core::StoreLock,
}

/// Constructed only in the invocation whose source transaction returned a new claim.
pub(crate) fn newly_committed(task: &Task, pending: PendingNotice) -> Result<NewlyCommittedClaim> {
    Ok(NewlyCommittedClaim {
        task: task.id.clone(),
        source: None,
        lease: Some(pending.lease),
        claim: task
            .attention
            .notice_outbox
            .iter()
            .find(|claim| claim.batch_id == pending.batch_id)
            .context("new claim missing from saved task")?
            .clone(),
    })
}

pub(crate) fn record_entry(
    ctx: &Ctx,
    task: &mut Task,
    producer: &str,
    now: Timestamp,
    exact: Vec<String>,
) -> Result<PendingNotice> {
    let source = crate::verification::digest(&serde_json::to_vec(&(
        task.current_run().map(|run| run.id.as_str()),
        task.status_since,
        now,
    ))?);
    let mut coverage = vec![NoticeCoverage::Episode {
        task: task.id.clone(),
        run: task.current_run().map(|run| run.id.clone()),
        stage: "needs_me".into(),
        at: now,
    }];
    coverage.extend(
        exact
            .into_iter()
            .map(|condition_key| NoticeCoverage::Exact { condition_key }),
    );
    let mut claim = prepare(ctx, task, producer, &source, now, coverage)?;
    // Responsibility is determined from the committed entry, never from transport success.
    let pm_entry = task
        .current_run()
        .is_some_and(|run| run.agent_state.as_deref() == Some("gone") || run.idle_no_report)
        || producer == "native_review_host_lost";
    if !pm_entry {
        claim.exemption_basis = Some("owner_responsible_entry".into());
    }
    refresh_fact(&mut claim)?;
    let batch = claim.batch_id.clone();
    let pending = reserve_task(ctx, &task.id, batch.clone())?;
    task.attention.entry_notice = Some(EntryNotice {
        batch_id: batch.clone(),
        at: now,
        status_since: None,
    });
    task.attention.push_notice(claim)?;
    Ok(pending)
}

pub(crate) fn record_stage(
    ctx: &Ctx,
    task: &mut Task,
    producer: &str,
    now: Timestamp,
) -> Result<PendingNotice> {
    let run = task.current_run().context("stage notice has no run")?;
    let stage = run
        .autopilot
        .as_ref()
        .context("stage notice has no stage")?;
    let source = format!("{}/{}/{now}", run.id, stage.stage.as_str());
    let claim = prepare(
        ctx,
        task,
        producer,
        &source,
        now,
        vec![NoticeCoverage::Episode {
            task: task.id.clone(),
            run: Some(run.id.clone()),
            stage: stage.stage.as_str().into(),
            at: stage.at,
        }],
    )?;
    let batch = claim.batch_id.clone();
    let pending = reserve_task(ctx, &task.id, batch)?;
    task.attention.push_notice(claim)?;
    Ok(pending)
}

pub(crate) fn refresh_fact(claim: &mut NoticeClaim) -> Result<()> {
    anyhow::ensure!(
        claim.pending.len() <= 1,
        "cannot refresh a published notice outcome"
    );
    claim.pending = vec![fact(claim, "notification_claimed")];
    Ok(())
}

pub(crate) fn covering_claim(task: &Task, condition: &Condition) -> Option<String> {
    let key = condition.key.as_ref()?;
    if let Some(record) = task.attention.records.get(key)
        && let Some(batch) = &record.levels.owner_claim
    {
        return Some(batch.clone());
    }
    for claim in &task.attention.notice_outbox {
        if claim.coverage.iter().any(|coverage| match coverage {
            NoticeCoverage::Exact { condition_key } => *condition_key == key.0,
            NoticeCoverage::Episode {
                task: id,
                run,
                stage,
                at,
            } => {
                if *id != task.id {
                    return false;
                }
                if stage == "needs_me" {
                    return task.attention.entry_notice.as_ref().is_some_and(|entry| {
                        entry.batch_id == claim.batch_id
                            && entry.status_since == task.status_since
                            && condition.since.is_some_and(|since| since <= entry.at)
                            && matches!(
                                condition.kind.as_str(),
                                "needs_me_other"
                                    | "agent_lost"
                                    | "silent_turn"
                                    | "decision_pending"
                            )
                    });
                }
                task.current_run().is_some_and(|current| {
                    run.as_deref() == Some(current.id.as_str())
                        && current
                            .autopilot
                            .as_ref()
                            .is_some_and(|pilot| pilot.stage.as_str() == stage && pilot.at == *at)
                        && matches!(
                            condition.kind.as_str(),
                            "escalation" | "infra_escalation" | "landing_blocked"
                        )
                })
            }
        }) {
            return Some(claim.batch_id.clone());
        }
    }
    // Older source markers predate attribution, but still prove a send was claimed.
    if matches!(
        condition.kind.as_str(),
        "escalation" | "infra_escalation" | "landing_blocked"
    ) && task
        .current_run()
        .and_then(|run| run.autopilot.as_ref())
        .is_some_and(|pilot| {
            matches!(
                pilot.stage,
                corral_core::AutoStage::Escalated | corral_core::AutoStage::Blocked
            )
        })
    {
        return Some(format!("legacy_unknown/{key}"));
    }
    if (condition.kind == "stuck_process"
        && task.current_run().is_some_and(|run| run.stuck.is_some()))
        || (condition.kind == "infra_hold"
            && task
                .infra_wait
                .as_ref()
                .is_some_and(|wait| wait.held_at.is_some()))
    {
        return Some(format!("legacy_unknown/{key}"));
    }
    None
}

pub(crate) fn prepare(
    ctx: &Ctx,
    task: &Task,
    producer: &str,
    source: &str,
    at: Timestamp,
    coverage: Vec<NoticeCoverage>,
) -> Result<NoticeClaim> {
    prepare_subject(ctx, Some(&task.id), producer, source, at, coverage)
}

pub(crate) fn prepare_subject(
    ctx: &Ctx,
    task: Option<&str>,
    producer: &str,
    source: &str,
    at: Timestamp,
    coverage: Vec<NoticeCoverage>,
) -> Result<NoticeClaim> {
    let claim_id = crate::verification::digest(&serde_json::to_vec(&(
        1,
        ctx.store.root(),
        task,
        producer,
        source,
    ))?);
    let batch_id = claim_id.clone();
    let mut claim = NoticeClaim {
        version: 1,
        batch_id,
        producer: producer.into(),
        source: source.into(),
        claimed_at: at,
        claim_ids: vec![claim_id],
        coverage,
        exemption_basis: None,
        outcome: None,
        pending: Vec::new(),
    };
    claim.pending.push(fact(&claim, "notification_claimed"));
    Ok(claim)
}

/// Commit a source transition and its claim together; return authority only to this caller.
pub(crate) fn commit(
    ctx: &Ctx,
    id: &str,
    change: impl FnOnce(&Store, &mut Task) -> Result<Option<NoticeClaim>>,
) -> Result<Option<NewlyCommittedClaim>> {
    let lease = ctx
        .store
        .try_lock(&format!("owner-notice-{id}"))?
        .context("notification_accounting_backpressure: task notice publication is in flight")?;
    let (_, claim) = ctx.store.dispatch_update(id, |store, task| {
        let Some(claim) = change(store, task)? else {
            return Ok(None);
        };
        task.attention.push_notice(claim.clone())?;
        Ok(Some(claim))
    })?;
    #[cfg(test)]
    crash_at(CrashPoint::AfterClaim)?;
    Ok(claim.map(|claim| NewlyCommittedClaim {
        task: id.into(),
        claim,
        source: None,
        lease: Some(std::sync::Arc::new(lease)),
    }))
}

/// Send once, only using a capability produced by the current source transition.
pub(crate) fn notify(
    ctx: &Ctx,
    newly_committed: NewlyCommittedClaim,
    title: &str,
    body: &str,
    urgent: bool,
) -> Result<()> {
    let NewlyCommittedClaim {
        task,
        claim,
        source,
        lease: _lease,
    } = newly_committed;
    if let Some((source, _lease)) = source {
        return notify_source(ctx, &task, &source, claim, title, body, urgent);
    }
    publish(ctx, &task, false)?;
    #[cfg(test)]
    crash_at(CrashPoint::BeforeNotify)?;
    let outcome = invoke(ctx, Some(&task), &claim, title, body, urgent);
    #[cfg(test)]
    crash_at(CrashPoint::AfterNotify)?;
    ctx.store.update_attention(&task, |ledger| {
        let stored = ledger
            .notice_outbox
            .iter_mut()
            .find(|stored| stored.batch_id == claim.batch_id)
            .context("committed owner notice disappeared before outcome save")?;
        save_outcome(stored, outcome);
        Ok(())
    })?;
    #[cfg(test)]
    crash_at(CrashPoint::AfterOutcomeSave)?;
    publish(ctx, &task, false)
}

fn invoke(
    ctx: &Ctx,
    task: Option<&str>,
    claim: &NoticeClaim,
    title: &str,
    body: &str,
    urgent: bool,
) -> NoticeOutcome {
    if claim.producer == "attention_ladder" && !ctx.store.config.pm.attention_notify {
        NoticeOutcome::NotInvoked {
            at: claim.claimed_at,
            reason: "disabled".into(),
        }
    } else {
        match ctx.herdr.notify(title, body, urgent) {
            Ok(()) => NoticeOutcome::Notified {
                at: claim.claimed_at,
            },
            Err(error) => {
                let error = format!("{error:#}");
                crate::messages::report(
                    task,
                    if matches!(
                        claim.producer.as_str(),
                        "obligation_transition" | "question_needs_me"
                    ) {
                        "obligations"
                    } else {
                        "notification"
                    },
                    format!("{} notification failed: {error}", claim.producer),
                );
                NoticeOutcome::Failed {
                    at: claim.claimed_at,
                    error,
                }
            }
        }
    }
}

fn save_outcome(stored: &mut NoticeClaim, outcome: NoticeOutcome) {
    let kind = match &outcome {
        NoticeOutcome::Notified { .. } => "owner_notified",
        NoticeOutcome::Failed { .. } => "owner_notify_failed",
        NoticeOutcome::NotInvoked { .. } => "notification_not_invoked",
    };
    stored.outcome = Some(outcome);
    stored.pending.push(fact(stored, kind));
}

/// Register before the standalone claim save. A ref is keyed discovery, not transport authority.
pub(crate) fn register_source(
    ctx: &Ctx,
    task: Option<&str>,
    area: &str,
    key: &str,
    claim: &NoticeClaim,
) -> Result<SourceRegistration> {
    let lease = ctx
        .store
        .try_lock(&format!("notice-source-{area}-{key}"))?
        .context("notification source transaction is in flight")?;
    let reference = SourceNoticeRef {
        area: area.into(),
        key: key.into(),
        claim_id: claim.batch_id.clone(),
    };
    if let Some(previous) = source_claim(ctx, &reference)?
        && previous.batch_id != reference.claim_id
    {
        publish_source(
            ctx,
            task,
            &SourceNoticeRef {
                area: area.into(),
                key: key.into(),
                claim_id: previous.batch_id,
            },
        )?;
    }
    if let Some(task) = task {
        ctx.store.update_attention(task, |ledger| {
            ledger
                .source_notices
                .retain(|old| old == &reference || old.area != area || old.key != key);
            if !ledger.source_notices.contains(&reference) {
                anyhow::ensure!(
                    ledger.source_notices.len() + ledger.notice_outbox.len() < 32,
                    "notification_accounting_backpressure: source refs full"
                );
                ledger.source_notices.push(reference.clone());
            }
            Ok(())
        })?;
    }
    #[cfg(test)]
    crash_at(CrashPoint::BeforeSourceClaim)?;
    Ok(SourceRegistration {
        reference,
        _lease: lease,
    })
}

pub(crate) fn source_committed(
    task: Option<&str>,
    registration: SourceRegistration,
    claim: NoticeClaim,
) -> Result<NewlyCommittedClaim> {
    #[cfg(test)]
    crash_at(CrashPoint::AfterClaim)?;
    Ok(NewlyCommittedClaim {
        task: task.unwrap_or("").into(),
        claim,
        source: Some((registration.reference, registration._lease)),
        lease: None,
    })
}

fn source_claim(ctx: &Ctx, source: &SourceNoticeRef) -> Result<Option<NoticeClaim>> {
    match source.area.as_str() {
        "availability" => crate::availability::notice(ctx, &source.key),
        "post-land" => crate::hooks::notice(ctx, &source.key),
        "resources" => crate::resource_escalation::notice(ctx),
        _ => anyhow::bail!("unknown notice source {}", source.area),
    }
}

fn update_source(
    ctx: &Ctx,
    source: &SourceNoticeRef,
    update: impl FnOnce(&mut NoticeClaim) -> Result<()>,
) -> Result<()> {
    match source.area.as_str() {
        "availability" => crate::availability::update_notice(ctx, &source.key, update),
        "post-land" => crate::hooks::update_notice(ctx, &source.key, update),
        "resources" => crate::resource_escalation::update_notice(ctx, update),
        _ => anyhow::bail!("unknown notice source {}", source.area),
    }
}

fn publish_source(ctx: &Ctx, task: Option<&str>, source: &SourceNoticeRef) -> Result<bool> {
    let Some(claim) = source_claim(ctx, source)? else {
        return Ok(false);
    };
    if claim.batch_id != source.claim_id {
        // The registered writer stopped before replacing this source's prior claim.
        return Ok(false);
    }
    for fact in &claim.pending {
        let mut detail = fact.detail.clone();
        detail["fact_id"] = json!(fact.fact_id);
        ctx.store.log_event(task, &fact.kind, detail)?;
        #[cfg(test)]
        crash_at(CrashPoint::AfterAppend)?;
    }
    #[cfg(test)]
    crash_at(CrashPoint::BeforeClear)?;
    if !claim.pending.is_empty() {
        update_source(ctx, source, |stored| {
            stored.pending.retain(|fact| {
                !claim
                    .pending
                    .iter()
                    .any(|sent| sent.fact_id == fact.fact_id)
            });
            Ok(())
        })?;
    }
    #[cfg(test)]
    crash_at(CrashPoint::AfterSourceClear)?;
    Ok(true)
}

fn notify_source(
    ctx: &Ctx,
    task: &str,
    source: &SourceNoticeRef,
    claim: NoticeClaim,
    title: &str,
    body: &str,
    urgent: bool,
) -> Result<()> {
    let task = (!task.is_empty()).then_some(task);
    publish_source(ctx, task, source)?;
    #[cfg(test)]
    crash_at(CrashPoint::BeforeNotify)?;
    let outcome = invoke(ctx, task, &claim, title, body, urgent);
    #[cfg(test)]
    crash_at(CrashPoint::AfterNotify)?;
    update_source(ctx, source, |stored| {
        save_outcome(stored, outcome);
        Ok(())
    })?;
    #[cfg(test)]
    crash_at(CrashPoint::AfterOutcomeSave)?;
    publish_source(ctx, task, source)?;
    Ok(())
}

pub(crate) fn drain_source(
    ctx: &Ctx,
    task: Option<&str>,
    source: &SourceNoticeRef,
) -> Result<bool> {
    let Some(_lock) = ctx
        .store
        .try_lock(&format!("notice-source-{}-{}", source.area, source.key))?
    else {
        return Ok(false);
    };
    publish_source(ctx, task, source)
}

fn fact(claim: &NoticeClaim, kind: &str) -> NoticeFact {
    NoticeFact {
        fact_id: format!(
            "{}/{}",
            claim.batch_id,
            if kind == "notification_claimed" {
                "claimed"
            } else {
                "outcome"
            }
        ),
        kind: kind.into(),
        detail: json!({
            "batch_id": claim.batch_id,
            "producer": claim.producer,
            "source": claim.source,
            "claimed_at": claim.claimed_at,
            "claim_ids": claim.claim_ids,
            "coverage": claim.coverage,
            "exemption_basis": claim.exemption_basis,
            "outcome": claim.outcome,
        }),
    }
}

pub(crate) fn drain_task(ctx: &Ctx, id: &str) -> Result<()> {
    // Serializes publication with a sender as well as concurrent sync passes.
    let Some(_publication) = ctx.store.try_lock(&format!("owner-notice-{id}"))? else {
        return Ok(());
    };
    publish(ctx, id, true)
}

fn publish(ctx: &Ctx, id: &str, retire: bool) -> Result<()> {
    let task = ctx.store.get(id)?;
    let facts: Vec<_> = task
        .attention
        .notice_outbox
        .iter()
        .flat_map(|claim| &claim.pending)
        .cloned()
        .collect();
    let ended: Vec<_> = task
        .attention
        .notice_outbox
        .iter()
        .filter(|claim| retire && source_ended(&task, claim))
        .map(|claim| claim.batch_id.clone())
        .collect();
    if facts.is_empty() && ended.is_empty() {
        return Ok(());
    }
    for fact in &facts {
        let mut detail = fact.detail.clone();
        detail["fact_id"] = json!(fact.fact_id);
        ctx.store.log_event(Some(id), &fact.kind, detail)?;
        #[cfg(test)]
        crash_at(CrashPoint::AfterAppend)?;
    }
    #[cfg(test)]
    crash_at(CrashPoint::BeforeClear)?;
    ctx.store.update_attention(id, |ledger| {
        for claim in &mut ledger.notice_outbox {
            claim
                .pending
                .retain(|fact| !facts.iter().any(|sent| sent.fact_id == fact.fact_id));
        }
        ledger
            .notice_outbox
            .retain(|claim| !claim.pending.is_empty() || !ended.contains(&claim.batch_id));
        Ok(())
    })?;
    Ok(())
}

fn source_ended(task: &Task, claim: &NoticeClaim) -> bool {
    match claim.producer.as_str() {
        "infra_hold" => !claim.coverage.iter().any(|coverage| match coverage {
            NoticeCoverage::Exact { condition_key } => {
                task.infra_wait.as_ref().is_some_and(|wait| {
                    *condition_key == format!("{}/infra_hold/{}", task.id, wait.first_seen)
                })
            }
            NoticeCoverage::Episode { .. } => false,
        }),
        "autopilot_escalate"
        | "landing_blocked"
        | "native_review_watchdog"
        | "legacy_review_fallback"
        | "legacy_review_watchdog" => !claim.coverage.iter().any(|coverage| match coverage {
            NoticeCoverage::Episode {
                task: id,
                run,
                stage,
                at,
            } => {
                *id == task.id
                    && task.current_run().is_some_and(|current| {
                        run.as_deref() == Some(current.id.as_str())
                            && current.autopilot.as_ref().is_some_and(|pilot| {
                                pilot.stage.as_str() == stage
                                    && pilot.at == *at
                                    && matches!(
                                        task.status,
                                        corral_core::Status::Review | corral_core::Status::NeedsMe
                                    )
                            })
                    })
            }
            NoticeCoverage::Exact { .. } => false,
        }),
        "needs_me_entry"
        | "provider_exhausted"
        | "availability_exhausted"
        | "native_review_host_lost"
        | "resource_escalation"
        | "question_needs_me" => task
            .attention
            .entry_notice
            .as_ref()
            .is_none_or(|entry| entry.batch_id != claim.batch_id),
        "attention_ladder" => !claim.coverage.iter().any(|coverage| match coverage {
            NoticeCoverage::Exact { condition_key } => task
                .attention
                .records
                .get(&corral_core::attention::ConditionKey(condition_key.clone()))
                .is_some_and(|record| record.resolved_at.is_none()),
            NoticeCoverage::Episode { .. } => false,
        }),
        "stuck" => !claim.coverage.iter().any(|coverage| match coverage {
            NoticeCoverage::Exact { condition_key } => task
                .current_run()
                .and_then(|run| run.stuck.as_ref())
                .is_some_and(|stuck| {
                    *condition_key
                        == format!(
                            "{}/stuck_process/{}:{}:{}",
                            task.id,
                            task.current_run().map_or("none", |run| run.id.as_str()),
                            stuck.pid,
                            stuck.started
                        )
                }),
            NoticeCoverage::Episode { .. } => false,
        }),
        "obligation_transition" => true,
        _ => false,
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CrashPoint {
    AfterClaim,
    AfterAppend,
    BeforeClear,
    BeforeNotify,
    AfterNotify,
    AfterOutcomeSave,
    BeforeSourceClaim,
    AfterSourceClear,
}

#[cfg(test)]
thread_local! {
    pub(crate) static CRASH: std::cell::Cell<Option<CrashPoint>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
fn crash_at(point: CrashPoint) -> Result<()> {
    if CRASH.with(|crash| {
        if crash.get() == Some(point) {
            crash.set(None);
            true
        } else {
            false
        }
    }) {
        anyhow::bail!("injected crash at {point:?}");
    }
    Ok(())
}

/// An owning sync drains a fresh post-mutation snapshot, including terminal tasks.
pub(crate) fn drain(ctx: &Ctx) -> Result<()> {
    for task in ctx.store.list_shared()?.iter() {
        if crate::session_scope::owns_task(ctx, task)? {
            for source in &task.attention.source_notices {
                let Some(_source_lock) = ctx
                    .store
                    .try_lock(&format!("notice-source-{}-{}", source.area, source.key))?
                else {
                    continue;
                };
                publish_source(ctx, Some(&task.id), source)?;
                // The creator holds this lease continuously from before registration through
                // send/outcome publication. Acquiring it proves that exact source transaction
                // settled; a worker's unrelated pane liveness is not evidence either way.
                ctx.store.update_attention(&task.id, |ledger| {
                    ledger
                        .source_notices
                        .retain(|reference| reference != source);
                    Ok(())
                })?;
            }
        }
        if !task.attention.notice_outbox.is_empty() && crate::session_scope::owns_task(ctx, task)? {
            drain_task(ctx, &task.id)?;
        }
    }
    Ok(())
}

#[cfg(all(test, unix))]
#[path = "../tests/attention_handling/review_entries.rs"]
mod review_entries;
#[cfg(all(test, unix))]
#[path = "../tests/attention_handling/owner_sources.rs"]
mod source_tests;
#[cfg(all(test, unix))]
#[path = "../tests/attention_handling/owner_notice.rs"]
mod tests;
