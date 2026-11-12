//! PM container retention. The container outlives every host generation, so its receipts and
//! generation records would otherwise fill their families for good. One pass runs, inside the
//! caller's PM transaction, once either family reaches its high-water mark:
//!
//! 1. evict settled receipts of ended generations, oldest first. Under generation pressure it
//!    also empties the oldest ended generations that nothing else pins;
//! 2. retire ended generations that then hold no receipts, after writing one history record
//!    of each one's usage attribution (never read on the hot path);
//! 3. advance the generation floor in the head; the generation counter itself never resets.
//!
//! A pass that reclaims nothing records an idle watermark in the head and later passes skip
//! the scan while it holds. It holds only while the current generation and the pressured
//! families are unchanged and no write touched a pinning family other than appending a
//! current-generation receipt (`save` clears it); those are the only ways a fruitless pass can
//! become fruitful.
//!
//! A fenced generation can no longer commit, so dropping its replay answer only turns a stale
//! retry into a fenced refusal. Never evicted or retired: the current generation and its
//! receipts, the newest bound generation (a relaunch continues its transcript), a receipt still
//! owing a settlement or post-commit effect or watched by an active wake, and a generation with
//! a pending question or a job that still needs reaping. A fenced generation's open turns can
//! never be settled, so they do not pin it. Budget request rows are never touched.
use std::collections::BTreeSet;

use anyhow::{Context, Result, ensure};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Child, Head, LAYOUT_RETIRED, RETIRED, area, split, supported, work_area};
use crate::Store;
use crate::agent::{
    AgentGeneration, AgentQuestion, GENERATIONS_HIGH_WATER, GENERATIONS_MAX, JobRecord,
    RECEIPTS_HIGH_WATER, RECEIPTS_MAX, RequestReceipt, WakeState, WakeSubscription,
};

/// When a retention pass runs and how far it reclaims each family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionLimits {
    pub receipts_high_water: u64,
    pub receipts_retained: u64,
    pub generations_high_water: u64,
    pub generations_retained: u64,
}

impl RetentionLimits {
    /// Both families are reclaimed down to half, so the scan runs once per batch.
    pub const PM: Self = Self {
        receipts_high_water: RECEIPTS_HIGH_WATER as u64,
        receipts_retained: RECEIPTS_MAX as u64 / 2,
        generations_high_water: GENERATIONS_HIGH_WATER as u64,
        generations_retained: GENERATIONS_MAX as u64 / 2,
    };
}

/// What one pass evicted and retired, and the receipt family size it left.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Retention {
    pub receipts: u64,
    pub generations: u64,
    pub receipt_count: u64,
}

/// A retention pass under these pressures reclaimed nothing while `generation` was current.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Idle {
    generation: u32,
    receipts: bool,
    generations: bool,
}

struct Held {
    ordinal: u64,
    key: String,
    generation: u32,
    evictable: bool,
}

fn children<T: DeserializeOwned>(store: &Store, kind: &str) -> Result<Vec<(String, u64, T)>> {
    store
        .record_rows(&area(kind))?
        .into_iter()
        .map(|row| {
            let child: Child = serde_json::from_value(row.doc)
                .with_context(|| format!("decoding PM {kind} record"))?;
            let record = serde_json::from_value(child.record)
                .with_context(|| format!("decoding PM {kind} record"))?;
            Ok((row.key, child.ordinal, record))
        })
        .collect()
}

fn remove(store: &Store, kind: &str, key: &str) -> Result<()> {
    store.delete_record(&area(kind), key)?;
    store.delete_record(&work_area(kind), key)
}

/// One retention pass. Call under the PM transaction whose limit check or read it precedes;
/// `save` rereads the counters, floor and watermark it writes.
pub fn retain(store: &Store, limits: &RetentionLimits) -> Result<Retention> {
    let Some(doc) = store.record::<Value>("agent-pm", "current")? else {
        return Ok(Retention::default());
    };
    if doc.get("layout").is_none() {
        // A legacy journal is upgraded by its next serialized update; nothing is reclaimed.
        let (_, rows) = split(&serde_json::from_value(doc).context("decoding legacy PM")?)?;
        let receipt_count = rows.keys().filter(|(kind, _)| kind == "receipts").count() as u64;
        return Ok(Retention {
            receipt_count,
            ..Retention::default()
        });
    }
    let mut head: Head = serde_json::from_value(doc).context("decoding PM head")?;
    ensure!(supported(head.layout), "unsupported PM record layout");
    let receipts = head.counts.get("receipts").copied().unwrap_or(0);
    let generations = head.counts.get("generations").copied().unwrap_or(0);
    let receipt_pressure = receipts >= limits.receipts_high_water;
    let generation_pressure = generations >= limits.generations_high_water;
    let current = head.pm.state.binding.generation;
    let idle = head.idle.is_some_and(|idle| {
        idle.generation == current
            && (!receipt_pressure || idle.receipts)
            && (!generation_pressure || idle.generations)
    });
    if idle || (!receipt_pressure && !generation_pressure) {
        return Ok(Retention {
            receipt_count: receipts,
            ..Retention::default()
        });
    }

    let mut ended: Vec<(String, AgentGeneration)> = children(store, "generations")?
        .into_iter()
        .map(|(key, _, generation)| (key, generation))
        .collect();
    ended.sort_by_key(|(_, generation)| generation.n);
    let latest_bound = ended
        .iter()
        .filter(|(_, g)| g.bound.is_some())
        .map(|(_, g)| g.n)
        .max();
    let mut pinned: BTreeSet<u32> = [Some(current), latest_bound]
        .into_iter()
        .flatten()
        .collect();
    pinned.extend(
        ended
            .iter()
            .filter(|(_, g)| g.fenced.is_none())
            .map(|(_, g)| g.n),
    );
    for (_, _, question) in children::<AgentQuestion>(store, "questions")? {
        if question.recorded_pending() {
            pinned.insert(question.asked.generation);
        }
    }
    for (_, _, job) in children::<JobRecord>(store, "jobs")? {
        if job.needs_reap() {
            pinned.insert(job.generation);
        }
    }
    let watched: BTreeSet<String> = children::<WakeSubscription>(store, "wakes")?
        .into_iter()
        .filter(|(_, _, wake)| matches!(wake.state, WakeState::Armed | WakeState::PendingStart))
        .map(|(_, _, wake)| wake.request_id)
        .collect();
    let mut held = Vec::new();
    for (key, ordinal, receipt) in children::<RequestReceipt>(store, "receipts")? {
        let generation = receipt.identity.generation;
        let evictable = generation != current
            && !receipt.effects_pending()
            && !watched.contains(&receipt.request_id);
        if !evictable {
            pinned.insert(generation);
        }
        held.push(Held {
            ordinal,
            key,
            generation,
            evictable,
        });
    }
    held.sort_unstable_by_key(|h| h.ordinal);

    let mut evict = BTreeSet::new();
    let receipt_excess = receipts.saturating_sub(limits.receipts_retained);
    let generation_excess = generations.saturating_sub(limits.generations_retained);
    if receipt_pressure {
        let excess = receipt_excess;
        evict.extend(
            held.iter()
                .filter(|h| h.evictable)
                .take(usize::try_from(excess).unwrap_or(usize::MAX))
                .map(|h| h.key.clone()),
        );
    }
    if generation_pressure {
        let excess = generation_excess;
        let emptied: BTreeSet<u32> = ended
            .iter()
            .map(|(_, g)| g.n)
            .filter(|n| !pinned.contains(n))
            .take(usize::try_from(excess).unwrap_or(usize::MAX))
            .collect();
        // Every receipt of an unpinned generation is settled and not watched.
        evict.extend(
            held.iter()
                .filter(|h| emptied.contains(&h.generation))
                .map(|h| h.key.clone()),
        );
    }
    for key in &evict {
        remove(store, "receipts", key)?;
    }
    let holding: BTreeSet<u32> = held
        .iter()
        .filter(|h| !evict.contains(&h.key))
        .map(|h| h.generation)
        .collect();

    let mut retired = 0u64;
    for (key, generation) in &ended {
        if pinned.contains(&generation.n) || holding.contains(&generation.n) {
            continue;
        }
        let record = head
            .pm
            .state
            .retired
            .get_or_insert_with(Default::default)
            .retire(generation);
        store.save_record(RETIRED, &format!("{:010}", generation.n), &record)?;
        remove(store, "generations", key)?;
        retired += 1;
    }

    let evicted = evict.len() as u64;
    let receipt_count = receipts.saturating_sub(evicted);
    if evicted > 0 || retired > 0 {
        head.idle = None;
        head.counts.insert("receipts".into(), receipt_count);
        head.counts
            .insert("generations".into(), generations.saturating_sub(retired));
        if head.pm.state.retired.is_some() {
            head.layout = LAYOUT_RETIRED;
        }
        store.save_record("agent-pm", "current", &head)?;
    } else if (!receipt_pressure || receipt_excess > 0)
        && (!generation_pressure || generation_excess > 0)
    {
        // With a positive excess, nothing reclaimed means nothing was eligible, whatever the
        // limits: the outcome can change only through what clears the watermark.
        head.idle = Some(Idle {
            generation: current,
            receipts: receipt_pressure,
            generations: generation_pressure,
        });
        store.save_record("agent-pm", "current", &head)?;
    }
    Ok(Retention {
        receipts: evicted,
        generations: retired,
        receipt_count,
    })
}
