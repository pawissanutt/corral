//! Historical observations, not prescribed lessons.
//!
//! Limits: the latest 256 tasks by persisted update time (ID breaks ties),
//! the last 4 MiB of complete event lines, and deterministic prefixes of
//! 64 KiB per artifact (compressed and decoded), 1 MiB total textual context,
//! 256 clusters, 32 evidence references and 64 values per metadata field.
//! Text and keys retain at most 512 characters. Missing/purged/unsafe artifacts
//! contribute no content. No current checkout files or current clock are consulted.
//! Per task: latest 128 events, 32 artifacts, 32 runs, one landed Git range.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use corral_core::{ArtifactKind, Event, LessonEvidence, Run, Status, Task};
use flate2::read::GzDecoder;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::Ctx;

const TASK_LIMIT: usize = 256;
const EVENT_BYTES: usize = 4 * 1024 * 1024;
const ARTIFACT_BYTES: usize = 64 * 1024;
const CONTEXT_BYTES: usize = 1024 * 1024;
const SIGNAL_LIMIT: usize = 256;
const EVIDENCE_LIMIT: usize = 32;
const VALUES_LIMIT: usize = 64;
const TEXT_CHARS: usize = 512;

/// A normalized cluster of source-backed observations for one project.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LessonSignal {
    pub key: String,
    pub category: String,
    pub text: String,
    pub evidence: Vec<LessonEvidence>,
    pub files: Vec<String>,
    pub kinds: Vec<String>,
    pub profiles: Vec<String>,
    pub harnesses: Vec<String>,
}

#[derive(Default)]
struct Collector {
    signals: BTreeMap<String, LessonSignal>,
    consumed: usize,
}

fn clipped(text: &str) -> String {
    text.chars().take(TEXT_CHARS).collect()
}

fn normalized(text: &str) -> String {
    clipped(
        &text
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase(),
    )
}

fn sorted_values(values: &mut Vec<String>) {
    values.sort();
    values.dedup();
    values.truncate(VALUES_LIMIT);
}

impl Collector {
    fn context<'a>(&mut self, text: &'a str) -> &'a str {
        let mut end = text.len().min(CONTEXT_BYTES.saturating_sub(self.consumed));
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        self.consumed += end;
        &text[..end]
    }

    fn add(
        &mut self,
        task: &Task,
        evidence: &LessonEvidence,
        category: &str,
        identity: &str,
        text: &str,
        files: &[String],
    ) {
        if identity.trim().is_empty() || text.trim().is_empty() {
            return;
        }
        let key = format!("{category}:{}", normalized(identity));
        if !self.signals.contains_key(&key) && self.signals.len() >= SIGNAL_LIMIT {
            return;
        }
        let text = clipped(text.trim());
        let entry = self
            .signals
            .entry(key.clone())
            .or_insert_with(|| LessonSignal {
                key,
                category: category.into(),
                text: text.clone(),
                evidence: Vec::new(),
                files: Vec::new(),
                kinds: Vec::new(),
                profiles: Vec::new(),
                harnesses: Vec::new(),
            });
        // A stable representative even when source order differs.
        if text < entry.text {
            entry.text = text;
        }
        entry.evidence.push(evidence.clone());
        entry
            .evidence
            .sort_by(|a, b| (&a.task, a.ts, &a.source).cmp(&(&b.task, b.ts, &b.source)));
        entry
            .evidence
            .dedup_by(|a, b| a.task == b.task && a.ts == b.ts && a.source == b.source);
        entry.evidence.truncate(EVIDENCE_LIMIT);
        entry.files.extend(files.iter().map(|s| clipped(s)));
        entry.kinds.extend(task.kind.iter().map(|s| clipped(s)));
        if let Some(profile) = &task.profile {
            entry.profiles.push(clipped(profile));
        }
        for run in task.runs.iter().rev().take(32) {
            entry
                .profiles
                .push(clipped(run.profile.as_deref().unwrap_or(&run.agent)));
            entry
                .harnesses
                .extend(run.harness.iter().map(|s| clipped(s)));
        }
        for values in [
            &mut entry.files,
            &mut entry.kinds,
            &mut entry.profiles,
            &mut entry.harnesses,
        ] {
            sorted_values(values);
        }
    }

    fn lines(
        &mut self,
        task: &Task,
        evidence: &LessonEvidence,
        text: &str,
        reviewer: bool,
        files: &[String],
    ) {
        let text = self.context(text);
        let mut not_verified_section = false;
        for line in text.lines() {
            let line = line.trim().trim_start_matches('#').trim();
            let line = line
                .strip_prefix("- ")
                .or_else(|| line.strip_prefix("* "))
                .unwrap_or(line);
            let lower = line.to_ascii_lowercase();
            if let Some((_, rest)) = lower.split_once("not verified:") {
                let rest = rest.trim().trim_matches(['.', '`', '*']).trim();
                if !rest.is_empty() && rest != "none" {
                    self.add(task, evidence, "not_verified", rest, line, files);
                }
                not_verified_section = rest.is_empty();
                continue;
            }
            if lower.trim_matches(['*', '`']) == "not verified" {
                not_verified_section = true;
                continue;
            }
            if not_verified_section {
                if lower.is_empty() {
                    continue;
                }
                if lower == "none" || lower == "none." {
                    not_verified_section = false;
                    continue;
                }
                self.add(task, evidence, "not_verified", line, line, files);
                // Summaries conventionally put the value on the next line.
                not_verified_section = false;
            }
            if let Some(test) = failed_test(line) {
                self.add(task, evidence, "failed_test", &test, line, files);
            }
            let category = if lower.contains("conflict") || lower.contains("both modified:") {
                Some("conflict")
            } else if lower.contains("sloc")
                || lower.contains("size ceiling")
                || (lower.contains("lines") && lower.contains("limit"))
            {
                Some("size_ceiling")
            } else {
                None
            };
            if let Some(category) = category {
                for file in mentioned_files(line) {
                    self.add(
                        task,
                        evidence,
                        category,
                        &file,
                        line,
                        std::slice::from_ref(&file),
                    );
                }
            }
            if reviewer && !line.is_empty() && !matches!(lower.as_str(), "none" | "none.") {
                let note = line
                    .strip_prefix("[P0]")
                    .or_else(|| line.strip_prefix("[P1]"))
                    .or_else(|| line.strip_prefix("[P2]"))
                    .or_else(|| line.strip_prefix("[P3]"))
                    .unwrap_or(line)
                    .trim();
                let note = note
                    .split_whitespace()
                    .map(|word| {
                        let (prefix, suffix) = word.split_once(':').unwrap_or((word, ""));
                        if !suffix.is_empty()
                            && suffix
                                .chars()
                                .all(|c| c.is_ascii_digit() || c == ':' || c == '-')
                        {
                            prefix
                        } else {
                            word
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                self.add(task, evidence, "reviewer_note", &note, line, files);
            }
        }
    }
}

fn failed_test(line: &str) -> Option<String> {
    if let Some(rest) = line.strip_prefix("test ")
        && let Some((name, result)) = rest.split_once(" ... ")
        && result.trim().starts_with("FAILED")
    {
        return Some(normalized(name));
    }
    if let Some(rest) = line.strip_prefix("--- FAIL:") {
        return rest.split_whitespace().next().map(normalized);
    }
    if let Some(rest) = line.strip_prefix("FAIL ") {
        return rest.split_whitespace().last().map(normalized);
    }
    None
}

fn file_token(token: &str) -> Option<String> {
    let token = token
        .trim_matches(|c: char| matches!(c, '`' | '\'' | '"' | '(' | ')' | '[' | ']' | ',' | ';'))
        .trim_end_matches(':');
    let token = token.split(':').next().unwrap_or(token);
    let token = token
        .strip_prefix("a/")
        .or_else(|| token.strip_prefix("b/"))
        .unwrap_or(token);
    let path = Path::new(token);
    if token.is_empty()
        || token == "/dev/null"
        || !path.components().all(|c| matches!(c, Component::Normal(_)))
        || (!token.contains('/')
            && path.extension().is_none_or(|ext| {
                ext.is_empty()
                    || !ext
                        .to_string_lossy()
                        .chars()
                        .any(|c| c.is_ascii_alphabetic())
            }))
        || token.contains(['*', '=', '<', '>'])
    {
        return None;
    }
    Some(clipped(token))
}

fn mentioned_files(line: &str) -> BTreeSet<String> {
    line.split_whitespace().filter_map(file_token).collect()
}

/// Read only normal relative paths and regular, non-symlink files beneath `root`.
/// The opened descriptor is checked too; Linux nonblocking/no-follow flags reject
/// a swapped-in FIFO or final symlink without blocking on it.
#[cfg(test)]
fn bounded_read(root: &Path, relative: &Path, cap: usize) -> Result<Option<Vec<u8>>> {
    bounded_source(root, relative, cap, false)
}

fn bounded_source(
    root: &Path,
    relative: &Path,
    cap: usize,
    recent: bool,
) -> Result<Option<Vec<u8>>> {
    if cap == 0 || relative.as_os_str().is_empty() {
        return Ok(None);
    }
    if !fs::symlink_metadata(root)
        .with_context(|| format!("inspecting lesson source root {}", root.display()))?
        .is_dir()
    {
        return Ok(None);
    }
    let mut path = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Ok(None);
        };
        path.push(name);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).with_context(|| format!("inspecting {}", path.display()));
            }
        };
        if metadata.file_type().is_symlink() {
            return Ok(None);
        }
    }
    if !fs::symlink_metadata(&path)
        .with_context(|| format!("inspecting {}", path.display()))?
        .is_file()
    {
        return Ok(None);
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(0x800 | 0x20000); // O_NONBLOCK | O_NOFOLLOW
    }
    let mut file = options
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    let metadata = file.metadata().context("inspecting opened lesson source")?;
    if !metadata.is_file() {
        return Ok(None);
    }
    let start = if recent {
        metadata.len().saturating_sub(u64::try_from(cap)?)
    } else {
        0
    };
    let mut partial_line = false;
    if start > 0 {
        file.seek(SeekFrom::Start(start - 1))
            .context("seeking recent lesson events")?;
        let mut previous = [0];
        file.read_exact(&mut previous)
            .context("reading lesson event boundary")?;
        partial_line = previous != [b'\n'];
    }
    let mut bytes = Vec::new();
    file.take(u64::try_from(cap)?)
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading {}", path.display()))?;
    if partial_line {
        let end = bytes
            .iter()
            .position(|b| *b == b'\n')
            .map_or(bytes.len(), |n| n + 1);
        bytes.drain(..end);
    }
    Ok(Some(bytes))
}

fn evidence(task: &Task, ts: Timestamp, source: impl Into<String>) -> LessonEvidence {
    LessonEvidence {
        task: task.id.clone(),
        ts,
        source: source.into(),
    }
}

fn artifact_text(
    ctx: &Ctx,
    artifact: &corral_core::Artifact,
    cap: usize,
) -> Result<Option<String>> {
    if artifact.purged_at.is_some() {
        return Ok(None);
    }
    let read_cap = if artifact.compressed_bytes.is_some() {
        cap.saturating_add(1)
    } else {
        cap
    };
    let Some(bytes) = bounded_source(
        ctx.store.dir(),
        Path::new(&artifact.path),
        read_cap,
        artifact.compressed_bytes.is_none() && artifact.kind == ArtifactKind::Log,
    )?
    else {
        return Ok(None);
    };
    if cap == 0 || bytes.len() > cap {
        return Ok(None);
    }
    let decoded = if artifact.compressed_bytes.is_some() {
        let mut decoded = Vec::new();
        GzDecoder::new(bytes.as_slice())
            .take(u64::try_from(cap)?)
            .read_to_end(&mut decoded)
            .with_context(|| format!("decoding lesson artifact {}", artifact.path))?;
        decoded
    } else {
        bytes
    };
    Ok(Some(String::from_utf8_lossy(&decoded).into_owned()))
}

fn is_sha(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|c| c.is_ascii_hexdigit())
}

/// Historical commit range, never a moving branch/worktree or arbitrary revspec.
fn landed_files(ctx: &Ctx, project: &str, event: &Event) -> Result<Vec<String>> {
    if event.kind != "approved" || event.detail["strategy"].as_str() == Some("keep_branch") {
        return Ok(Vec::new());
    }
    let Some((previous, sha)) = event.detail["previous_sha"]
        .as_str()
        .zip(event.detail["sha"].as_str())
        .filter(|(previous, sha)| is_sha(previous) && is_sha(sha))
    else {
        return Ok(Vec::new());
    };
    let Some(_) = ctx.store.config.projects.get(project) else {
        return Ok(Vec::new());
    };
    let repo = ctx.store.project_path(project)?;
    let mut child = Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args([
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--name-only",
            "-z",
            previous,
            sha,
            "--",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("reading landed paths in {}", repo.display()))?;
    let mut bytes = Vec::new();
    let read = child
        .stdout
        .take()
        .context("git diff stdout missing")?
        .take(u64::try_from(ARTIFACT_BYTES + 1)?)
        .read_to_end(&mut bytes);
    if bytes.len() > ARTIFACT_BYTES || read.is_err() {
        child.kill().context("stopping bounded lesson git diff")?;
    }
    let status = child.wait().context("waiting for lesson git diff")?;
    read.context("reading lesson git diff")?;
    if !status.success() || bytes.len() > ARTIFACT_BYTES {
        return Ok(Vec::new());
    }
    let mut files: Vec<_> = bytes
        .split(|b| *b == 0)
        .filter_map(|name| std::str::from_utf8(name).ok())
        .filter(|name| !name.is_empty())
        .map(clipped)
        .collect();
    sorted_values(&mut files);
    Ok(files)
}

fn event_category(event: &Event) -> Option<&str> {
    match event.kind.as_str() {
        "approved" | "done" => Some("success"),
        "discarded" => Some("discarded"),
        "changes_requested" => Some("changes_requested"),
        "verdict" => Some("verdict"),
        "gate" => Some("gate"),
        "needs_me" | "escalated" | "blocked" => Some("escalation"),
        "failed" => Some("failure"),
        "round" => Some("round"),
        "autopilot" => Some("round"),
        _ => None,
    }
}

fn run_signals(collector: &mut Collector, task: &Task, run: &Run, files: &[String]) {
    let source = evidence(
        task,
        run.ended.unwrap_or(run.started),
        format!("run:{}:stats", run.id),
    );
    let wall = run
        .ended
        .filter(|end| *end >= run.started)
        .map(|end| end.duration_since(run.started).as_secs_f64());
    let usage = run.total_usage();
    let text = format!(
        "run={} outcome={:?} rounds={} failures={} wall_seconds={wall:?} reported_usd={:?} estimated_usd={:?}",
        run.id,
        run.outcome,
        task.rounds,
        run.failures,
        usage.as_ref().and_then(|u| u.reported_usd.or(u.cost_usd)),
        usage.as_ref().and_then(|u| u.estimated_usd),
    );
    collector.add(
        task,
        &source,
        "run_stats",
        &format!("{}:{}", task.id, run.id),
        &text,
        files,
    );
    if let Some(summary) = &run.summary {
        collector.lines(
            task,
            &evidence(task, source.ts, format!("run:{}:summary", run.id)),
            summary,
            false,
            files,
        );
    }
    if let Some(review) = &run.review
        && let Some(notes) = &review.notes
    {
        collector.lines(
            task,
            &evidence(task, review.at, format!("run:{}:verdict", run.id)),
            notes,
            true,
            files,
        );
    }
    if let Some(gate) = &run.gate {
        let source = evidence(task, gate.at, format!("run:{}:gate", run.id));
        collector.add(
            task,
            &source,
            "gate",
            gate.result.as_str(),
            gate.result.as_str(),
            files,
        );
        if let Some(command) = &gate.failed_cmd {
            collector.add(task, &source, "failed_command", command, command, files);
        }
    }
    if let Some(state) = &run.autopilot
        && let Some(reason) = &state.reason
    {
        let source = evidence(
            task,
            state.at,
            format!("run:{}:autopilot:{}", run.id, state.stage.as_str()),
        );
        collector.add(task, &source, "round", state.stage.as_str(), reason, files);
        collector.lines(task, &source, reason, false, files);
    }
}

/// Collect deterministic source-backed clusters belonging only to `project`.
/// This is read-only; missing historical Git objects simply leave diff artifacts
/// as the path authority. Ordering and limits are documented at module level.
///
/// # Errors
/// Returns store, regular-file I/O, gzip decoding or subprocess setup failures.
pub fn lesson_signals(ctx: &Ctx, project: &str) -> Result<Vec<LessonSignal>> {
    let mut tasks = ctx.store.list()?;
    tasks.retain(|task| task.project.as_deref() == Some(project));
    tasks.sort_by(|a, b| (b.updated, &b.id).cmp(&(a.updated, &a.id)));
    tasks.truncate(TASK_LIMIT);
    let bytes = bounded_source(
        ctx.store.dir(),
        Path::new("events.jsonl"),
        EVENT_BYTES,
        true,
    )?
    .unwrap_or_default();
    let events: Vec<Event> = bytes
        .split_inclusive(|b| *b == b'\n')
        .filter_map(|line| serde_json::from_slice(line).ok())
        .collect();
    let mut collector = Collector::default();
    for task in tasks {
        let mut related: Vec<_> = events
            .iter()
            .filter(|event| event.task.as_deref() == Some(&task.id))
            .filter(|event| {
                event.detail["project"]
                    .as_str()
                    .is_none_or(|p| p == project)
            })
            .collect();
        related.sort_by(|a, b| {
            (a.ts, &a.kind, a.detail.to_string()).cmp(&(b.ts, &b.kind, b.detail.to_string()))
        });
        related.reverse();
        related.truncate(128);
        let mut files = Vec::new();
        for event in related.iter().filter(|e| e.kind == "approved").take(1) {
            let paths = landed_files(ctx, project, event)?;
            if !paths.is_empty() {
                let source = evidence(
                    &task,
                    event.ts,
                    format!(
                        "event:approved:{}",
                        event.detail["sha"].as_str().unwrap_or("")
                    ),
                );
                collector.add(
                    &task,
                    &source,
                    "landed_files",
                    &task.id,
                    &paths.join(", "),
                    &paths,
                );
                files.extend(paths);
            }
        }
        let mut artifacts: Vec<_> = task
            .artifacts
            .iter()
            .filter(|a| {
                matches!(
                    a.kind,
                    ArtifactKind::Log
                        | ArtifactKind::Summary
                        | ArtifactKind::Report
                        | ArtifactKind::Diff
                        | ArtifactKind::Evidence
                )
            })
            .collect();
        artifacts.sort_by(|a, b| (b.added, &b.path).cmp(&(a.added, &a.path)));
        artifacts.truncate(32);
        for artifact in artifacts {
            let cap = ARTIFACT_BYTES.min(CONTEXT_BYTES.saturating_sub(collector.consumed));
            let Some(text) = artifact_text(ctx, artifact, cap)? else {
                continue;
            };
            let source = evidence(&task, artifact.added, artifact.path.clone());
            if artifact.kind == ArtifactKind::Diff {
                let mut in_hunk = false;
                let mut paths: Vec<_> = text
                    .lines()
                    .filter_map(|line| {
                        // Git file metadata precedes its hunks. Only a new
                        // file section can introduce more path headers.
                        if line.starts_with("diff --git ") {
                            in_hunk = false;
                        }
                        if line.starts_with("@@") {
                            in_hunk = true;
                        }
                        if in_hunk {
                            return None;
                        }
                        let path = line
                            .strip_prefix("+++ ")
                            .or_else(|| line.strip_prefix("--- "))
                            .or_else(|| line.strip_prefix("rename from "))
                            .or_else(|| line.strip_prefix("rename to "))?;
                        let path = path
                            .strip_prefix("a/")
                            .or_else(|| path.strip_prefix("b/"))
                            .unwrap_or(path);
                        (path != "/dev/null"
                            && !path.is_empty()
                            && Path::new(path)
                                .components()
                                .all(|c| matches!(c, Component::Normal(_))))
                        .then(|| clipped(path))
                    })
                    .collect();
                sorted_values(&mut paths);
                if !paths.is_empty() {
                    collector.add(
                        &task,
                        &source,
                        "touched_files",
                        &task.id,
                        &paths.join(", "),
                        &paths,
                    );
                    files.extend(paths);
                }
            }
            collector.lines(
                &task,
                &source,
                &text,
                artifact.title.starts_with("Review ("),
                &files,
            );
        }
        sorted_values(&mut files);
        for signal in collector
            .signals
            .values_mut()
            .filter(|signal| signal.evidence.iter().any(|source| source.task == task.id))
        {
            signal.files.extend(files.iter().cloned());
            sorted_values(&mut signal.files);
        }
        if matches!(task.status, Status::Done | Status::Dropped) {
            let source = evidence(&task, task.updated, format!("task:{}:status", task.id));
            let category = if task.status == Status::Done {
                "success"
            } else {
                "discarded"
            };
            collector.add(
                &task,
                &source,
                category,
                task.status.as_str(),
                task.status.as_str(),
                &files,
            );
        }
        for event in related {
            let Some(category) = event_category(event) else {
                continue;
            };
            let source = evidence(&task, event.ts, format!("event:{}", event.kind));
            let mut text = event.kind.clone();
            for field in [
                "result",
                "verdict",
                "to",
                "round",
                "max_rounds",
                "reason",
                "note",
                "notes",
                "text",
                "summary",
                "failed_cmd",
            ] {
                if let Some(value) = event.detail.get(field) {
                    let value = value
                        .as_str()
                        .map_or_else(|| value.to_string(), str::to_string);
                    let remaining = CONTEXT_BYTES.saturating_sub(collector.consumed);
                    let mut end = value.len().min(remaining);
                    while !value.is_char_boundary(end) {
                        end -= 1;
                    }
                    let value = &value[..end];
                    text.push_str(&format!(" {field}={}", clipped(value)));
                    if matches!(
                        field,
                        "reason" | "note" | "notes" | "text" | "summary" | "failed_cmd"
                    ) {
                        collector.lines(
                            &task,
                            &source,
                            value,
                            event.kind == "verdict" || event.kind == "changes_requested",
                            &files,
                        );
                    } else {
                        collector.consumed += value.len();
                    }
                }
            }
            let identity = match event.kind.as_str() {
                "verdict" => format!("{}:{}", event.kind, event.detail["verdict"]),
                _ => format!(
                    "{}:{}:{}",
                    event.kind, event.detail["result"], event.detail["to"]
                ),
            };
            collector.add(&task, &source, category, &identity, &text, &files);
        }
        for run in task.runs.iter().rev().take(32) {
            run_signals(&mut collector, &task, run, &files);
            let path = crate::gate::gate_paths(ctx, &task.id, &run.id).log;
            if let Ok(relative) = path.strip_prefix(ctx.store.dir()) {
                let cap = ARTIFACT_BYTES.min(CONTEXT_BYTES.saturating_sub(collector.consumed));
                if let Some(bytes) = bounded_source(ctx.store.dir(), relative, cap, true)? {
                    let source = evidence(
                        &task,
                        run.gate.as_ref().map_or(run.started, |g| g.at),
                        relative.to_string_lossy(),
                    );
                    collector.lines(
                        &task,
                        &source,
                        &String::from_utf8_lossy(&bytes),
                        false,
                        &files,
                    );
                }
            }
        }
    }
    Ok(collector.signals.into_values().collect())
}

#[cfg(test)]
#[path = "lesson_signals_tests.rs"]
mod tests;
