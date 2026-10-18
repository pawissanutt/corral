//! Read-time, per-task lifecycle history. No events are rewritten or backfilled.

use std::time::Duration;

use anyhow::Result;
use corral_core::{Actor, Event, Store};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryClass {
    Lifecycle,
    Review,
    Run,
    Noise,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TimelineFilter {
    /// Lifecycle, review and run entries; hide delivery receipts and other noise.
    #[default]
    Relevant,
    All,
}

/// Reserved for read-time block/resolution pairing (task T-165).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wait {
    pub opened_at: Timestamp,
    pub duration: Duration,
    pub opened_by_kind: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimelineEntry {
    pub at: Timestamp,
    pub kind: String,
    pub class: EntryClass,
    pub actor: Option<String>,
    pub via: Option<String>,
    pub run: Option<String>,
    pub summary: String,
    pub wait: Option<Wait>,
    pub event: Event,
}

/// Query one task's events, oldest first, retaining append order for equal timestamps.
///
/// # Errors
/// Returns an error for an invalid task id or an unreadable event log.
pub fn timeline(
    store: &Store,
    task_id: &str,
    filter: TimelineFilter,
) -> Result<Vec<TimelineEntry>> {
    let id = Store::normalize_id(task_id)?;
    let mut entries: Vec<_> = store
        .events()?
        .into_iter()
        .filter(|event| event.task.as_deref() == Some(&id))
        .map(|event| {
            let actor = event
                .actor
                .map(|actor| match actor {
                    Actor::Owner => "owner",
                    Actor::Pm => "pm",
                    Actor::Agent => "agent",
                    Actor::Autopilot => "autopilot",
                    Actor::Reviewer => "reviewer",
                    Actor::System => "system",
                })
                .or_else(|| event.detail["actor"].as_str())
                .or_else(|| event.detail["by"].as_str())
                .or_else(|| {
                    (event.kind == "created")
                        .then(|| event.detail["origin"].as_str())
                        .flatten()
                })
                .map(str::to_owned);
            TimelineEntry {
                at: event.ts,
                kind: event.kind.clone(),
                class: classify(&event.kind),
                actor,
                via: event.via.clone(),
                run: event.detail["run"].as_str().map(str::to_owned),
                summary: summarize(&event),
                wait: None,
                event,
            }
        })
        .filter(|entry| filter == TimelineFilter::All || entry.class != EntryClass::Noise)
        .collect();
    entries.sort_by_key(|entry| entry.at);
    Ok(entries)
}

fn classify(kind: &str) -> EntryClass {
    match kind {
        "created" | "edited" | "moved" | "queued" | "dequeued" | "idea_decided" | "idea_merged"
        | "owner_decision" | "dispatched" | "needs_me" | "blocked" | "escalated" | "replied"
        | "unstuck" | "commented" | "approved" | "changes_requested" | "discarded" | "done"
        | "reverted" | "superseded" => EntryClass::Lifecycle,
        "gate_started" | "gate" | "review_started" | "verdict" | "autopilot" => EntryClass::Review,
        kind if kind.starts_with("plan_") => EntryClass::Review,
        "profile_switched" | "progress_updated" | "idle_no_report" => EntryClass::Run,
        kind if kind.starts_with("checkpoint_") || kind.starts_with("provider_") => EntryClass::Run,
        _ => EntryClass::Noise,
    }
}

fn text<'a>(detail: &'a Value, keys: &[&str]) -> &'a str {
    keys.iter()
        .find_map(|key| detail.pointer(key).and_then(Value::as_str))
        .unwrap_or("")
}

fn summarize(event: &Event) -> String {
    let d = &event.detail;
    let reason = text(d, &["/reason", "/note", "/text", "/error"]);
    let summary = match event.kind.as_str() {
        "created" => format!("{} \"{}\"", text(d, &["/status"]), text(d, &["/title"])),
        "edited" => d["fields"]
            .as_object()
            .map(|fields| {
                fields
                    .iter()
                    .map(|(name, change)| {
                        match (change["lines_from"].as_i64(), change["lines_to"].as_i64()) {
                            (Some(from), Some(to)) => {
                                format!("{name} ({:+} lines)", to.saturating_sub(from))
                            }
                            _ => name.clone(),
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default(),
        "moved" | "autopilot" | "profile_switched" => {
            format!("{} -> {} {reason}", text(d, &["/from"]), text(d, &["/to"]))
        }
        "dispatched" | "plan_dispatched" => format!(
            "{} {}, pane {}",
            text(d, &["/run"]),
            text(d, &["/model", "/profile", "/agent", "/harness"]),
            text(d, &["/pane"])
        ),
        "gate_started" => format!(
            "{} commands, pane {}",
            d["commands"].as_array().map_or(0, Vec::len),
            text(d, &["/pane"])
        ),
        "gate" => format!("{} {}", text(d, &["/result"]), text(d, &["/failed_cmd"])),
        "review_started" | "plan_review_started" | "plan_review_requested" => {
            format!("{} pane {}", text(d, &["/reviewer"]), text(d, &["/pane"]))
        }
        "verdict" | "plan_verdict" => format!(
            "{} {}",
            text(d, &["/verdict", "/review/verdict"]),
            text(d, &["/notes", "/review/notes"])
        ),
        "needs_me" => {
            let question = text(d, &["/question", "/reason", "/note", "/text", "/error"]);
            let action = text(d, &["/requested_action"]);
            if action.is_empty() {
                question.to_owned()
            } else {
                format!("{question} Action: {action}")
            }
        }
        "approved" => match text(d, &["/strategy"]) {
            "keep_branch" => format!(
                "kept branch {} at {} (not merged)",
                text(d, &["/branch"]),
                text(d, &["/sha"])
            ),
            strategy => format!(
                "landed {} ({}) {reason}",
                text(d, &["/sha"]),
                if strategy == "fast_forward" {
                    "ff"
                } else {
                    strategy
                }
            ),
        },
        "done" => format!(
            "{} files, +{} -{}",
            d["files"].as_u64().unwrap_or(0),
            d["added"].as_u64().unwrap_or(0),
            d["removed"].as_u64().unwrap_or(0)
        ),
        "idea_decided" | "owner_decision" => {
            format!("{} {reason}", text(d, &["/action"]))
        }
        "idea_merged" => format!("into {} {reason}", text(d, &["/into"])),
        "commented" => text(d, &["/comment/text", "/text"]).to_owned(),
        "unstuck" => format!("stopped pid {}", d["pid"]),
        "reverted" => format!("reverted {}", text(d, &["/landed_sha"])),
        "superseded" => format!("superseded {}", text(d, &["/run"])),
        "queued" | "dequeued" | "blocked" | "escalated" | "replied" | "changes_requested"
        | "discarded" | "idle_no_report" => reason.to_owned(),
        "progress_updated" => format!("revision {}", d["revision"]),
        kind if kind.starts_with("checkpoint_") => {
            format!("{kind} revision {}", d["revision"])
        }
        "provider_stall" => format!(
            "provider_stall {}",
            text(d, &["/message", "/reason", "/note", "/text", "/error"])
        ),
        kind if kind.starts_with("provider_") => format!("{kind} {reason}"),
        "plan_submitted" => format!("plan {}", text(d, &["/plan/id"])),
        "plan_policy" => format!("required: {}", d["required"]),
        "plan_decomposed" => format!("{} children", d["children"].as_array().map_or(0, Vec::len)),
        kind if kind.starts_with("plan_") => {
            format!("{kind} {} {reason}", text(d, &["/status", "/plan", "/run"]))
        }
        _ => event.kind.clone(),
    };
    let summary = summary.split_whitespace().collect::<Vec<_>>().join(" ");
    if summary.is_empty() {
        event.kind.clone()
    } else {
        summary
    }
}

#[cfg(test)]
#[path = "timeline_tests.rs"]
mod tests;
