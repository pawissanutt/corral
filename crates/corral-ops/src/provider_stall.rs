//! Transient provider failures (overloaded, rate limited, 5xx) that end a harness turn.
//!
//! The harness stops with `stopReason=error`, nothing retries it, and herdr reports a blocked or idle
//! pane. Sync detects this from the harness session log, re-prompts the agent or reviewer with
//! backoff, records every attempt as an event, and escalates with the real reason once the retries
//! are used up. `provider_session::discover` is the per-harness adapter hook: OmO and OMP read their
//! session logs, other harnesses report no stall until they get an adapter.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use corral_core::{Run, Status, Task};
use corral_herdr::{AgentStatus, PaneInfo};
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Change, Ctx, provider_session};

/// Wait before each retry, measured from the stall (first) or the previous attempt. After the last
/// retry the same final delay is allowed for recovery before escalating.
pub(crate) const BACKOFF_SECS: [i64; 3] = [60, 120, 300];
const TAIL_BYTES: u64 = 512 * 1024;

/// The harness turn that ended on a transient provider error, and nothing after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Stall {
    pub at: Timestamp,
    pub message: String,
}

pub(crate) fn is_transient(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    [
        "overloaded",
        "rate limit",
        "rate_limit",
        "too many requests",
        "try again later",
        "temporarily unavailable",
        "service unavailable",
        "bad gateway",
        "gateway timeout",
        "internal server error",
        "capacity",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
        || lower
            .split(|c: char| !c.is_ascii_digit())
            .any(|word| matches!(word, "429" | "500" | "502" | "503" | "504" | "529"))
}

/// Session-log JSONL -> the open stall, if the last assistant turn failed transiently. A later
/// assistant or tool-result entry means the harness moved on; user entries (our own retry prompts) do not.
pub(crate) fn scan(log: &str) -> Option<Stall> {
    let mut stall = None;
    for line in log.lines() {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if entry["type"] != "message" {
            continue;
        }
        let message = &entry["message"];
        match message["role"].as_str() {
            Some("assistant") => {
                stall = None;
                if message["stopReason"] == "error"
                    && let Some(text) = message["errorMessage"]
                        .as_str()
                        .filter(|text| is_transient(text))
                    && let Some(at) = entry["timestamp"]
                        .as_str()
                        .and_then(|at| at.parse::<Timestamp>().ok())
                {
                    stall = Some(Stall {
                        at,
                        message: text.to_string(),
                    });
                }
            }
            Some("toolResult") => stall = None,
            _ => {}
        }
    }
    stall
}

fn read_tail(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    // A tail read starts mid-line.
    Some(if start == 0 {
        text.into_owned()
    } else {
        text.split_once('\n')?.1.to_string()
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct State {
    pub since: Timestamp,
    pub error_at: Timestamp,
    pub message: String,
    pub attempts: u32,
    pub last_attempt: Option<Timestamp>,
    pub escalated: bool,
    /// The session log that owns this stall; later failure and recovery are read from it alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<PathBuf>,
}

impl State {
    pub(crate) fn reason(&self) -> String {
        let label = if self.message.to_ascii_lowercase().contains("overloaded") {
            "provider overloaded"
        } else {
            "provider unavailable"
        };
        format!(
            "{label} since {}: {}",
            hhmm(self.since),
            self.message.trim()
        )
    }
}

fn hhmm(at: Timestamp) -> String {
    at.to_zoned(jiff::tz::TimeZone::system())
        .strftime("%H:%M")
        .to_string()
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// No stall, or an escalated one that already needs the owner.
    Idle,
    Recovered,
    Wait,
    Retry(u32),
    Escalate,
}

/// Pure backoff machine. `may_retry` false defers a due retry (the harness looks busy).
pub(crate) fn advance_state(
    state: &mut Option<State>,
    stall: Option<&Stall>,
    may_retry: bool,
    now: Timestamp,
) -> Step {
    let Some(stall) = stall else {
        return if state.take().is_some() {
            Step::Recovered
        } else {
            Step::Idle
        };
    };
    let fresh = || State {
        since: stall.at,
        error_at: stall.at,
        message: stall.message.clone(),
        attempts: 0,
        last_attempt: None,
        escalated: false,
        session: None,
    };
    let st = state.get_or_insert_with(fresh);
    if stall.at > st.error_at {
        if st.escalated {
            // The owner nudged it and it failed again: a new episode.
            *st = fresh();
        } else {
            st.error_at = stall.at;
            st.message.clone_from(&stall.message);
        }
    }
    if st.escalated {
        return Step::Idle;
    }
    let waited = now.duration_since(st.last_attempt.unwrap_or(st.since));
    let index = usize::try_from(st.attempts).unwrap_or(usize::MAX);
    if let Some(delay) = BACKOFF_SECS.get(index) {
        if waited < SignedDuration::from_secs(*delay) || !may_retry {
            return Step::Wait;
        }
        st.attempts += 1;
        st.last_attempt = Some(now);
        return Step::Retry(st.attempts);
    }
    let last = BACKOFF_SECS.last().copied().unwrap_or_default();
    // A final retry still running has not failed yet; one that logged a newer error has.
    let attempt_failed = st.last_attempt.is_some_and(|at| st.error_at > at);
    if !may_retry || (waited < SignedDuration::from_secs(last) && !attempt_failed) {
        return Step::Wait;
    }
    st.escalated = true;
    Step::Escalate
}

fn state_path(ctx: &Ctx, key: &str) -> PathBuf {
    ctx.store
        .state_dir()
        .join("provider-stall")
        .join(format!("{key}.json"))
}

fn load(ctx: &Ctx, key: &str) -> Option<State> {
    serde_json::from_slice(&std::fs::read(state_path(ctx, key)).ok()?).ok()
}

fn save(ctx: &Ctx, key: &str, state: &State) -> Result<()> {
    let path = state_path(ctx, key);
    let dir = path.parent().context("state path has no parent")?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec(state)?)
        .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("replacing {}", path.display()))
}

/// Stalled and still inside its retry budget: the owner is not needed yet.
pub(crate) fn retrying(ctx: &Ctx, key: &str) -> bool {
    load(ctx, key).is_some_and(|state| !state.escalated)
}

pub(crate) fn escalated(ctx: &Ctx, key: &str) -> bool {
    load(ctx, key).is_some_and(|state| state.escalated)
}

pub(crate) fn agent_key(task: &str, run: &Run) -> String {
    format!("{task}-{}", run.id)
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Drive {
    Quiet,
    Holding,
    Escalated(String),
}

#[derive(Clone, Copy)]
pub(crate) enum Role {
    Agent,
    Reviewer,
}

pub(crate) struct Target<'a> {
    pub task: &'a str,
    pub key: &'a str,
    pub pane: &'a str,
    pub role: Role,
    pub pane_status: AgentStatus,
    pub run: &'a Run,
    /// Earliest instant a session or error can belong to this run.
    pub boundary: Timestamp,
    pub scope: provider_session::Scope,
}

fn retry_text(role: Role, message: &str, attempt: u32) -> String {
    let ask = match role {
        Role::Agent => "Continue where you left off.",
        Role::Reviewer => {
            "Re-run the review where you left off and submit your verdict as instructed."
        }
    };
    format!(
        "The provider was overloaded ({}) and your last turn failed. {ask} (corral automatic retry {attempt}/{})",
        message.trim(),
        BACKOFF_SECS.len()
    )
}

/// Applies one sync tick: advance and persist the backoff state, send a due retry, and log it all.
pub(crate) fn drive(ctx: &Ctx, target: &Target<'_>, now: Timestamp) -> Result<Drive> {
    let before = load(ctx, target.key);
    let session = before
        .as_ref()
        .and_then(|state| state.session.clone())
        .or_else(|| provider_session::discover(ctx, target.run, target.boundary, target.scope));
    let stall = session
        .as_deref()
        .and_then(read_tail)
        .and_then(|log| scan(&log))
        .filter(|stall| stall.at >= target.boundary);
    let mut state = before.clone();
    let may_retry = target.pane_status != AgentStatus::Working;
    let step = advance_state(&mut state, stall.as_ref(), may_retry, now);
    if let Some(current) = state.as_mut().filter(|current| current.session.is_none()) {
        current.session = session;
    }
    let task = Some(target.task);
    match (&before, &state) {
        (Some(_), None) => {
            let _ = std::fs::remove_file(state_path(ctx, target.key));
            ctx.store.log_event(
                task,
                "provider_recovered",
                json!({"key": target.key, "since": before.as_ref().map(|s| s.since.to_string())}),
            )?;
        }
        (_, Some(current)) if before.as_ref() != Some(current) => {
            save(ctx, target.key, current)?;
            if before.as_ref().map(|s| s.since) != Some(current.since) {
                ctx.store.log_event(
                    task,
                    "provider_stall",
                    json!({
                        "key": target.key, "since": current.since.to_string(),
                        "message": current.message,
                    }),
                )?;
            }
        }
        _ => {}
    }
    let Some(current) = state else {
        return Ok(Drive::Quiet);
    };
    match step {
        Step::Retry(attempt) => {
            let text = retry_text(target.role, &current.message, attempt);
            let delivery = ctx.herdr.agent_prompt(target.pane, &text);
            let (submitted, error) = match &delivery {
                Ok(d) => (d.submitted, d.error.clone()),
                Err(err) => (false, Some(format!("{err:#}"))),
            };
            ctx.store.log_event(
                task,
                "provider_retry",
                json!({
                    "key": target.key, "pane": target.pane, "attempt": attempt,
                    "of": BACKOFF_SECS.len(), "text": text,
                    "submitted": submitted, "error": error,
                }),
            )?;
            Ok(Drive::Holding)
        }
        Step::Wait => Ok(Drive::Holding),
        Step::Escalate => {
            let reason = current.reason();
            ctx.store.log_event(
                task,
                "provider_escalated",
                json!({
                    "key": target.key, "reason": reason, "attempts": current.attempts,
                    "since": current.since.to_string(),
                }),
            )?;
            Ok(Drive::Escalated(reason))
        }
        Step::Idle | Step::Recovered => Ok(Drive::Quiet),
    }
}

/// Task agents: retry a stalled live run; once exhausted move it to Needs me with the real reason.
pub(crate) fn advance_agent(
    ctx: &Ctx,
    task: &Task,
    panes: &HashMap<String, PaneInfo>,
    now: Timestamp,
) -> Result<Option<Change>> {
    let Some(run) = task.current_run().filter(|r| r.outcome.is_none()) else {
        return Ok(None);
    };
    let Some(pane_id) = run.herdr.as_ref().map(|h| h.pane_id.as_str()) else {
        return Ok(None);
    };
    let Some(pane) = panes.get(pane_id) else {
        return Ok(None);
    };
    if run.obligations.pending_decision().is_some() {
        return Ok(None);
    }
    let key = agent_key(&task.id, run);
    let target = Target {
        task: &task.id,
        key: &key,
        pane: pane_id,
        role: Role::Agent,
        pane_status: pane.status,
        run,
        boundary: provider_session::boundary(ctx, &task.id, run),
        scope: provider_session::Scope::Created {
            until: run.review_runs.first().map(|review| review.started),
        },
    };
    let Drive::Escalated(reason) = drive(ctx, &target, now)? else {
        return Ok(None);
    };
    let from = task.status;
    let run_id = run.id.clone();
    let saved = ctx.store.try_update(&task.id, |t| {
        if t.status != from {
            return Ok(false);
        }
        let Some(r) = t
            .current_run_mut()
            .filter(|r| r.id == run_id && r.outcome.is_none())
        else {
            return Ok(false);
        };
        r.blocked_excerpt = Some(reason.clone());
        t.status = Status::NeedsMe;
        Ok(true)
    })?;
    if saved.is_none() || from == Status::NeedsMe {
        return Ok(None);
    }
    if let Err(err) = ctx
        .herdr
        .notify(&format!("▲ {} needs you", task.id), &reason, true)
    {
        crate::messages::report(
            Some(&task.id),
            "notification",
            format!("notification for {} failed: {err:#}", task.id),
        );
    }
    ctx.store.log_event(
        Some(&task.id),
        "needs_me",
        json!({"from": from.as_str(), "agent_state": pane.status.as_str(), "note": reason}),
    )?;
    Ok(Some(Change {
        task: task.id.clone(),
        from,
        to: Status::NeedsMe,
        note: reason,
        stage: None,
    }))
}

/// Pending reviewer: same retry budget, keyed on the reviewer child run.
pub(crate) fn advance_reviewer(
    ctx: &Ctx,
    id: &str,
    child: &Run,
    reused_session: bool,
    pane_id: &str,
    pane: &PaneInfo,
    now: Timestamp,
) -> Result<Drive> {
    let key = agent_key(id, child);
    let target = Target {
        task: id,
        key: &key,
        pane: pane_id,
        role: Role::Reviewer,
        pane_status: pane.status,
        run: child,
        boundary: child.started,
        scope: if reused_session {
            provider_session::Scope::Active
        } else {
            provider_session::Scope::Created { until: None }
        },
    };
    drive(ctx, &target, now)
}

#[cfg(test)]
#[path = "provider_stall_tests.rs"]
mod tests;
