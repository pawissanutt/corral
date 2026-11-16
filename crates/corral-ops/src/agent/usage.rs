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
use corral_core::agent::{self, AgentRole, AgentRunState, ContainerRef, pm_state};
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

/// One structural input span. Queued inputs may replace the span without starting a new turn.
pub(crate) struct CostSpan {
    pub turn: (u32, u64),
    pub key: Option<String>,
    pub queued: bool,
}

/// Responses share their span's key without copying it for every provider request.
pub(crate) struct CostUsageEntry {
    pub usage: UsageEntry,
    pub span: Option<std::sync::Arc<CostSpan>>,
}

/// Bounded app read coverage counts examined transcript lines, not just priced responses.
pub(crate) struct UsageReadCoverage {
    pub scanned_rows: usize,
    pub partial: bool,
    pub reason: Option<&'static str>,
    remaining: Option<usize>,
    observational: bool,
    unlimited_bytes: bool,
}

impl UsageReadCoverage {
    fn new(max: Option<usize>) -> Self {
        Self {
            scanned_rows: 0,
            partial: false,
            reason: None,
            remaining: max,
            observational: false,
            unlimited_bytes: false,
        }
    }

    fn admit(&mut self) -> bool {
        if let Some(remaining) = &mut self.remaining {
            if *remaining == 0 {
                self.partial = true;
                self.reason = Some("transcript_row_limit");
                return false;
            }
            *remaining -= 1;
        }
        self.scanned_rows += 1;
        true
    }
}

pub(crate) fn collect_entries(
    state: &AgentRunState,
    model: Option<&str>,
    store: &Store,
    complete: bool,
) -> Result<Vec<CostUsageEntry>> {
    let (entries, _) = collect_entries_map(state, model, store, complete)?;
    let mut entries: Vec<_> = entries.into_values().collect();
    for entry in &mut entries {
        entry.usage.reprice(&store.config);
    }
    entries.sort_by_key(|entry| entry.usage.at);
    Ok(entries)
}

type CollectedEntries = (
    BTreeMap<(String, String), CostUsageEntry>,
    UsageReadCoverage,
);

fn collect_entries_map(
    state: &AgentRunState,
    model: Option<&str>,
    store: &Store,
    complete: bool,
) -> Result<CollectedEntries> {
    let inputs = read::inputs(state, model, store)?;
    if inputs.files.is_empty() {
        bail!("no bound Pi session");
    }
    let mut entries = BTreeMap::new();
    let mut coverage = UsageReadCoverage::new(None);
    for (path, input) in &inputs.files {
        read_session_with_turns(
            path,
            &input.session,
            input.model.as_deref(),
            &mut entries,
            complete,
            &inputs.bindings,
            &mut coverage,
        )?;
        if coverage.partial {
            break;
        }
    }
    if entries.is_empty() && !coverage.partial {
        bail!("no assistant usage found in the bound Pi sessions");
    }
    Ok((entries, coverage))
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
    details: Option<serde_json::Value>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SummaryUsage {
    summary_profile: Option<String>,
    summary_model: Option<String>,
    summary_provider: Option<String>,
    summary_binding: Option<corral_core::connection::ConnectionBinding>,
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
        &mut UsageReadCoverage::new(None),
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
    coverage: &mut UsageReadCoverage,
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
    let byte_limit = if coverage.unlimited_bytes {
        u64::MAX
    } else {
        SESSION_MAX_BYTES + 1
    };
    let mut reader = BufReader::new(file.take(byte_limit));
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
        if !coverage.admit() {
            break;
        }
        if !coverage.unlimited_bytes && read > SESSION_MAX_BYTES {
            if coverage.observational {
                coverage.partial = true;
                coverage.reason = Some("transcript_byte_limit");
                break;
            }
            bail!("Pi session {display} exceeds {SESSION_MAX_BYTES} bytes");
        }
        // The host may be appending: an unterminated last line is not yet a record.
        if line.last() != Some(&b'\n') {
            if complete {
                bail!(
                    "Pi session {display} has an incomplete trailing line; usage snapshot is incomplete"
                );
            }
            if coverage.observational {
                coverage.partial = true;
                coverage.reason = Some("incomplete_transcript_tail");
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
                    Some(std::sync::Arc::new(CostSpan {
                        turn: (
                            u32::try_from(data["generation"].as_u64()?).ok()?,
                            data["turn"].as_u64()?,
                        ),
                        key: data["key"].as_str().map(str::to_owned),
                        queued: data["queued"].as_bool().unwrap_or(false),
                    }))
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
            // Legacy summaries inherit the current model; quick summaries carry exact request metadata.
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
        let summary = entry
            .details
            .filter(|details| {
                matches!(entry.kind.as_str(), "compaction" | "branch_summary")
                    && details.get("summaryModel").is_some()
            })
            .map(serde_json::from_value::<SummaryUsage>)
            .transpose()
            .context("decoding quick summary usage metadata")?
            .unwrap_or_default();
        let has_summary_route = summary.summary_provider.is_some();
        let message_model = message_model.or(summary.summary_model);
        let message_provider = message_provider.or(summary.summary_provider);
        let binding = if has_summary_route {
            summary.summary_binding.as_ref()
        } else {
            binding
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
            profile: summary.summary_profile,
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
        entries.entry(key).or_insert_with(|| CostUsageEntry {
            usage: evidence,
            span: cost_turn.clone(),
        });
    }
    Ok(())
}

#[cfg(test)]
#[path = "usage_tests.rs"]
mod tests;

#[path = "usage_read.rs"]
pub(crate) mod read;
