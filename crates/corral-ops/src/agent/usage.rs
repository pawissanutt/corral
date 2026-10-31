//! Pi-native usage from the run's bound Pi session files, priced at API-equivalent rates and
//! deduplicated by (session ID, response ID). Owner: T-252 child pi-usage.
//!
//! The collector reads the `session_file` each generation of a worker run or review child bound at
//! hello, so a worker's usage lands on its run and a reviewer's on its review child. It is a pure
//! function of those files: collecting twice, or after a resume that rebinds or copies the same
//! session, yields the same `Usage`. `openai-codex/<model>` is priced as `<model>` through corral
//! pricing; the API-equivalent figure is `estimated_usd` and Pi's own reported cost stays in
//! `reported_usd`. PM sessions are bound to the `AgentPm` record, never to a run, so they are
//! attributed to no implementation task and are excluded from the Pi-native vs OmO comparison.
//! The shared container readback also covers PM/planning, plus durable auxiliary references;
//! references never authorize another host or replace a bound transcript.
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, ErrorKind, Read};
use std::path::Path;

use anyhow::{Context, Result, bail};
use corral_core::agent::{self, AgentRunState, ContainerRef, pm_state};
use corral_core::{Run, Store, Usage, UsageEntry};
use jiff::Timestamp;
use serde::Deserialize;

/// The T-252 child that lands the collector.
pub const OWNER: &str = "pi-usage";

/// Largest session file the collector reads.
pub const SESSION_MAX_BYTES: u64 = 256 * 1024 * 1024;

/// The provider prefix normalized away before a pricing lookup.
const CODEX_PREFIX: &str = "openai-codex/";

/// The corral pricing key of a Pi model ID: `openai-codex/<model>` becomes `<model>`.
pub fn pricing_key(model: &str) -> &str {
    model.strip_prefix(CODEX_PREFIX).unwrap_or(model)
}

pub fn collect(run: &Run, store: &Store) -> Result<Usage> {
    let state = run
        .agent_host
        .as_deref()
        .context("no bound Pi session: the run has no host binding")?;
    collect_state(state, run.model.as_deref(), store)
}

/// Read a named container without attributing PM/planning cost to implementation runs.
/// This is an offline readback; it grants no role or session replacement authority.
pub fn collect_container(container: &ContainerRef, store: &Store) -> Result<Usage> {
    if let Some(task) = container.task() {
        let task = store.get(task)?;
        let state = agent::container_state(&task, container).context("no bound Pi container")?;
        let model = match container {
            ContainerRef::Planner { run, .. } => task
                .plan_runs
                .iter()
                .find(|candidate| candidate.id == *run)
                .and_then(|run| run.model.as_deref()),
            _ => agent::container_run(&task, container).and_then(|run| run.model.as_deref()),
        };
        collect_state(state, model, store)
    } else {
        let pm = pm_state::load(store)?.context("no bound Pi PM")?;
        collect_state(&pm.state, None, store)
    }
}

/// Shared accounting for worker, reviewer, PM, planner and plan-reviewer records. The caller
/// selects the durable container, never a directory scan. All generations and auxiliary requests
/// share one response identity map, so copies/resumes cannot multiply spending.
pub fn collect_state(state: &AgentRunState, model: Option<&str>, store: &Store) -> Result<Usage> {
    // Generations that resumed the same file are read once; copies are deduplicated by key.
    let mut files: BTreeMap<&Path, (&str, Option<&str>)> = BTreeMap::new();
    for bound in state.generations.iter().filter_map(|g| g.bound.as_ref()) {
        files
            .entry(bound.session_file.as_path())
            .or_insert((bound.session_id.as_str(), model));
    }
    for session in &state.usage_sessions {
        if state.generation(session.generation).is_none() {
            bail!(
                "usage session references absent generation {}",
                session.generation
            );
        }
        files
            .entry(session.session_file.as_path())
            .or_insert((session.session_id.as_str(), session.model.as_deref()));
    }
    if files.is_empty() {
        bail!("no bound Pi session");
    }
    let mut entries = BTreeMap::new();
    for (path, (session_id, model)) in files {
        read_session(path, session_id, model, &mut entries)?;
    }
    if entries.is_empty() {
        bail!("no assistant usage found in the bound Pi sessions");
    }
    let mut entries: Vec<UsageEntry> = entries.into_values().collect();
    entries.sort_by_key(|entry| entry.at);
    for entry in &mut entries {
        entry.reprice(&store.config);
    }
    Ok(Usage::from_entries(entries))
}

#[derive(Deserialize)]
struct Line {
    #[serde(rename = "type")]
    kind: String,
    id: Option<String>,
    timestamp: Option<Timestamp>,
    #[serde(rename = "modelId")]
    model_id: Option<String>,
    message: Option<Message>,
    /// A compaction or branch summary's own request usage, recorded on its entry.
    usage: Option<WireUsage>,
}

#[derive(Deserialize)]
struct Message {
    role: String,
    model: Option<String>,
    #[serde(rename = "responseId")]
    response_id: Option<String>,
    usage: Option<WireUsage>,
}

#[derive(Deserialize)]
struct WireUsage {
    input: u64,
    output: u64,
    #[serde(default, rename = "cacheRead")]
    cache_read: u64,
    #[serde(default, rename = "cacheWrite")]
    cache_write: u64,
    cost: Option<WireCost>,
}

#[derive(Deserialize)]
struct WireCost {
    total: Option<f64>,
}

/// Adds the file's assistant responses to `entries`, keyed by (session ID, response ID). A file a
/// bound generation has not written yet (Pi writes on the first response) contributes nothing.
fn read_session(
    path: &Path,
    bound_session: &str,
    run_model: Option<&str>,
    entries: &mut BTreeMap<(String, String), UsageEntry>,
) -> Result<()> {
    let display = path.display();
    let meta = match fs::symlink_metadata(path) {
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        meta => meta.with_context(|| format!("reading {display}"))?,
    };
    if !meta.file_type().is_file() {
        bail!("Pi session {display} is not a regular file");
    }
    let file = File::open(path).with_context(|| format!("opening {display}"))?;
    if !file
        .metadata()
        .with_context(|| format!("reading {display}"))?
        .is_file()
    {
        bail!("Pi session {display} is not a regular file");
    }
    let mut reader = BufReader::new(file.take(SESSION_MAX_BYTES + 1));
    let mut session_id = bound_session.to_owned();
    let mut model = run_model.map(str::to_owned);
    let mut previous_at = None;
    let mut read = 0u64;
    let mut line = Vec::new();
    for number in 1usize.. {
        line.clear();
        let n = reader
            .read_until(b'\n', &mut line)
            .with_context(|| format!("reading {display}"))?;
        if n == 0 {
            break;
        }
        read += u64::try_from(n).context("session length overflow")?;
        if read > SESSION_MAX_BYTES {
            bail!("Pi session {display} exceeds {SESSION_MAX_BYTES} bytes");
        }
        // The host may be appending: an unterminated last line is not yet a record.
        if line.last() != Some(&b'\n') {
            break;
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let entry: Line = serde_json::from_slice(&line)
            .with_context(|| format!("unrecognized Pi session line {number} in {display}"))?;
        let preceding_at = std::mem::replace(&mut previous_at, entry.timestamp);
        match entry.kind.as_str() {
            "session" => {
                if let Some(id) = entry.id {
                    session_id = id;
                }
                continue;
            }
            "model_change" => {
                if entry.model_id.is_some() {
                    model = entry.model_id;
                }
                continue;
            }
            // Summarization requests run on the session's current model (T-564).
            "compaction" | "branch_summary" => {}
            "message" => {}
            _ => continue,
        }
        let (usage, response_id, message_model) = match entry.message {
            Some(message) if entry.kind == "message" && message.role == "assistant" => {
                (message.usage, message.response_id, message.model)
            }
            _ if entry.kind != "message" => (entry.usage, None, None),
            _ => continue,
        };
        let Some(usage) = usage else {
            continue;
        };
        let reported_usd = usage.cost.and_then(|cost| cost.total);
        if reported_usd.is_some_and(|cost| !cost.is_finite() || cost < 0.0) {
            bail!("invalid usage cost at line {number} of {display}");
        }
        let response = match (response_id, entry.id) {
            (Some(response), _) => format!("response:{response}"),
            (None, Some(id)) => format!("entry:{id}"),
            (None, None) => format!("line:{number}"),
        };
        let at = entry.timestamp;
        let latency_ms = match (preceding_at, at) {
            (Some(previous), Some(at)) if previous <= at => Some(
                u64::try_from(at.duration_since(previous).as_millis())
                    .context("response latency overflow")?,
            ),
            _ => None,
        };
        let evidence = UsageEntry {
            at,
            latency_ms,
            model: message_model
                .or_else(|| model.clone())
                .map(|m| pricing_key(&m).to_owned()),
            input: usage.input,
            output: usage.output,
            cache_read: usage.cache_read,
            cache_write: usage.cache_write,
            reported_usd,
            ..UsageEntry::default()
        };
        entries
            .entry((session_id.clone(), response))
            .or_insert(evidence);
    }
    Ok(())
}

#[cfg(test)]
#[path = "usage_tests.rs"]
mod tests;
