//! corral-pi usage from the run's bound Pi session files, priced at API-equivalent rates and
//! deduplicated by (session ID, response ID). Owner: T-252 child pi-usage.
//!
//! The collector reads the `session_file` each generation of a worker run or review child bound at
//! hello, so a worker's usage lands on its run and a reviewer's on its review child. It is a pure
//! function of those files: collecting twice, or after a resume that rebinds or copies the same
//! session, yields the same `Usage`. `openai-codex/<model>` is priced as `<model>` through corral
//! pricing; the API-equivalent figure is `estimated_usd` and Pi's own reported cost stays in
//! `reported_usd`. PM sessions are bound to the `AgentPm` record, never to a run, so they
//! remain excluded from implementation-run comparisons. PM cost reports separately join
//! responses to durable `corral_cost_turn` transcript boundaries, never timestamp proximity.
//! Auxiliary references never authorize another host or replace a bound transcript.
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
    } else if let ContainerRef::Assistant { session } = container {
        let assistant = store.get_assistant_container(session)?;
        collect_state(&assistant.state, None, store)
    } else {
        let pm = pm_state::load(store)?.context("no bound Pi PM")?;
        collect_state(&pm.state, None, store)
    }
}

/// Shared accounting for worker, reviewer, PM, planner and plan-reviewer records. The caller
/// selects the durable container, never a directory scan. All generations and auxiliary requests
/// share one response identity map, so copies/resumes cannot multiply spending.
pub fn collect_state(state: &AgentRunState, model: Option<&str>, store: &Store) -> Result<Usage> {
    collect_state_inner(state, model, store, false)
}

/// Finish requires a complete snapshot, rather than silently reporting a partial total.
pub(crate) fn collect_state_complete(state: &AgentRunState, store: &Store) -> Result<Usage> {
    collect_state_inner(state, None, store, true)
}

fn collect_state_inner(
    state: &AgentRunState,
    model: Option<&str>,
    store: &Store,
    complete: bool,
) -> Result<Usage> {
    Ok(Usage::from_entries(
        collect_entries(state, model, store, complete)?
            .into_iter()
            .map(|entry| entry.usage)
            .collect(),
    ))
}

/// Response evidence with a structural turn identity, absent in legacy/auxiliary transcripts.
pub(crate) struct CostUsageEntry {
    pub usage: UsageEntry,
    pub turn: Option<(u32, u64)>,
}

pub(crate) fn collect_entries(
    state: &AgentRunState,
    model: Option<&str>,
    store: &Store,
    complete: bool,
) -> Result<Vec<CostUsageEntry>> {
    // Generations that resumed the same file are read once; copies are deduplicated by key.
    // Retired PM generations keep their transcripts in the archive; both are read in
    // generation order, so retirement never changes which binding claims a file first.
    let retired = state.retired.as_ref();
    let mut bound: Vec<(u32, &Path, &str)> = state
        .generations
        .iter()
        .filter_map(|g| {
            let b = g.bound.as_ref()?;
            Some((g.n, b.session_file.as_path(), b.session_id.as_str()))
        })
        .chain(retired.into_iter().flat_map(|r| {
            r.transcripts.iter().map(|t| {
                (
                    t.generation,
                    t.session_file.as_path(),
                    t.session_id.as_str(),
                )
            })
        }))
        .collect();
    bound.sort_by_key(|(n, ..)| *n);
    let mut files: BTreeMap<&Path, (&str, Option<&str>)> = BTreeMap::new();
    for (_, file, session_id) in bound {
        files.entry(file).or_insert((session_id, model));
    }
    for session in &state.usage_sessions {
        if state.generation(session.generation).is_none()
            && !state.generation_retired(session.generation)
        {
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
    let bindings: BTreeMap<_, _> = retired
        .into_iter()
        .flat_map(|r| {
            r.connections
                .iter()
                .flat_map(|c| c.generations.iter().map(move |n| (*n, c.binding.clone())))
        })
        .chain(
            state
                .generations
                .iter()
                .filter_map(|g| g.connection.clone().map(|b| (g.n, b))),
        )
        .collect();
    for binding in bindings.values() {
        binding.validate()?;
    }
    let mut entries = BTreeMap::new();
    for (path, (session_id, model)) in files {
        read_session_with_turns(path, session_id, model, &mut entries, complete, &bindings)?;
    }
    if entries.is_empty() {
        bail!("no assistant usage found in the bound Pi sessions");
    }
    let mut entries: Vec<_> = entries.into_values().collect();
    for entry in &mut entries {
        entry.usage.reprice(&store.config);
    }
    entries.sort_by_key(|entry| entry.usage.at);
    Ok(entries)
}

#[derive(Deserialize)]
struct Line {
    #[serde(rename = "type")]
    kind: String,
    id: Option<String>,
    timestamp: Option<Timestamp>,
    #[serde(rename = "modelId")]
    model_id: Option<String>,
    provider: Option<String>,
    #[serde(rename = "customType")]
    custom_type: Option<String>,
    data: Option<serde_json::Value>,
    message: Option<Message>,
    /// A compaction or branch summary's own request usage, recorded on its entry.
    usage: Option<WireUsage>,
}

#[derive(Deserialize)]
struct Message {
    role: String,
    provider: Option<String>,
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
#[cfg(test)]
fn read_session(
    path: &Path,
    bound_session: &str,
    run_model: Option<&str>,
    entries: &mut BTreeMap<(String, String), UsageEntry>,
    complete: bool,
    bindings: &BTreeMap<u32, corral_core::connection::ConnectionBinding>,
) -> Result<()> {
    let mut detailed = BTreeMap::new();
    let result = read_session_with_turns(
        path,
        bound_session,
        run_model,
        &mut detailed,
        complete,
        bindings,
    );
    entries.extend(detailed.into_iter().map(|(key, entry)| (key, entry.usage)));
    result
}

fn read_session_with_turns(
    path: &Path,
    bound_session: &str,
    run_model: Option<&str>,
    entries: &mut BTreeMap<(String, String), CostUsageEntry>,
    complete: bool,
    bindings: &BTreeMap<u32, corral_core::connection::ConnectionBinding>,
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
    let mut provider = None;
    let mut binding = None;
    let mut cost_turn = None;
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
            if complete {
                bail!(
                    "Pi session {display} has an incomplete trailing line; usage snapshot is incomplete"
                );
            }
            break;
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let entry: Line = serde_json::from_slice(&line)
            .with_context(|| format!("unrecognized Pi session line {number} in {display}"))?;
        let preceding_at = std::mem::replace(&mut previous_at, entry.timestamp);
        match entry.kind.as_str() {
            "custom" if entry.custom_type.as_deref() == Some("corral_cost_turn") => {
                cost_turn = entry.data.as_ref().and_then(|data| {
                    if data["phase"].as_str() != Some("start") {
                        return None;
                    }
                    Some((
                        u32::try_from(data["generation"].as_u64()?).ok()?,
                        data["turn"].as_u64()?,
                    ))
                });
                continue;
            }
            "custom" if entry.custom_type.as_deref() == Some("corral_host_bound") => {
                cost_turn = None;
                binding = entry
                    .data
                    .as_ref()
                    .and_then(|d| d["generation"].as_u64())
                    .and_then(|n| u32::try_from(n).ok())
                    .and_then(|n| bindings.get(&n));
                continue;
            }
            "session" => {
                cost_turn = None;
                if let Some(id) = entry.id {
                    session_id = id;
                }
                continue;
            }
            "model_change" => {
                provider = entry.provider;
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
        let (usage, response_id, message_model, message_provider) = match entry.message {
            Some(message) if entry.kind == "message" && message.role == "assistant" => (
                message.usage,
                message.response_id,
                message.model,
                message.provider,
            ),
            _ if entry.kind != "message" => (entry.usage, None, None, None),
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
        let connection = binding.map(|b| b.connection).or_else(|| {
            match message_provider.as_deref().or(provider.as_deref()) {
                Some("openai-codex") => {
                    Some(corral_core::connection::ConnectionId::OpenaiSubscription)
                }
                Some(vendor @ ("anthropic" | "amazon-bedrock" | "llama.cpp")) => {
                    corral_core::connection::ConnectionId::legacy(vendor)
                }
                _ => None,
            }
        });
        let evidence = UsageEntry {
            connection,
            billing: connection.map(|c| c.billing()),
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
        let key = (
            session_id.clone(),
            format!("{}:{response}", connection.map_or("legacy", |c| c.id())),
        );
        entries.entry(key).or_insert(CostUsageEntry {
            usage: evidence,
            turn: cost_turn,
        });
    }
    Ok(())
}

#[cfg(test)]
#[path = "usage_tests.rs"]
mod tests;
