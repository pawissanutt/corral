//! Transcript token evidence, reported costs, and explicitly configured API-equivalent estimates.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use corral_core::backend::HeadQuery;
use corral_core::{Config, Event, ProfileChoice, Run, Status, Store, TaskHead, Usage, UsageEntry};
use jiff::Timestamp;
use serde::Deserialize;

pub mod pages;
mod transcript_cache;

#[cfg(test)]
thread_local! {
    static EVENT_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn usage_events(store: &Store, task: &str) -> Result<Vec<Event>> {
    // Count task-scoped indexed requests.
    #[cfg(test)]
    EVENT_READS.set(EVENT_READS.get() + 1);
    store.events_of(
        Some(task),
        &[
            "review_started",
            "verdict",
            "approved",
            "changes_requested",
            "discarded",
        ],
    )
}

/// Usage belongs to the assistant, never an implementation run. Reuse the shared collector
/// over its durable generation references; no transcript or collector evidence means unknown.
pub fn assistant_usage(
    store: &Store,
    session: &corral_core::assistant::AssistantSession,
) -> Result<Option<Usage>> {
    let container = store.get_assistant_container(&session.id)?;
    Ok(
        crate::agent::usage::collect_state(&container.state, None, store)
            .ok()
            .or_else(|| session.usage.clone()),
    )
}

/// Refresh unfinished runs and backfill older runs. Store updates preserve concurrent edits.
pub(crate) fn collect_missing(store: &Store, since: Option<Timestamp>) -> Result<()> {
    for task in store.list()? {
        let events = usage_events(store, &task.id)?;
        refresh_runs(store, &task.id, &task.runs, since, &events)?;
    }
    Ok(())
}

/// Refresh the selected task without scanning unrelated task transcripts.
pub fn refresh_usage(store: &Store, id: &str) -> Result<()> {
    let task = store.get(id)?;
    let events = usage_events(store, &task.id)?;
    refresh_runs(store, &task.id, &task.runs, None, &events)
}

/// Refresh the current run (and its reviews) of every unfinished task, for the board's background update.
/// Finished tasks keep the usage collected when they finished. A failure is reported through the message sink
/// (stderr in CLI use) and never stops the other tasks.
///
/// Open heads replace the board listing, and each active task's current run is refreshed as an
/// own-record pass over that run's rows (plan T-253 section 4, "Usage refresh"): terminal
/// tasks, other tasks, the task's earlier runs and other runs' events are never read.
pub fn refresh_active_usage(store: &Store) {
    let _hot = corral_core::work::HotScope::enter();
    let refreshed = transcript_cache::refresh(|| {
        let heads = match store.heads(&HeadQuery::Open { project: None }) {
            Ok(heads) => heads,
            Err(error) => {
                corral_core::report_message(None, "Usage not refreshed", format!("{error:#}"));
                return;
            }
        };
        for head in heads {
            if !matches!(
                head.status,
                Status::Running | Status::NeedsMe | Status::Review
            ) {
                continue;
            }
            if let Err(error) = refresh_current(store, &head) {
                corral_core::report_message(
                    Some(&head.id),
                    "Usage not refreshed",
                    format!("{}: {error:#}", head.id),
                );
            }
        }
    });
    if let Err(error) = refreshed {
        corral_core::report_message(None, "Usage not refreshed", format!("{error:#}"));
    }
}

const VERDICT_KINDS: [&str; 4] = ["verdict", "approved", "changes_requested", "discarded"];

/// `refresh_runs`' algorithm on the current run alone, reading that run's own records and its
/// own events; the write is guarded by the whole assembled run, as the full form was.
fn refresh_current(store: &Store, head: &TaskHead) -> Result<()> {
    let Some(run_id) = head.current_run.as_ref().map(|run| run.id.clone()) else {
        return Ok(());
    };
    let _own = corral_core::work::own_records("usage_refresh", &head.id, &run_id);
    let Some(run) = head.current_run_record()? else {
        return Ok(());
    };
    let mut measured = run.clone();
    if measured.review_runs.is_empty() {
        #[cfg(test)]
        EVENT_READS.set(EVENT_READS.get() + 1);
        let mut kinds = vec!["review_started"];
        kinds.extend(VERDICT_KINDS);
        let events = store.events_of_run(Some(&head.id), &kinds, Some(&run.id), 0, u32::MAX)?;
        backfill_reviews(&mut measured, store, &head.id, &events);
    }
    for review in &mut measured.review_runs {
        if review.ended.is_none()
            && let Some(end) =
                first_verdict_from(store, &head.id, &run.id, &review.id, review.started)?
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
    if measured == run {
        return Ok(());
    }
    store.try_update_head(&head.id, |current| {
        if current
            .head()
            .current_run
            .as_ref()
            .is_none_or(|r| r.id != run.id)
        {
            return Ok(false);
        }
        current.edit_current_run(&run.id, |stored| {
            if *stored != run {
                return Ok(false);
            }
            stored.usage = measured.usage.clone();
            stored.usage_note = measured.usage_note.clone();
            stored.review_runs = measured.review_runs.clone();
            Ok(true)
        })
    })?;
    Ok(())
}

/// The earliest verdict time at or after a review child's start, kept with the event position
/// through which the run's verdict events were observed.
#[derive(Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
struct VerdictWatch {
    #[serde(default, with = "crate::settling::padded")]
    after: Option<u64>,
    first: Option<Timestamp>,
}

const VERDICTS: corral_core::records::RecordArea<VerdictWatch> =
    corral_core::records::RecordArea::new("usage-verdicts");

/// The earliest time of the run's verdict events at or after `from` (an owner action may finish
/// a review without a harness verdict), as the full selection over every verdict event of the
/// run takes it, whatever order their times have. Each call reads only the verdict events
/// appended after the child's kept position; the first one reads the run's earlier ones once.
fn first_verdict_from(
    store: &Store,
    task: &str,
    run: &str,
    review: &str,
    from: Timestamp,
) -> Result<Option<Timestamp>> {
    #[cfg(test)]
    EVENT_READS.set(EVENT_READS.get() + 1);
    let key = format!("{task}/{run}/{review}");
    let saved = VERDICTS.load(store, &key)?.unwrap_or_default();
    let mut watch = VerdictWatch {
        after: saved.after,
        first: saved.first,
    };
    let mut after = saved.after.unwrap_or(0);
    loop {
        let page = store.events_of_run(Some(task), &VERDICT_KINDS, Some(run), after, 64)?;
        let Some(last) = page.last() else {
            break;
        };
        after = last.seq;
        watch.after = Some(after);
        for event in page.iter().filter(|event| event.ts >= from) {
            watch.first = Some(watch.first.map_or(event.ts, |first| first.min(event.ts)));
        }
    }
    if watch != saved {
        VERDICTS.save(store, &key, &watch)?;
    }
    Ok(watch.first)
}

fn refresh_runs(
    store: &Store,
    id: &str,
    runs: &[Run],
    since: Option<Timestamp>,
    events: &[Event],
) -> Result<()> {
    for run in runs {
        if since.is_some_and(|s| run.started < s) {
            continue;
        }
        let mut measured = run.clone();
        backfill_reviews(&mut measured, store, id, events);
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
    if run.usage_pending {
        // The collection a finished reviewer's usage was owed: its result as-is, as at finish.
        collect(run, store);
        run.usage_pending = false;
        if let Some(usage) = &mut run.usage {
            usage.reprice(&store.config, run.model.as_deref());
        }
        for review in &mut run.review_runs {
            refresh_run(review, store);
        }
        return;
    }
    let reread = run.ended.is_none()
        || run.usage.as_ref().is_none_or(|u| u.entries.is_empty())
        || run
            .usage_note
            .as_deref()
            .is_some_and(|n| n.starts_with("unsupported harness"));
    let backfill_timing = run
        .usage
        .as_ref()
        .is_some_and(|usage| usage.entries.iter().any(|entry| entry.at.is_none()));
    if reread || backfill_timing {
        let mut previous = run.usage.clone();
        let previous_note = run.usage_note.clone();
        collect(run, store);
        if !reread {
            if let (Some(previous), Some(measured)) = (&mut previous, &run.usage) {
                // Legacy entries have no response IDs. Only unique token/model/cost
                // evidence can be matched; duplicate fingerprints remain untimed.
                let fingerprint = |entry: &UsageEntry| {
                    (
                        entry.model.clone(),
                        [
                            entry.input,
                            entry.output,
                            entry.cache_read,
                            entry.cache_write,
                        ],
                        entry.reported_usd.map(f64::to_bits),
                    )
                };
                let mut previous_indices = BTreeMap::new();
                for (index, entry) in previous.entries.iter().enumerate() {
                    previous_indices
                        .entry(fingerprint(entry))
                        .and_modify(|index| *index = None)
                        .or_insert(Some(index));
                }
                let mut measured_entries = BTreeMap::new();
                for entry in &measured.entries {
                    measured_entries
                        .entry(fingerprint(entry))
                        .and_modify(|entry| *entry = None)
                        .or_insert(Some(entry));
                }
                for (fingerprint, index) in previous_indices {
                    if let Some(index) = index
                        && let Some(Some(measured)) = measured_entries.get(&fingerprint)
                        && previous.entries[index].at.is_none()
                    {
                        previous.entries[index].at = measured.at;
                        previous.entries[index].latency_ms = measured.latency_ms;
                    }
                }
            }
            run.usage = previous;
            run.usage_note = previous_note;
        } else if run.usage.is_none() {
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
    harness_of(run.harness.as_deref(), &run.agent, config)
}

fn harness_of<'a>(harness: Option<&'a str>, agent: &'a str, config: &'a Config) -> &'a str {
    harness
        .or_else(|| config.profiles.get(agent).map(|p| p.harness.as_str()))
        .unwrap_or(agent)
}

pub(crate) fn collect(run: &mut Run, store: &Store) {
    let result = if crate::agent::hooks::run_is_pi_native(run) {
        crate::agent::hooks::usage::collect(run, store)
    } else {
        (|| {
            let (root, claude) = session_root(run, store)?;
            collect_from(&root, run, claude, &store.config)
        })()
    };
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

fn session_root(run: &Run, store: &Store) -> Result<(PathBuf, bool)> {
    if let Some(session) = &run.session
        && (session.transcript.is_some()
            || std::fs::read_dir(&session.directory).is_ok_and(|entries| {
                entries
                    .filter_map(Result::ok)
                    .any(|entry| entry.path().extension().is_some_and(|ext| ext == "jsonl"))
            }))
    {
        let harness = store
            .config
            .harnesses
            .get(&session.harness)
            .and_then(|h| h.flags.as_deref())
            .unwrap_or(&session.harness);
        return Ok((session.directory.clone(), harness == "claude"));
    }
    session_root_of(run_harness(run, &store.config), &run.worktree, store)
}

fn session_root_of(harness: &str, worktree: &Path, store: &Store) -> Result<(PathBuf, bool)> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")?;
    match harness {
        "omo" => Ok((
            crate::omo::agent_dir(store.config.sync.omo_agent_dir.as_deref())
                .join("sessions")
                .join(crate::omo::encode_cwd(worktree)),
            false,
        )),
        "omp" => Ok((home.join(".omp/agent/sessions"), false)),
        "claude" => Ok((home.join(".claude/projects"), true)),
        other => bail!("unsupported harness {other}"),
    }
}

/// Only scan regular JSONL files at the session root and one project-directory level.
/// Artifacts, subagent directories, and symlinks are not transcript candidates.
fn session_paths(root: &Path, run: &Run, claude: bool) -> Result<Vec<PathBuf>> {
    if let Some(path) = run
        .session
        .as_ref()
        .and_then(|session| session.transcript.as_ref())
    {
        return Ok(vec![path.clone()]);
    }
    session_paths_of(root, &run.worktree, claude)
}

fn session_paths_of(root: &Path, worktree: &Path, claude: bool) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let cwd_slug: String = worktree
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
    let omo_slug = crate::omo::encode_cwd(worktree);
    // omp 18 uses home-relative names, unlike Pi's absolute legacy buckets.
    // Directory names are only discovery hints; the transcript cwd remains authoritative.
    let home_slug = std::env::var_os("HOME")
        .and_then(|home| {
            worktree
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
    let temp_slug = worktree
        .strip_prefix(std::env::temp_dir())
        .ok()
        .map(|relative| {
            format!(
                "-tmp-{}",
                relative.to_string_lossy().replace(['/', '\\', ':'], "-")
            )
        });
    let listing = match fs::read_dir(root) {
        Ok(listing) => listing,
        // A supported harness need not have produced a session (or even its root) yet.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(paths),
        Err(error) => return Err(error).with_context(|| format!("reading {}", root.display())),
    };
    for entry in listing {
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
    Ok(paths)
}

fn collect_from(root: &Path, run: &Run, claude: bool, config: &Config) -> Result<Usage> {
    let mut total = None;
    for path in session_paths(root, run, claude)? {
        if let Some(usage) = transcript_cache::with_lines(&path, |lines| {
            parse_session_entries(lines, run, claude, config)
        })? {
            add(&mut total, usage)?;
        }
    }
    total.context("no assistant usage found for this worktree and run time window")
}

/// The reviewer transcript of a review child: only its cwd and time window, not other rounds
/// sharing a session.
pub(crate) fn transcript_head(run: &corral_core::RunHead, store: &Store) -> Result<Vec<u8>> {
    let harness = harness_of(run.harness.as_deref(), &run.agent, &store.config);
    transcript_of(harness, (&run.worktree, run.started, run.ended), store)
}

pub(crate) fn has_transcript(run: &Run, store: &Store) -> bool {
    matches!(run_harness(run, &store.config), "omo" | "omp" | "claude")
}

pub(crate) fn transcript_run(run: &Run, store: &Store) -> Result<Vec<u8>> {
    transcript_of(
        run_harness(run, &store.config),
        (&run.worktree, run.started, run.ended),
        store,
    )
}

fn transcript_of(
    harness: &str,
    (worktree, started, ended): (&Path, Timestamp, Option<Timestamp>),
    store: &Store,
) -> Result<Vec<u8>> {
    let (root, claude) = session_root_of(harness, worktree, store)?;
    let end = ended.unwrap_or_else(Timestamp::now);
    let mut bytes = Vec::new();
    for path in session_paths_of(&root, worktree, claude)? {
        let mut matched = false;
        let file = File::open(&path).with_context(|| format!("opening {}", path.display()))?;
        for line in BufReader::new(file).lines() {
            let line = line.context("reading reviewer transcript")?;
            if line.trim().is_empty() {
                continue;
            }
            let entry: Entry =
                serde_json::from_str(&line).context("parsing reviewer transcript")?;
            if let Some(cwd) = entry.cwd {
                matched = cwd == worktree;
            }
            if matched && entry.timestamp.is_some_and(|at| at >= started && at <= end) {
                bytes.extend_from_slice(line.as_bytes());
                bytes.push(b'\n');
                if u64::try_from(bytes.len()).context("transcript length overflow")?
                    > store.config.artifacts.max_bytes
                {
                    bail!("reviewer transcript exceeds artifact max_bytes");
                }
            }
        }
    }
    Ok(bytes)
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
#[cfg(test)]
fn parse_session_priced(
    reader: impl BufRead,
    run: &Run,
    claude: bool,
    config: &Config,
) -> Result<Option<Usage>> {
    let lines: Vec<_> = reader
        .lines()
        .enumerate()
        .map(|(index, line)| match line {
            Ok(line) => parse_session_line(line.as_bytes(), index),
            Err(error) => Err(format!("reading session line: {error}")),
        })
        .collect();
    parse_session_entries(&lines, run, claude, config)
}

type SessionLine = std::result::Result<Option<Entry>, String>;

fn parse_session_line(bytes: &[u8], index: usize) -> SessionLine {
    // Match BufRead::lines, including its CRLF handling and error columns.
    let bytes = match bytes.strip_suffix(b"\n") {
        Some(line) => line.strip_suffix(b"\r").unwrap_or(line),
        None => bytes,
    };
    let line = std::str::from_utf8(bytes)
        .map_err(|_| "reading session line: stream did not contain valid UTF-8".to_owned())?;
    if line.trim().is_empty() {
        return Ok(None);
    }
    serde_json::from_str(line)
        .map(Some)
        .map_err(|error| format!("unrecognized session format at line {}: {error}", index + 1))
}

fn parse_session_entries(
    lines: &[SessionLine],
    run: &Run,
    claude: bool,
    config: &Config,
) -> Result<Option<Usage>> {
    let end = run.ended.unwrap_or_else(Timestamp::now);
    let mut matched_cwd = false;
    let mut messages = BTreeMap::new();
    let mut model = run.model.clone();
    let mut previous_at = None;
    let mut trigger_at = None;
    let mut claude_triggers = BTreeMap::new();
    for (index, line) in lines.iter().enumerate() {
        let entry = match line {
            Ok(Some(entry)) => entry,
            Ok(None) => continue,
            Err(error) => bail!("{error}"),
        };
        let preceding_at = previous_at;
        previous_at = entry.timestamp;
        if claude
            && entry.message.as_ref().is_some_and(|message| {
                matches!(message.role.as_str(), "user" | "toolResult" | "tool")
            })
        {
            trigger_at = entry.timestamp;
        }
        if let Some(cwd) = &entry.cwd {
            if cwd != &run.worktree {
                return Ok(None);
            }
            matched_cwd = true;
        }
        if entry.kind == "model_change" {
            model = entry.model.clone();
            continue;
        }
        if !matched_cwd || !matches!(entry.kind.as_str(), "message" | "assistant") {
            continue;
        }
        let Some(message) = entry.message.as_ref().filter(|m| m.role == "assistant") else {
            continue;
        };
        // Fragments outside the run window still establish this provider message's
        // trigger. Later fragments update the response time, never that trigger.
        let preceding_at = if claude {
            message
                .id
                .as_ref()
                .and_then(|id| *claude_triggers.entry(id.clone()).or_insert(trigger_at))
        } else {
            preceding_at
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
                .clone()
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
                .clone()
                .context("Claude usage has no provider message id")?
        } else {
            entry.id.clone().unwrap_or_else(|| index.to_string())
        };
        let mut evidence = UsageEntry {
            at: Some(at),
            latency_ms: preceding_at
                .filter(|previous| *previous <= at)
                .map(|previous| {
                    u64::try_from(at.duration_since(previous).as_millis())
                        .context("response latency overflow")
                })
                .transpose()?,
            model: message
                .model
                .clone()
                .or_else(|| entry.model.clone())
                .or_else(|| model.clone()),
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

#[cfg(test)]
#[path = "usage_guard_tests.rs"]
mod guard_tests;

#[cfg(test)]
#[path = "usage_verdict_tests.rs"]
mod verdict_tests;
