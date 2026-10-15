//! Best-effort accounting from local harness transcripts. Never infer dollar prices.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use corral_core::{Config, Run, Store, Usage};
use jiff::Timestamp;
use serde::Deserialize;

/// Refresh unfinished runs and backfill older runs. Store updates preserve concurrent edits.
pub(crate) fn collect_missing(store: &Store, since: Option<Timestamp>) -> Result<()> {
    for task in store.list()? {
        for run in &task.runs {
            if since.is_some_and(|s| run.started < s)
                || (run.usage.is_some()
                    && run.ended.is_some()
                    && !run
                        .usage_note
                        .as_deref()
                        .is_some_and(|note| note.starts_with("unsupported harness")))
            {
                continue;
            }
            let mut measured = run.clone();
            collect(&mut measured, store);
            if measured.usage == run.usage && measured.usage_note == run.usage_note {
                continue;
            }
            if let Err(error) = store.update(&task.id, |task| {
                if let Some(current) = task.runs.iter_mut().find(|r| r.id == run.id)
                    && current.started == run.started
                    && current.ended == run.ended
                    && current.usage == run.usage
                {
                    current.usage = measured.usage.clone();
                    current.usage_note = measured.usage_note.clone();
                }
                Ok(())
            }) {
                eprintln!(
                    "corral: usage for {} {} not saved: {error:#}",
                    task.id, run.id
                );
            }
        }
    }
    Ok(())
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
        collect_from(&root, run, claude)
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
fn collect_from(root: &Path, run: &Run, claude: bool) -> Result<Usage> {
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
    for entry in fs::read_dir(root).with_context(|| format!("reading {}", root.display()))? {
        let entry = entry.context("reading session directory entry")?;
        let kind = entry.file_type().context("reading session file type")?;
        if kind.is_file() && entry.path().extension().is_some_and(|e| e == "jsonl") {
            paths.push(entry.path());
        } else if kind.is_dir()
            && (entry.file_name() == cwd_slug.as_str() || entry.file_name() == omo_slug.as_str())
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
        if let Some(usage) = parse_session(BufReader::new(file), run, claude)
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
    #[serde(rename = "costUSD")]
    cost_usd: Option<f64>,
}

#[derive(Deserialize)]
struct Message {
    role: String,
    id: Option<String>,
    usage: Option<serde_json::Value>,
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
fn parse_session(reader: impl BufRead, run: &Run, claude: bool) -> Result<Option<Usage>> {
    let end = run.ended.unwrap_or_else(Timestamp::now);
    let mut matched_cwd = false;
    let mut messages = BTreeMap::new();
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
        messages.insert(
            key,
            Usage {
                input: usage.input,
                output: usage.output,
                cache_read: usage.cache_read,
                cache_write: usage.cache_write,
                cost_usd,
            },
        );
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
    // A partial dollar sum is not the cost of the run.
    sum.cost_usd = sum.cost_usd.zip(usage.cost_usd).map(|(a, b)| a + b);
    if sum.cost_usd.is_some_and(|cost| !cost.is_finite()) {
        bail!("usage cost overflow");
    }
    Ok(())
}

#[cfg(test)]
#[path = "usage_tests.rs"]
mod tests;
