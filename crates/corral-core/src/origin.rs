//! Owner intent and explicit, revision-bound permission to reduce it.
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Event, Status, Store, Task};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Owner,
    Pm,
    Agent,
}

impl Origin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Pm => "pm",
            Self::Agent => "agent",
        }
    }
}

/// A recorded disposition keeps merged ideas discoverable after leaving Idea.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdeaDecision {
    /// A new request remains distinct even when its disposition parameters repeat.
    #[serde(default)]
    pub revision: u64,
    pub action: String,
    pub reason: String,
    pub into: Option<String>,
}

impl Task {
    pub fn effective_origin(&self) -> Origin {
        self.origin.unwrap_or(if self.status == Status::Idea {
            Origin::Owner
        } else {
            Origin::Agent
        })
    }
}

/// Permissions bind both the source specification and its current disposition.
pub fn owner_revision(task: &Task) -> Result<String> {
    let bytes = serde_json::to_vec(&(&task.title, &task.body, task.status, &task.idea_decision))
        .context("serializing owner idea revision")?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

impl Store {
    /// Explicit owner consent, not an inference from arbitrary comment prose.
    pub fn owner_decision(&self, id: &str, action: &str, reason: &str, source: &str) -> Result<()> {
        if !matches!(action, "drop" | "decline" | "defer" | "narrow" | "merge") {
            bail!("unknown owner action {action:?}");
        }
        if reason.trim().is_empty() {
            bail!("an owner decision requires a reason");
        }
        self.read_locked(id, |task| {
            self.log_event(
                Some(&task.id),
                "owner_decision",
                serde_json::json!({
                    "action": action, "reason": reason, "actor": "owner",
                    "source": source, "revision": owner_revision(task)?
                }),
            )
        })
    }

    /// Called inside the board write lock, before any protected mutation is saved.
    pub(crate) fn protect_owner_change(&self, before: &Task, after: &Task) -> Result<Vec<Event>> {
        if before.effective_origin() != Origin::Owner {
            return Ok(Vec::new());
        }
        if after.effective_origin() != Origin::Owner {
            bail!("{}: owner origin cannot be removed", before.id);
        }
        let mut actions = Vec::new();
        if before.idea_decision != after.idea_decision {
            actions.push(
                after
                    .idea_decision
                    .as_ref()
                    .map_or("narrow", |d| d.action.as_str()),
            );
        }
        if before.status != Status::Dropped
            && after.status == Status::Dropped
            && !actions
                .iter()
                .any(|a| matches!(*a, "drop" | "decline" | "merge"))
        {
            actions.push("drop");
        }
        if before.title != after.title || before.body != after.body {
            // Semantic narrowing cannot be inferred safely: every spec replacement
            // requires consent, including changes that happen to expand the idea.
            if !actions.contains(&"narrow") {
                actions.push("narrow");
            }
        }
        if matches!(after.status, Status::Idea | Status::Backlog)
            && before.status != after.status
            && matches!(
                before.status,
                Status::Ready | Status::Running | Status::NeedsMe | Status::Review
            )
            && !actions.contains(&"defer")
        {
            actions.push("defer");
        }
        if actions.is_empty() {
            return Ok(Vec::new());
        }
        let events = self.events_shared()?;
        let revision = owner_revision(before)?;
        actions.into_iter().map(|action| {
            let decision = events.iter().rev().find(|event| {
            event.task.as_deref() == Some(&before.id)
                && event.kind == "owner_decision"
                && event.detail["actor"] == "owner"
                && event.detail["action"] == action
                && event.detail["revision"] == revision
                && !events.iter().any(|used| {
                    used.task == event.task
                        && used.kind == "idea_decided"
                        && used.detail["decision_at"] == event.ts.to_string()
                })
            });
            let Some(decision) = decision else {
                bail!(
                "{}: {action} of an owner idea needs explicit owner approval; use --owner-ok or an owner comment `owner-ok: {action} <reason>`",
                before.id
                );
            };
            Ok(decision.clone())
        }).collect()
    }
}
