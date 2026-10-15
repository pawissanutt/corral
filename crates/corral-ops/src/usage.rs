//! Transcript token evidence, reported costs, and explicitly configured API-equivalent estimates.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use corral_core::{Config, Event, ProfileChoice, Run, Store, Usage, UsageEntry};
use jiff::Timestamp;
use serde::Deserialize;

/// Refresh unfinished runs and backfill older runs. Store updates preserve concurrent edits.
pub(crate) fn collect_missing(store: &Store, since: Option<Timestamp>) -> Result<()> {
    for task in store.list()? {
        refresh_runs(store, &task.id, &task.runs, since)?;
    }
    Ok(())
}

/// Refresh the selected task without scanning unrelated task transcripts.
pub fn refresh_usage(store: &Store, id: &str) -> Result<()> {
    let task = store.get(id)?;
    refresh_runs(store, &task.id, &task.runs, None)
}

fn refresh_runs(store: &Store, id: &str, runs: &[Run], since: Option<Timestamp>) -> Result<()> {
    let events = store.events()?;
    for run in runs {
        if since.is_some_and(|s| run.started < s) {
            continue;
        }
        let mut measured = run.clone();
        backfill_reviews(&mut measured, store, id, &events);
        // Owner actions may finish a review without a harness verdict.
        for review in &mut measured.review_runs {
            if review.ended.is_none()
                && let Some(end) = events
                    .iter()
                    .filter(|event| {
                        event.task.as_deref() == Some(id)
                            && event.detail["run"].as_str() == Some(run.id.as_str())
                            && event.ts >= review.started
                            && matches!(
                                event.kind.as_str(),
                                "verdict" | "approved" | "changes_requested" | "discarded"
                            )
                    })
                    .map(|event| event.ts)
                    .min()
            {
                review.ended = Some(end);
                let previous = review.usage.clone();
                collect(review, store);
                if review.usage.is_none() {
                    review.usage = previous;
                }
            }
        }
        refresh_run(&mut measured, store);
        if measured == *run {
            continue;
        }
        store.update(id, |task| {
            if let Some(current) = task.runs.iter_mut().find(|r| r.id == run.id)
                && current == run
            {
                current.usage = measured.usage.clone();
                current.usage_note = measured.usage_note.clone();
                current.review_runs = measured.review_runs.clone();
            }
            Ok(())
        })?;
    }
    Ok(())
}

/// Older versions kept only the latest review record. Events retain each review window.
fn backfill_reviews(run: &mut Run, store: &Store, task_id: &str, events: &[Event]) {
    if !run.review_runs.is_empty() {
        return;
    }
    let mut reviews: Vec<Run> = Vec::new();
    for event in events.iter().filter(|event| {
        event.task.as_deref() == Some(task_id)
            && event.detail["run"].as_str() == Some(run.id.as_str())
    }) {
        match event.kind.as_str() {
            "review_started" => {
                if let Some(previous) = reviews.last_mut()
                    && previous.ended.is_none()
                {
                    previous.ended = Some(event.ts);
                }
                let Some(reviewer) = event.detail["reviewer"].as_str() else {
                    continue;
                };
                let choice = ProfileChoice {
                    profile: Some(if reviewer == "pm" {
                        store.config.pm.agent.clone()
                    } else {
                        reviewer.to_owned()
                    }),
                    ..ProfileChoice::default()
                };
                let resolved = store.config.resolve_profile(&choice, None, None).ok();
                let mut review = run.clone();
                review.id = format!("{}-review{}", run.id, reviews.len() + 1);
                review.agent = resolved
                    .as_ref()
                    .map_or_else(|| reviewer.to_owned(), |profile| profile.profile.clone());
                review.profile = None;
                review.harness = resolved.as_ref().map(|profile| profile.harness.clone());
                review.model = resolved.as_ref().and_then(|profile| profile.model.clone());
                review.effort = resolved.and_then(|profile| profile.effort);
                review.route = None;
                if reviewer == "pm" {
                    review.worktree = store.root().to_path_buf();
                }
                review.started = event.ts;
                review.ended = None;
                review.outcome = None;
                review.herdr = None;
                review.herdr_idle_since = None;
                review.stuck = None;
                review.pending_reply = None;
                review.review = None;
                review.review_runs.clear();
                review.usage = None;
                review.usage_note = None;
                review.summary = None;
                review.gate = None;
                review.autopilot = None;
                reviews.push(review);
            }
            "verdict" | "approved" | "changes_requested" | "discarded" => {
                if let Some(review) = reviews.last_mut()
                    && review.ended.is_none()
                {
                    review.ended = Some(event.ts);
                }
            }
            _ => {}
        }
    }
    run.review_runs = reviews;
}

fn refresh_run(run: &mut Run, store: &Store) {
    let reread = run.ended.is_none()
        || run.usage.as_ref().is_none_or(|u| u.entries.is_empty())
        || run
            .usage_note
            .as_deref()
            .is_some_and(|n| n.starts_with("unsupported harness"));
    if reread {
        let previous = run.usage.clone();
        collect(run, store);
        if run.usage.is_none() {
            run.usage = previous;
        }
    }
    if let Some(usage) = &mut run.usage {
        usage.reprice(&store.config, run.model.as_deref());
    }
    for review in &mut run.review_runs {
        refresh_run(review, store);
    }
}

fn run_harness<'a>(run: &'a Run, config: &'a Config) -> &'a str {
    run.harness
        .as_deref()
        .or_else(|| config.profiles.get(&run.agent).map(|p| p.harness.as_str()))
        .unwrap_or(&run.agent)
}

pub(crate) fn collect(run: &mut Run, store: &Store) {
    let result = (|| {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .context("HOME is not set")?;
        let harness = run_harness(run, &store.config);
        let (root, claude) = match harness {
            "omo" => (
                crate::omo::agent_dir(store.config.sync.omo_agent_dir.as_deref())
                    .join("sessions")
                    .join(crate::omo::encode_cwd(&run.worktree)),
                false,
            ),
            "omp" => (home.join(".omp/agent/sessions"), false),
            "claude" => (home.join(".claude/projects"), true),
            other => bail!("unsupported harness {other}"),
        };
        collect_from(&root, run, claude, &store.config)
    })();
    match result {
        Ok(usage) => {
            run.usage = Some(usage);
            run.usage_note = None;
        }
        Err(error) => {
            run.usage = None;
            run.usage_note = Some(format!("{error:#}"));
        }
    }
}

/// Only scan regular JSONL files at the session root and one project-directory level.
/// Artifacts, subagent directories, and symlinks are not transcript candidates.
fn collect_from(root: &Path, run: &Run, claude: bool, config: &Config) -> Result<Usage> {
    let mut paths = Vec::new();
    let cwd_slug: String = run
        .worktree
        .to_string_lossy()
        .chars()
        .map(|c| {
            if (claude && !c.is_ascii_alphanumeric()) || matches!(c, '/' | '\\' | ':') {
                '-'
            } else {
                c
            }
        })
        .collect();
    let omo_slug = crate::omo::encode_cwd(&run.worktree);
    // omp 18 uses home-relative names, unlike Pi's absolute legacy buckets.
    // Directory names are only discovery hints; the transcript cwd remains authoritative.
    let home_slug = std::env::var_os("HOME")
        .and_then(|home| {
            run.worktree
                .strip_prefix(PathBuf::from(home))
                .ok()
                .map(Path::to_path_buf)
        })
        .map(|relative| {
            format!(
                "-{}",
                relative.to_string_lossy().replace(['/', '\\', ':'], "-")
            )
        });
    let temp_slug = run
        .worktree
        .strip_prefix(std::env::temp_dir())
        .ok()
        .map(|relative| {
            format!(
                "-tmp-{}",
                relative.to_string_lossy().replace(['/', '\\', ':'], "-")
            )
        });
    for entry in fs::read_dir(root).with_context(|| format!("reading {}", root.display()))? {
        let entry = entry.context("reading session directory entry")?;
        let kind = entry.file_type().context("reading session file type")?;
        if kind.is_file() && entry.path().extension().is_some_and(|e| e == "jsonl") {
            paths.push(entry.path());
        } else if kind.is_dir()
            && (entry.file_name() == cwd_slug.as_str()
                || entry.file_name() == omo_slug.as_str()
                || home_slug
                    .as_ref()
                    .is_some_and(|s| entry.file_name() == s.as_str())
                || temp_slug
                    .as_ref()
                    .is_some_and(|s| entry.file_name() == s.as_str()))
        {
            for child in fs::read_dir(entry.path()).context("reading session project directory")? {
                let child = child.context("reading session project entry")?;
                if child
                    .file_type()
                    .context("reading session file type")?
                    .is_file()
                    && child.path().extension().is_some_and(|e| e == "jsonl")
                {
                    paths.push(child.path());
                }
            }
        }
    }
    paths.sort();
    let mut total = None;
    for path in paths {
        let file = File::open(&path).with_context(|| format!("opening {}", path.display()))?;
        if let Some(usage) = parse_session_priced(BufReader::new(file), run, claude, config)
            .with_context(|| format!("parsing {}", path.display()))?
        {
            add(&mut total, usage)?;
        }
    }
    total.context("no assistant usage found for this worktree and run time window")
}

#[derive(Deserialize)]
struct Entry {
    #[serde(rename = "type")]
    kind: String,
    cwd: Option<PathBuf>,
    timestamp: Option<Timestamp>,
    id: Option<String>,
    message: Option<Message>,
    #[serde(alias = "modelId")]
    model: Option<String>,
    #[serde(rename = "costUSD")]
    cost_usd: Option<f64>,
}

#[derive(Deserialize)]
struct Message {
    role: String,
    id: Option<String>,
    usage: Option<serde_json::Value>,
    model: Option<String>,
}

#[derive(Deserialize)]
struct WireUsage {
    #[serde(alias = "input_tokens")]
    input: u64,
    #[serde(alias = "output_tokens")]
    output: u64,
    #[serde(default, rename = "cacheRead", alias = "cache_read_input_tokens")]
    cache_read: u64,
    #[serde(default, rename = "cacheWrite", alias = "cache_creation_input_tokens")]
    cache_write: u64,
    cost: Option<Cost>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Cost {
    Number(f64),
    Breakdown { total: f64 },
}

/// Pi-family usage is per assistant entry. Claude fragments repeat a message's usage:
/// retain the last snapshot per provider message id rather than billing every fragment.
fn parse_session_priced(
    reader: impl BufRead,
    run: &Run,
    claude: bool,
    config: &Config,
) -> Result<Option<Usage>> {
    let end = run.ended.unwrap_or_else(Timestamp::now);
    let mut matched_cwd = false;
    let mut messages = BTreeMap::new();
    let mut model = run.model.clone();
    for (index, line) in reader.lines().enumerate() {
        let line = line.context("reading session line")?;
        if line.trim().is_empty() {
            continue;
        }
        let entry: Entry = serde_json::from_str(&line)
            .with_context(|| format!("unrecognized session format at line {}", index + 1))?;
        if let Some(cwd) = entry.cwd {
            if cwd != run.worktree {
                return Ok(None);
            }
            matched_cwd = true;
        }
        if entry.kind == "model_change" {
            model = entry.model;
            continue;
        }
        if !matched_cwd || !matches!(entry.kind.as_str(), "message" | "assistant") {
            continue;
        }
        let Some(message) = entry.message.filter(|m| m.role == "assistant") else {
            continue;
        };
        let at = entry
            .timestamp
            .context("assistant message has no timestamp")?;
        if at < run.started || at > end {
            continue;
        }
        let usage: WireUsage = serde_json::from_value(
            message
                .usage
                .context("assistant message has no recognized usage")?,
        )
        .context("unrecognized assistant usage format")?;
        let cost_usd = usage
            .cost
            .map(|cost| match cost {
                Cost::Number(value) | Cost::Breakdown { total: value } => value,
            })
            .or(entry.cost_usd);
        if cost_usd.is_some_and(|cost| !cost.is_finite() || cost < 0.0) {
            bail!("invalid usage cost");
        }
        let key = if claude {
            message
                .id
                .context("Claude usage has no provider message id")?
        } else {
            entry.id.unwrap_or_else(|| index.to_string())
        };
        let mut evidence = UsageEntry {
            model: message.model.or(entry.model).or_else(|| model.clone()),
            input: usage.input,
            output: usage.output,
            cache_read: usage.cache_read,
            cache_write: usage.cache_write,
            reported_usd: cost_usd,
            ..UsageEntry::default()
        };
        evidence.reprice(config);
        messages.insert(key, Usage::from_entries(vec![evidence]));
    }
    let mut total = None;
    for usage in messages.into_values() {
        add(&mut total, usage)?;
    }
    Ok(total)
}

fn add(total: &mut Option<Usage>, usage: Usage) -> Result<()> {
    let Some(sum) = total else {
        *total = Some(usage);
        return Ok(());
    };
    sum.input = sum
        .input
        .checked_add(usage.input)
        .context("input token overflow")?;
    sum.output = sum
        .output
        .checked_add(usage.output)
        .context("output token overflow")?;
    sum.cache_read = sum
        .cache_read
        .checked_add(usage.cache_read)
        .context("cache read overflow")?;
    sum.cache_write = sum
        .cache_write
        .checked_add(usage.cache_write)
        .context("cache write overflow")?;
    let mut entries = std::mem::take(&mut sum.entries);
    entries.extend(usage.entries);
    *sum = Usage::from_entries(entries);
    if [sum.reported_usd, sum.estimated_usd]
        .into_iter()
        .flatten()
        .any(|cost| !cost.is_finite())
    {
        bail!("usage cost overflow");
    }
    Ok(())
}

#[cfg(test)]
#[path = "usage_tests.rs"]
mod tests;
