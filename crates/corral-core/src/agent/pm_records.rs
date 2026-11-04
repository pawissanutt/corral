//! Normalized root PM journal. The head has no retained child vectors; unfinished
//! indexes contain references, never a second authority for child documents.
use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{AgentPm, NativeRecord, paths};
use crate::Store;

const ENDED: &str = "agent-pm-ended-turns";

pub(super) const KINDS: [&str; 10] = [
    "generations",
    "receipts",
    "questions",
    "turns",
    "wakes",
    "reports",
    "jobs",
    "rewinds",
    "usage_sessions",
    "budget_requests",
];

/// Explicit read set. Unselected children are not removed by `update_selected`.
#[derive(Debug, Clone, Default)]
pub struct Selection {
    pub current: bool,
    pub work: bool,
    pub keys: Vec<(&'static str, String)>,
    pub families: Vec<&'static str>,
}

impl Selection {
    pub fn current() -> Self {
        Self {
            current: true,
            ..Self::default()
        }
    }
    pub fn work() -> Self {
        Self {
            work: true,
            ..Self::current()
        }
    }
    pub fn request(generation: u32, request: &str) -> Self {
        Self::current()
            .key("generations", format!("gen:{generation}"))
            .key("receipts", format!("request:{request}"))
    }
    pub fn key(mut self, kind: &'static str, token: impl Into<String>) -> Self {
        self.keys.push((kind, token.into()));
        self
    }
    pub fn family(mut self, kind: &'static str) -> Self {
        self.families.push(kind);
        self
    }
    pub(super) fn all() -> Self {
        Self {
            families: KINDS.into(),
            ..Self::default()
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Head {
    layout: u32,
    #[serde(with = "fixed_counts")]
    next: BTreeMap<String, u64>,
    #[serde(with = "fixed_counts")]
    counts: BTreeMap<String, u64>,
    pm: AgentPm,
}

#[derive(Clone, Serialize, Deserialize)]
struct Child {
    #[serde(with = "fixed_ordinal")]
    ordinal: u64,
    record: Value,
}

type Children = BTreeMap<(String, String), Child>;

fn area(kind: &str) -> String {
    format!("agent-pm-{kind}")
}
fn work_area(kind: &str) -> String {
    format!("agent-pm-work-{kind}")
}
fn key(token: &str) -> String {
    paths::sha256_hex(token.as_bytes())
}

fn split(pm: &AgentPm) -> Result<(AgentPm, Children)> {
    let mut doc = serde_json::to_value(pm)?;
    let state = doc["state"]
        .as_object_mut()
        .context("PM state is not an object")?;
    let mut children = Children::new();
    for kind in KINDS {
        let values = if kind == "budget_requests" {
            state
                .remove("budget")
                .and_then(|mut b| b["requests"].take().as_array().cloned())
        } else {
            state.remove(kind).and_then(|v| v.as_array().cloned())
        }
        .unwrap_or_default();
        for (ordinal, record) in values.into_iter().enumerate() {
            let native = NativeRecord::from_doc(kind, record.clone())?;
            ensure!(
                children
                    .insert(
                        (kind.into(), key(&native.token())),
                        Child {
                            ordinal: ordinal as u64,
                            record,
                        }
                    )
                    .is_none(),
                "duplicate PM {kind} identity"
            );
        }
    }
    Ok((serde_json::from_value(doc)?, children))
}

fn assemble(mut pm: AgentPm, rows: &Children) -> Result<AgentPm> {
    // Fold in stored vector order, not hashed identity order.
    let mut sorted: Vec<_> = rows.iter().collect();
    sorted.sort_by_key(|((kind, _), child)| (kind.as_str(), child.ordinal));
    for ((kind, _), child) in sorted {
        NativeRecord::from_doc(kind, child.record.clone())?.put(&mut pm.state);
    }
    Ok(pm)
}

pub(super) fn read(store: &Store, selection: &Selection) -> Result<Option<(Head, AgentPm)>> {
    let Some(doc) = store.record::<Value>("agent-pm", "current")? else {
        return Ok(None);
    };
    if doc.get("layout").is_none() {
        // Legacy journals are upgraded by the next serialized update/migrate. A read never
        // acquires dispatch recursively (task transactions also read the PM binding).
        let pm: AgentPm = serde_json::from_value(doc).context("decoding legacy PM")?;
        let (shell, rows) = split(&pm)?;
        return Ok(Some((head(shell, &rows), pm)));
    }
    let head: Head = serde_json::from_value(doc).context("decoding PM head")?;
    ensure!(head.layout == 2, "unsupported PM record layout");
    let mut keys = selection.keys.clone();
    if selection.current {
        keys.push((
            "generations",
            format!("gen:{}", head.pm.state.binding.generation),
        ));
    }
    let mut rows = Children::new();
    for kind in KINDS {
        if selection.families.contains(&kind) && head.counts.get(kind).copied().unwrap_or(0) != 0 {
            for row in store.record_rows(&area(kind))? {
                rows.insert((kind.into(), row.key), serde_json::from_value(row.doc)?);
            }
        } else if selection.work {
            for row in store.record_rows(&work_area(kind))? {
                let child = store
                    .record(&area(kind), &row.key)?
                    .context("dangling PM work index")?;
                rows.insert((kind.into(), row.key), child);
            }
        }
    }
    for (kind, token) in keys {
        ensure!(KINDS.contains(&kind), "unknown PM family {kind}");
        let key = key(&token);
        if !rows.contains_key(&(kind.into(), key.clone()))
            && let Some(child) = store.record(&area(kind), &key)?
        {
            rows.insert((kind.into(), key), child);
        }
    }
    let pm = assemble(head.pm.clone(), &rows)?;
    Ok(Some((head, pm)))
}

pub(super) fn count(store: &Store, kind: &str) -> Result<u64> {
    ensure!(KINDS.contains(&kind), "unknown PM family {kind}");
    let Some(doc) = store.record::<Value>("agent-pm", "current")? else {
        return Ok(0);
    };
    if doc.get("layout").is_none() {
        let (_, children) = split(&serde_json::from_value(doc)?)?;
        return Ok(children.keys().filter(|(k, _)| k == kind).count() as u64);
    }
    let head: Head = serde_json::from_value(doc)?;
    ensure!(head.layout == 2, "unsupported PM record layout");
    Ok(head.counts.get(kind).copied().unwrap_or(0))
}

fn head(pm: AgentPm, rows: &Children) -> Head {
    let mut counts = BTreeMap::new();
    for (kind, _) in rows.keys() {
        *counts.entry(kind.clone()).or_default() += 1;
    }
    Head {
        layout: 2,
        next: counts.clone(),
        counts,
        pm,
    }
}

/// Compare only the selected before/after sets, retaining every unselected record.
pub(super) fn save(store: &Store, old: Option<&(Head, AgentPm)>, pm: &AgentPm) -> Result<()> {
    let (shell, mut after) = split(pm)?;
    let before = old
        .map(|(_, pm)| split(pm).map(|(_, rows)| rows))
        .transpose()?
        .unwrap_or_default();
    let mut head = old
        .map(|(head, _)| head.clone())
        .unwrap_or_else(|| head(shell.clone(), &Children::new()));
    head.pm = shell;
    let legacy = store
        .record::<Value>("agent-pm", "current")?
        .is_some_and(|d| d.get("layout").is_none());
    if legacy {
        head.next.clear();
        head.counts.clear();
    }
    for ((kind, key), previous) in &before {
        if !after.contains_key(&(kind.clone(), key.clone())) {
            if kind == "turns"
                && let Some(child) = store.record::<Child>(&area(kind), key)?
            {
                store.delete_record(ENDED, &format!("{:020}", child.ordinal))?;
            }
            store.delete_record(&area(kind), key)?;
            store.delete_record(&work_area(kind), key)?;
            let count = head.counts.entry(kind.clone()).or_default();
            *count = count.saturating_sub(1);
        } else if !legacy && after[&(kind.clone(), key.clone())].record == previous.record {
            continue;
        }
    }
    let mut ordered: Vec<_> = after.iter_mut().collect();
    ordered.sort_by_key(|((kind, _), child)| (kind.as_str(), child.ordinal));
    for ((kind, key), child) in ordered {
        if !legacy
            && before
                .get(&(kind.clone(), key.clone()))
                .is_some_and(|c| c.record == child.record)
        {
            continue;
        }
        let existing: Option<Child> = if legacy {
            None
        } else {
            store.record(&area(kind), key)?
        };
        child.ordinal = if let Some(old) = existing {
            old.ordinal
        } else {
            let next = head.next.entry(kind.clone()).or_default();
            let ordinal = *next;
            *next += 1;
            *head.counts.entry(kind.clone()).or_default() += 1;
            ordinal
        };
        store.save_record(&area(kind), key, child)?;
        if kind == "turns" {
            let ordinal = format!("{:020}", child.ordinal);
            if child.record.get("ended").is_some_and(|v| !v.is_null()) {
                store.save_record(ENDED, &ordinal, key)?;
            } else {
                store.delete_record(ENDED, &ordinal)?;
            }
        }
        let native = NativeRecord::from_doc(kind, child.record.clone())?;
        if native.metadata().0 {
            store.save_record(&work_area(kind), key, &true)?;
        } else {
            store.delete_record(&work_area(kind), key)?;
        }
    }
    // Match event retention: evict oldest ended turns only; open turns are never lost.
    while head.counts.get("turns").copied().unwrap_or(0) > super::TURNS_MAX as u64 {
        let Some(row) = store.first_record_rows(ENDED, 1)?.pop() else {
            break;
        };
        let key = row.doc.as_str().context("invalid PM ended-turn index")?;
        store.delete_record(&area("turns"), key)?;
        store.delete_record(&work_area("turns"), key)?;
        store.delete_record(ENDED, &row.key)?;
        *head.counts.entry("turns".into()).or_default() -= 1;
    }
    for (kind, limit) in [
        ("receipts", super::RECEIPTS_MAX),
        ("generations", super::GENERATIONS_MAX),
    ] {
        let count = head.counts.get(kind).copied().unwrap_or(0);
        let previous = old
            .and_then(|(h, _)| h.counts.get(kind))
            .copied()
            .unwrap_or(0);
        ensure!(
            count <= limit as u64 || count <= previous,
            "PM {kind} limit reached"
        );
    }
    store.save_record("agent-pm", "current", &head)
}

// Fixed-width counters keep the bounded head's byte cost independent of ordinal width.
mod fixed_ordinal {
    use super::*;
    pub fn serialize<S: serde::Serializer>(value: &u64, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format!("{value:020}"))
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

mod fixed_counts {
    use super::*;
    pub fn serialize<S: serde::Serializer>(
        values: &BTreeMap<String, u64>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        values
            .iter()
            .map(|(k, v)| (k, format!("{v:020}")))
            .collect::<BTreeMap<_, _>>()
            .serialize(serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<BTreeMap<String, u64>, D::Error> {
        BTreeMap::<String, String>::deserialize(deserializer)?
            .into_iter()
            .map(|(k, v)| v.parse().map(|v| (k, v)).map_err(serde::de::Error::custom))
            .collect()
    }
}

pub(super) fn delete(store: &Store) -> Result<()> {
    for kind in KINDS {
        for area in [area(kind), work_area(kind)] {
            for row in store.record_rows(&area)? {
                store.delete_record(&area, &row.key)?;
            }
        }
    }
    for row in store.record_rows(ENDED)? {
        store.delete_record(ENDED, &row.key)?;
    }
    store.delete_record("agent-pm", "current")
}
