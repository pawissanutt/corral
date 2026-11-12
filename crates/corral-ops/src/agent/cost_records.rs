//! PM envelope causes and per-response cost reporting. Evidence, never admission policy.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};
use corral_core::Usage;
use corral_core::agent::pm_cost::{self, Cause, CostAttribution, CostClass, PendingCause};
use corral_core::agent::request::Continuation;
use corral_core::agent::{AgentRole, AgentRunState, AgentTurn, TurnOriginKind, pm_state};
use corral_core::inbox::{Envelope, Inbox, Message, Recipient};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Ctx;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CostWindow {
    /// Inclusive. Undated legacy entries are included only in an unbounded window.
    pub since: Option<Timestamp>,
    /// Exclusive.
    pub until: Option<Timestamp>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CostRow {
    pub class: CostClass,
    pub project: Option<String>,
    pub task: Option<String>,
    pub item_key: Option<String>,
    pub requests: u64,
    pub input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub output: u64,
    /// Tokens from requests without an API-equivalent price, independent of partial USD.
    pub unpriced_tokens: u64,
    /// API-equivalent estimate, matching the container's estimated_usd; None if unpriced.
    pub usd: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CostCoverage {
    pub attributed_requests: u64,
    pub total_requests: u64,
    pub dropped: u64,
    /// Requests with no structural turn marker (legacy and auxiliary sessions).
    pub legacy: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CostReport {
    /// Always includes explicit Overhead and Unallocated rows, even when zero.
    pub rows: Vec<CostRow>,
    pub coverage: CostCoverage,
    pub total: Usage,
}

fn string(subject: &Value, field: &str) -> Option<String> {
    subject[field].as_str().map(str::to_owned)
}

fn task_cause(ctx: &Ctx, key: &str, project: Option<String>) -> Result<Cause> {
    let task = key.split('/').next().filter(|id| {
        id.strip_prefix("T-")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    });
    let Some(task) = task else {
        return Ok(Cause {
            project,
            ..Cause::default()
        });
    };
    let head = ctx
        .store
        .head(task)
        .with_context(|| format!("resolving cost cause {key}"))?;
    Ok(Cause {
        class: CostClass::Item,
        project: head.task.project.clone().or(project),
        task: Some(task.into()),
        item_key: Some(key.into()),
        kinds: Vec::new(),
    })
}

/// Merge all members conservatively: a mixed/multi-task envelope never claims one task.
fn merge(causes: Vec<Cause>) -> Cause {
    let kinds: Vec<_> = causes
        .iter()
        .flat_map(|c| c.kinds.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .take(4)
        .collect();
    if causes.is_empty() || causes.iter().all(|c| c.class == CostClass::Overhead) {
        return Cause {
            class: CostClass::Overhead,
            kinds,
            ..Cause::default()
        };
    }
    let projects: BTreeSet<_> = causes.iter().map(|c| c.project.clone()).collect();
    let project = (projects.len() == 1)
        .then(|| projects.into_iter().next().flatten())
        .flatten();
    let tasks: BTreeSet<_> = causes.iter().map(|c| c.task.clone()).collect();
    let task = (tasks.len() == 1)
        .then(|| tasks.into_iter().next().flatten())
        .flatten();
    let keys: BTreeSet<_> = causes.iter().map(|c| c.item_key.clone()).collect();
    let item_key = (keys.len() == 1)
        .then(|| keys.into_iter().next().flatten())
        .flatten();
    let class = if task.is_some() && causes.iter().all(|c| c.class == CostClass::Item) {
        CostClass::Item
    } else if causes.len() > 1 || causes.iter().any(|c| c.class == CostClass::Shared) {
        CostClass::Shared
    } else {
        causes[0].class
    };
    Cause {
        class,
        project,
        task: (class == CostClass::Item).then_some(task).flatten(),
        item_key,
        kinds,
    }
}

fn message_cause(ctx: &Ctx, message: &Message) -> Result<Cause> {
    let subject = message.subject.as_ref().unwrap_or(&Value::Null);
    let mut cause = match message.kind.as_str() {
        "question_wake" => {
            let key = subject["question"]
                .as_str()
                .or_else(|| subject["key"].as_str())
                .unwrap_or("");
            task_cause(ctx, key, string(subject, "project"))?
        }
        "pm_inbox_wake" => {
            let id = subject["item"]
                .as_str()
                .context("PM inbox wake has no item subject")?;
            let item = ctx.store.pm_inbox_item(id)?;
            let mut causes = Vec::new();
            for reference in &item.refs {
                if let Some(project) = reference.strip_prefix("project:") {
                    causes.push(Cause {
                        class: CostClass::Shared,
                        project: Some(project.into()),
                        ..Cause::default()
                    });
                } else if reference.starts_with("T-") {
                    causes.push(task_cause(ctx, reference, string(subject, "project"))?);
                }
            }
            let mut cause = if causes.is_empty() {
                Cause {
                    class: CostClass::Item,
                    project: string(subject, "project"),
                    ..Cause::default()
                }
            } else {
                merge(causes)
            };
            cause.item_key = Some(id.into());
            cause
        }
        "pm_event_wake" => {
            let mut causes = Vec::new();
            for key in subject["causes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                let task_key = key
                    .strip_prefix("q:")
                    .or_else(|| key.strip_prefix("cond:").and_then(|s| s.split(':').next()));
                causes.push(match task_key {
                    Some(key) => task_cause(ctx, key, None)?,
                    None => Cause::default(),
                });
            }
            if causes.is_empty() {
                Cause::default()
            } else {
                merge(causes)
            }
        }
        "pm_wait_wake" => Cause {
            class: CostClass::Shared,
            project: string(subject, "project"),
            item_key: string(subject, "key"),
            ..Cause::default()
        },
        _ => Cause {
            class: CostClass::Overhead,
            ..Cause::default()
        },
    };
    cause.kinds = vec![message.kind.clone()];
    Ok(cause)
}

/// Called immediately before bridge submission, after the envelope was frozen. Only typed
/// member metadata is read; untrusted body text is never parsed for attribution.
pub(crate) fn prepare_envelope(ctx: &Ctx, envelope: &Envelope) -> Result<()> {
    if envelope.idempotency_key.split('@').next() != Some("pm") {
        return Ok(());
    }
    let inbox = Inbox::new(&ctx.store, Recipient::Pm)?;
    let causes = envelope
        .members
        .iter()
        .map(|seq| message_cause(ctx, &inbox.message(*seq)?))
        .collect::<Result<Vec<_>>>()?;
    pm_cost::prepare(
        &ctx.store,
        PendingCause {
            envelope_key: super::delivery::wire_key(&envelope.idempotency_key),
            cause: merge(causes),
        },
    )
}

/// Called inside the canonical turn-start transaction. The request ledger, not sequence or
/// time proximity, supplies the predecessor for a continuation key.
pub(crate) fn bind_turn(ctx: &Ctx, state: &AgentRunState, turn: &AgentTurn) -> Result<()> {
    if state.binding.role != AgentRole::Pm {
        return Ok(());
    }
    let parent = if turn.origin.kind == TurnOriginKind::Continuation {
        state
            .todo_requests
            .as_ref()
            .and_then(|ledger| ledger.turn.as_ref())
            .and_then(|record| {
                let key = match &record.continuation {
                    Continuation::Due { key, .. } | Continuation::Duplicate { key } => key,
                    _ => return None,
                };
                (turn.origin.idempotency_key.as_ref() == Some(key))
                    .then_some((record.generation, record.turn))
            })
    } else {
        None
    };
    pm_cost::bind(&ctx.store, turn.generation, turn.seq, &turn.origin, parent)?;
    Ok(())
}

pub fn by_item(ctx: &Ctx, window: CostWindow) -> Result<CostReport> {
    let pm = pm_state::load(&ctx.store)?.context("no bound Pi PM")?;
    let records = pm_cost::load(&ctx.store)?;
    let attributions: BTreeMap<_, _> = records
        .attributions
        .into_iter()
        .map(|record| ((record.generation, record.turn), record))
        .collect();
    let envelopes: BTreeMap<_, _> = attributions
        .values()
        .filter_map(|record| Some((record.envelope_key.as_deref()?, record)))
        .collect();
    let pending: BTreeMap<_, _> = records
        .pending
        .into_iter()
        .map(|pending| {
            let cause = pending.cause;
            (
                pending.envelope_key,
                CostAttribution {
                    class: cause.class,
                    project: cause.project,
                    task: cause.task,
                    item_key: cause.item_key,
                    kinds: cause.kinds,
                    ..CostAttribution::default()
                },
            )
        })
        .collect();
    let entries = super::usage::collect_entries(&pm.state, None, &ctx.store, false)?;
    let mut report = CostReport {
        coverage: CostCoverage {
            dropped: records.dropped,
            ..CostCoverage::default()
        },
        ..CostReport::default()
    };
    type RowKey = (CostClass, Option<String>, Option<String>, Option<String>);
    let mut rows: BTreeMap<RowKey, CostRow> = BTreeMap::new();
    for class in [CostClass::Overhead, CostClass::Unallocated] {
        rows.insert(
            (class, None, None, None),
            CostRow {
                class,
                ..CostRow::default()
            },
        );
    }
    let mut total = Vec::new();
    for entry in entries {
        if match entry.usage.at {
            Some(at) => {
                window.since.is_some_and(|since| at < since)
                    || window.until.is_some_and(|until| at >= until)
            }
            None => window.since.is_some() || window.until.is_some(),
        } {
            continue;
        }
        report.coverage.total_requests += 1;
        if entry.span.is_none() {
            report.coverage.legacy += 1;
        }
        let cause = entry.span.as_ref().and_then(|span| {
            if span.queued {
                // A queued input owns a new span, not the attribution of the running turn.
                span.key
                    .as_deref()
                    .and_then(|key| envelopes.get(key).copied().or_else(|| pending.get(key)))
            } else {
                attributions.get(&span.turn)
            }
        });
        let missing = CostAttribution::default();
        let cause = cause.unwrap_or(&missing);
        if cause.class != CostClass::Unallocated {
            report.coverage.attributed_requests += 1;
        }
        let key = (
            cause.class,
            cause.project.clone(),
            cause.task.clone(),
            cause.item_key.clone(),
        );
        let row = rows.entry(key).or_insert_with(|| CostRow {
            class: cause.class,
            project: cause.project.clone(),
            task: cause.task.clone(),
            item_key: cause.item_key.clone(),
            ..CostRow::default()
        });
        row.requests += 1;
        row.input = row.input.saturating_add(entry.usage.input);
        row.cache_read = row.cache_read.saturating_add(entry.usage.cache_read);
        row.cache_write = row.cache_write.saturating_add(entry.usage.cache_write);
        row.output = row.output.saturating_add(entry.usage.output);
        row.unpriced_tokens = row
            .unpriced_tokens
            .saturating_add(entry.usage.unpriced_tokens);
        if let Some(usd) = entry.usage.estimated_usd {
            *row.usd.get_or_insert(0.0) += usd;
        }
        total.push(entry.usage);
    }
    report.rows = rows.into_values().collect();
    report.total = Usage::from_entries(total);
    Ok(report)
}

#[cfg(all(test, unix))]
#[path = "cost_records_tests.rs"]
mod tests;
