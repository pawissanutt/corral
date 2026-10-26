use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};
use corral_core::Task;
use corral_core::attention::{
    AttentionHandling, AttentionLedger, AttentionRecord, Condition, ConditionKey,
    MAX_ATTENTION_NOTE_BYTES, NoticeCoverage, Responsible, SinceSource,
};
use jiff::{SignedDuration, Timestamp};
use serde_json::json;

use super::outbox::{self, Wakes};
use crate::{Ctx, owner_notice};

/// Reconcile from one fresh post-mutation listing; no event-history reads or transport retries.
pub fn reconcile(ctx: &Ctx, now: Timestamp, pm_live: Option<bool>) -> Result<()> {
    let Some(_lock) = ctx.store.try_lock("attention-reconcile")? else {
        return Ok(());
    };
    let (tasks, board) = super::board_snapshot(ctx, now)?;
    owner_notice::drain_snapshot(ctx, &tasks)?;
    let mut wakes = Wakes::new();
    let mut published = BTreeMap::new();
    for (snapshot, assessment) in tasks.iter().zip(board.tasks) {
        if assessment.assessment.conditions.is_empty() && snapshot.attention.is_empty() {
            continue;
        }
        let mut task = if snapshot.attention.notice_outbox.is_empty()
            && snapshot.attention.source_notices.is_empty()
        {
            snapshot.clone()
        } else {
            ctx.store.get(&snapshot.id)?
        };
        if !crate::session_scope::owns_task(ctx, &task)? {
            continue;
        }
        // Retained facts publish even if the condition or task is already terminal.
        if !task.attention.outbox.is_empty() {
            outbox::publish(ctx, &task.id, &mut wakes)?;
            task = ctx.store.get(&task.id)?;
        }
        let mut ledger = task.attention.clone();
        let old_ids: Vec<_> = ledger
            .outbox
            .iter()
            .map(|fact| fact.fact_id.clone())
            .collect();
        let claim = transition(
            ctx,
            &task,
            &mut ledger,
            &assessment.assessment.conditions,
            now,
            pm_live,
        )?;
        let pending = claim
            .as_ref()
            .map(|claim| owner_notice::reserve_task(ctx, &task.id, claim.batch_id.clone()))
            .transpose()?;
        let new_ids: Vec<_> = ledger
            .outbox
            .iter()
            .map(|fact| fact.fact_id.clone())
            .collect();
        if ledger != task.attention {
            let expected = task.attention.clone();
            ctx.store.update_attention(&task.id, |current| {
                ensure!(
                    *current == expected,
                    "attention_changed: retry on a fresh snapshot"
                );
                *current = ledger;
                Ok(())
            })?;
            #[cfg(test)]
            crash_at(CrashPoint::AfterTransition)?;
        }
        if !new_ids.is_empty() {
            // Publish only newly committed facts here; prior facts were already appended above.
            publish_new(ctx, &task.id, &old_ids, &mut wakes)?;
        }
        published.insert(
            task.id.clone(),
            old_ids.into_iter().chain(new_ids).collect(),
        );
        if let Some(pending) = pending {
            owner_notice::notify(
                ctx,
                owner_notice::newly_committed(&ctx.store.get(&task.id)?, pending)?,
                &format!("{} attention overdue", task.id),
                &format!("Inspect corral attention --json for {}", task.id),
                true,
            )?;
        }
    }
    outbox::finish(ctx, &published, wakes, pm_live, now)
}

fn publish_new(ctx: &Ctx, id: &str, old: &[String], wakes: &mut Wakes) -> Result<()> {
    // Publishing the complete outbox is safe: deterministic IDs make the old facts replayable.
    // Avoid a second append of old facts in the ordinary path by selecting their complement.
    let task = ctx.store.get(id)?;
    for fact in task
        .attention
        .outbox
        .iter()
        .filter(|fact| !old.contains(&fact.fact_id))
    {
        let mut detail = fact.detail.clone();
        detail["fact_id"] = json!(fact.fact_id);
        ctx.store.log_event(Some(id), &fact.kind, detail)?;
        #[cfg(test)]
        crash_at(CrashPoint::AfterAppend)?;
        if fact.detail["wake"] == true {
            wakes.entry(task.project.clone()).or_default().insert(
                fact.fact_id.clone(),
                json!({
                    "key": fact.key, "task": id, "class": fact.detail["class"],
                    "reason": fact.detail["reason"], "fact_id": fact.fact_id,
                }),
            );
        }
    }
    #[cfg(test)]
    crash_at(CrashPoint::BeforeClear)?;
    Ok(())
}

fn transition(
    ctx: &Ctx,
    task: &Task,
    ledger: &mut AttentionLedger,
    conditions: &[Condition],
    now: Timestamp,
    pm_live: Option<bool>,
) -> Result<Option<corral_core::NoticeClaim>> {
    let overflow = ConditionKey(format!("{}/ledger_overflow/v1", task.id));
    let current: BTreeMap<_, _> = conditions
        .iter()
        .filter_map(|c| c.key.as_ref().map(|key| (key.clone(), c)))
        .collect();
    for key in ledger.records.keys().cloned().collect::<Vec<_>>() {
        if key == overflow
            || current.contains_key(&key)
            || ledger
                .records
                .get(&key)
                .is_some_and(|record| record.resolved_at.is_some())
        {
            continue;
        }
        let record = ledger
            .records
            .get_mut(&key)
            .context("attention record disappeared")?;
        record.resolved_at = Some(now);
        let detail = json!({"key": key, "resolved_at": now, "record": record,
            "kind": record.kind, "responsible": record.responsible, "since": record.since,
            "since_source": record.since_source, "opened_at": record.opened_at,
            "pm_live_at_open": record.pm_live_at_open, "first_handled_at": record.first_handled_at,
            "levels": record.levels});
        outbox::enqueue(ledger, &key, "attention_resolved", "", detail)?;
    }
    let mut uncovered = Vec::new();
    for (key, condition) in &current {
        let opening = !ledger.records.contains_key(key);
        if !ledger.records.contains_key(key) {
            if ledger
                .records
                .keys()
                .filter(|key| **key != overflow)
                .count()
                >= 31
            {
                continue;
            }
            let record = AttentionRecord {
                kind: condition.kind.clone(),
                responsible: condition.responsible,
                since: condition.since.unwrap_or(now),
                since_source: condition.since_source,
                first_recorded: now,
                opened_at: now,
                pm_live_at_open: pm_live,
                levels: corral_core::attention::AttentionLevels {
                    announced_at: (condition.responsible == Responsible::Pm).then_some(now),
                    ..Default::default()
                },
                handling: None,
                first_handled_at: None,
                resolved_at: None,
            };
            ledger.records.insert(key.clone(), record);
            outbox::enqueue(
                ledger,
                key,
                "attention_opened",
                "",
                json!({
                    "key": key, "kind": condition.kind, "class": condition.class,
                    "responsible": condition.responsible, "since": condition.since.unwrap_or(now),
                    "since_source": condition.since_source, "opened_at": now,
                    "pm_live": pm_live, "reason": condition.reason,
                    "wake": condition.responsible == Responsible::Pm,
                }),
            )?;
        }
        let record = ledger
            .records
            .get_mut(key)
            .context("new attention record missing")?;
        if let Some(batch) = owner_notice::covering_claim(task, condition) {
            record.levels.owner_claim = Some(batch.clone());
            if opening
                && let Some(fact) = ledger
                    .outbox
                    .iter_mut()
                    .find(|fact| fact.key == *key && fact.kind == "attention_opened")
            {
                fact.detail["owner_claim"] = json!(batch);
            }
        }
        let paused = record
            .handling
            .as_ref()
            .is_some_and(|handling| now < handling.next_check);
        if paused {
            continue;
        }
        let age = now.duration_since(record.since).as_secs().max(0);
        let mut facts = Vec::new();
        if !opening
            && record.responsible == Responsible::Pm
            && record.levels.rewoke_at.is_none()
            && age >= i64::try_from(ctx.store.config.pm.attention_secs)?
            && pm_live != Some(false)
        {
            record.levels.rewoke_at = Some(now);
            facts.push(("rewake", true));
        }
        if matches!(record.responsible, Responsible::Pm | Responsible::Owner)
            && record.levels.owner_claimed_at.is_none()
            && record.levels.owner_claim.is_none()
            && age >= i64::try_from(ctx.store.config.pm.attention_owner_secs)?
        {
            record.levels.owner_claimed_at = Some(now);
            uncovered.push(key.clone());
            facts.push(("owner", false));
        }
        for (level, wake) in facts {
            outbox::enqueue(
                ledger,
                key,
                "attention_overdue",
                &format!("/{level}"),
                json!({
                    "key": key, "kind": condition.kind, "class": condition.class,
                    "responsible": condition.responsible, "level": level, "age_secs": age,
                    "pm_live": pm_live, "wake": wake, "reason": condition.reason, "at": now,
                }),
            )?;
        }
    }
    let missing = current
        .keys()
        .filter(|key| **key != overflow && !ledger.records.contains_key(*key))
        .count();
    if missing > 0 {
        ledger.overflow_count = Some(missing);
        if !ledger.records.contains_key(&overflow) {
            ledger.records.insert(
                overflow.clone(),
                AttentionRecord {
                    kind: "ledger_overflow".into(),
                    responsible: Responsible::Pm,
                    since: now,
                    since_source: SinceSource::Discovered,
                    first_recorded: now,
                    opened_at: now,
                    pm_live_at_open: pm_live,
                    levels: corral_core::attention::AttentionLevels {
                        announced_at: Some(now),
                        ..Default::default()
                    },
                    handling: None,
                    first_handled_at: None,
                    resolved_at: None,
                },
            );
            outbox::enqueue(
                ledger,
                &overflow,
                "attention_opened",
                "",
                json!({
                    "key": overflow, "kind": "ledger_overflow", "responsible": "pm",
                    "since": now, "since_source": "discovered", "pm_live": pm_live, "count": missing,
                    "opened_at": now, "class": corral_core::attention::WaitClass::PmAction,
                    "reason": format!("{missing} conditions are listed without ladder records; resolve retained conditions to free capacity"),
                    "wake": true,
                }),
            )?;
        }
    } else if let Some(record) = ledger.records.get_mut(&overflow)
        && record.resolved_at.is_none()
    {
        ledger.overflow_count = None;
        record.resolved_at = Some(now);
        let detail = json!({"key": overflow, "record": record, "resolved_at": now});
        outbox::enqueue(ledger, &overflow, "attention_resolved", "", detail)?;
    }
    if uncovered.is_empty() {
        return Ok(None);
    }
    let source = crate::verification::digest(&serde_json::to_vec(&uncovered)?);
    let mut claim = owner_notice::prepare(
        ctx,
        task,
        "attention_ladder",
        &source,
        now,
        uncovered
            .iter()
            .map(|key| NoticeCoverage::Exact {
                condition_key: key.0.clone(),
            })
            .collect(),
    )?;
    claim.claim_ids = uncovered
        .iter()
        .map(|key| crate::verification::digest(format!("{key}/owner").as_bytes()))
        .collect();
    owner_notice::refresh_fact(&mut claim)?;
    for key in uncovered {
        ledger
            .records
            .get_mut(&key)
            .context("owner record missing")?
            .levels
            .owner_claim = Some(claim.batch_id.clone());
        for fact in ledger.outbox.iter_mut().filter(|fact| fact.key == key) {
            fact.detail["owner_claim"] = json!(claim.batch_id);
        }
    }
    ledger.push_notice(claim.clone())?;
    Ok(Some(claim))
}

/// Context guard is shared by the CLI and operation; handling never grants execution authority.
pub fn guard(ctx: &Ctx) -> Result<()> {
    let managed = [
        "AI_AGENT",
        "PI_CODING_AGENT",
        "CORRAL_AGENT",
        "CORRAL_REVIEWER",
        "CORRAL_AGENT_TASK",
    ]
    .iter()
    .any(|key| std::env::var_os(key).is_some());
    ensure!(
        !managed || std::env::var("CORRAL_PM").as_deref() == Ok("1"),
        "worker_context: attention handling is PM/owner only"
    );
    let cwd = std::env::current_dir()
        .context("reading attention caller cwd")?
        .canonicalize()
        .context("resolving attention caller cwd")?;
    ensure!(
        !ctx.store
            .list_shared()?
            .iter()
            .flat_map(|task| &task.runs)
            .any(|run| run
                .worktree
                .canonicalize()
                .is_ok_and(|worktree| cwd.starts_with(worktree))),
        "run_worktree: attention handling is refused inside a run worktree"
    );
    Ok(())
}

pub fn attend(
    ctx: &Ctx,
    key: &str,
    note: &str,
    next_check_secs: u64,
    request: &str,
    now: Timestamp,
) -> Result<AttentionHandling> {
    guard(ctx)?;
    attend_authorized(ctx, key, note, next_check_secs, request, now)
}

pub(super) fn attend_authorized(
    ctx: &Ctx,
    key: &str,
    note: &str,
    next_check_secs: u64,
    request: &str,
    now: Timestamp,
) -> Result<AttentionHandling> {
    ensure!(
        !note.trim().is_empty() && note.len() <= MAX_ATTENTION_NOTE_BYTES,
        "note must contain 1..1024 bytes"
    );
    ensure!(
        !request.is_empty() && request.len() <= 128,
        "request must contain 1..128 bytes"
    );
    ensure!(
        (60..=86400).contains(&next_check_secs),
        "next_check must be 1m..24h"
    );
    let key = ConditionKey(key.into());
    let id = key.0.split('/').next().context("invalid attention key")?;
    let Some(_lock) = ctx.store.try_lock("attention-reconcile")? else {
        anyhow::bail!("attention_busy");
    };
    let board = super::board(ctx, now)?;
    let assessment = board
        .tasks
        .iter()
        .find(|task| task.task == id)
        .context("attention task not found")?;
    ensure!(
        assessment
            .assessment
            .conditions
            .iter()
            .any(|condition| condition.key.as_ref() == Some(&key)),
        "stale_item: {id} current keys: {:?}",
        assessment
            .assessment
            .conditions
            .iter()
            .filter_map(|c| c.key.as_ref())
            .collect::<Vec<_>>()
    );
    let condition = assessment
        .assessment
        .conditions
        .iter()
        .find(|condition| condition.key.as_ref() == Some(&key))
        .context("current attention condition disappeared")?;
    // Older revisions live in the retained lifecycle facts, not an ever-growing request
    // vector on every task. This explicit command may read history; sync never does.
    let task = ctx.store.get(id)?;
    let latest = task
        .attention
        .records
        .get(&key)
        .and_then(|record| record.handling.as_ref());
    let mut prior = latest
        .filter(|handling| handling.request == request)
        .cloned();
    if prior.is_none() {
        prior = task
            .attention
            .outbox
            .iter()
            .find(|fact| {
                fact.key == key
                    && fact.kind == "attention_handled"
                    && fact.detail["handling"]["request"] == request
            })
            .map(|fact| {
                serde_json::from_value::<AttentionHandling>(fact.detail["handling"].clone())
            })
            .transpose()?;
    }
    if prior.is_none() && latest.is_some() {
        for event in ctx.store.events()? {
            if event.kind != "attention_handled"
                || event.detail["key"] != key.0
                || event.detail["handling"]["request"] != request
            {
                continue;
            }
            prior = Some(serde_json::from_value::<AttentionHandling>(
                event.detail["handling"].clone(),
            )?);
            break;
        }
    }
    if let Some(prior) = prior {
        ensure!(
            prior.note == note
                && prior.next_check.duration_since(prior.at).as_secs()
                    == i64::try_from(next_check_secs)?,
            "request_payload_mismatch"
        );
        return Ok(prior);
    }
    let mut wakes = Wakes::new();
    outbox::publish(ctx, id, &mut wakes)?;
    let old_ids: Vec<_> = ctx
        .store
        .get(id)?
        .attention
        .outbox
        .iter()
        .map(|fact| fact.fact_id.clone())
        .collect();
    let mut accepted = None;
    ctx.store.update_attention(id, |ledger| {
        if !ledger.records.contains_key(&key) {
            ensure!(
                ledger.records.len() < 31,
                "attention_ledger_full: resolve retained conditions first"
            );
            ledger.records.insert(
                key.clone(),
                AttentionRecord {
                    kind: condition.kind.clone(),
                    responsible: condition.responsible,
                    since: condition.since.unwrap_or(now),
                    since_source: condition.since_source,
                    first_recorded: now,
                    opened_at: now,
                    pm_live_at_open: None,
                    levels: corral_core::attention::AttentionLevels {
                        announced_at: (condition.responsible == Responsible::Pm).then_some(now),
                        ..Default::default()
                    },
                    handling: None,
                    first_handled_at: None,
                    resolved_at: None,
                },
            );
            outbox::enqueue(
                ledger,
                &key,
                "attention_opened",
                "",
                json!({
                    "key": key, "kind": condition.kind, "responsible": condition.responsible,
                    "since": condition.since.unwrap_or(now), "since_source": condition.since_source,
                    "opened_at": now, "pm_live": null,
                    "class": condition.class, "reason": condition.reason,
                    "wake": condition.responsible == Responsible::Pm,
                }),
            )?;
        }
        let record = ledger
            .records
            .get_mut(&key)
            .context("attention_unrecorded: reconcile before handling this key")?;
        let handling = AttentionHandling {
            revision: record.handling.as_ref().map_or(Ok(1), |h| {
                h.revision
                    .checked_add(1)
                    .context("handling revision overflow")
            })?,
            request: request.into(),
            note: note.into(),
            at: now,
            next_check: now
                .checked_add(SignedDuration::from_secs(i64::try_from(next_check_secs)?))?,
            by: if std::env::var("CORRAL_PM").as_deref() == Ok("1") {
                Responsible::Pm
            } else {
                Responsible::Owner
            },
        };
        record.first_handled_at.get_or_insert(now);
        record.handling = Some(handling.clone());
        outbox::enqueue(
            ledger,
            &key,
            "attention_handled",
            &format!("/{}", handling.revision),
            json!({"key": key, "handling": handling}),
        )?;
        accepted = Some(handling);
        Ok(())
    })?;
    #[cfg(test)]
    crash_at(CrashPoint::AfterTransition)?;
    publish_new(ctx, id, &old_ids, &mut wakes)?;
    let facts = ctx
        .store
        .get(id)?
        .attention
        .outbox
        .iter()
        .map(|fact| fact.fact_id.clone())
        .collect();
    outbox::finish(ctx, &BTreeMap::from([(id.into(), facts)]), wakes, None, now)?;
    accepted.context("attention handling was not recorded")
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CrashPoint {
    AfterTransition,
    AfterAppend,
    BeforeClear,
}
#[cfg(test)]
thread_local! {
    pub(super) static CRASH: std::cell::Cell<Option<CrashPoint>> = const { std::cell::Cell::new(None) };
}
#[cfg(test)]
pub(super) fn crash_at(point: CrashPoint) -> Result<()> {
    if CRASH.with(|crash| {
        crash.get() == Some(point) && {
            crash.set(None);
            true
        }
    }) {
        anyhow::bail!("injected attention crash: {point:?}");
    }
    Ok(())
}
