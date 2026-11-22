//! React extensions: deliver events to `run` lines in sequence order, one at a time per
//! extension, behind a durable cursor.
//!
//! The cursor holds the last sequence an extension finished: delivered, skipped after
//! repeated failure, or not matching its `on` and `projects`. It advances only after that
//! outcome, by an atomic replace (write, fsync, rename, fsync directory). Event sequences
//! come from an AUTOINCREMENT key assigned under SQLite's single writer, so a reader never
//! sees a later sequence before an earlier one has committed, and none is reused. Crash
//! points:
//! - before or during a delivery: the cursor is still below the event, so the next pass
//!   delivers it again;
//! - after a delivery, before the save: delivered again (at least once, never zero times);
//! - after the save: the event is finished.
//!
//! No sequence between the cursor and the head is passed over without an outcome, so none is
//! skipped silently. A per-extension lock keeps the daemon and `ext deliver` from delivering
//! the same extension at once. Delivery runs off every engine operation's path.
//!
//! A pass holds the extension's lock. Each line runs under an attempt supervisor that
//! alone inherits it and is the parent of the line's shell. When the shell exits, at the
//! deadline, or when the pass dies, the supervisor kills the line's process group, reaps
//! the shell and only then exits, releasing the lock. So no two attempts for an extension
//! ever run at once, and after a crash the event is delivered again only once the
//! interrupted attempt has stopped. A new extension's cursor is saved at the head observed
//! with the configuration, before any line of the pass runs, so a busy extension cannot
//! make it miss events.

use std::{
    fs::{self, File, OpenOptions, TryLockError},
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    Config, Store,
    config::Extension,
    events::Event,
    extension::{CommandContext, run},
};

pub const DEADLINE: Duration = Duration::from_secs(60);
/// Failed attempts after which an event is skipped and counted.
pub const MAX_FAILURES: u32 = 3;
const MAX_STDOUT: usize = 1024 * 1024;
const PAGE: u16 = 128;
const DIR: &str = "extensions";

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Cursor {
    /// The last sequence this extension finished.
    pub seq: u64,
    /// The event the failures were counted on.
    pub failing: Option<u64>,
    /// Failed attempts at `failing`.
    pub failures: u32,
    pub failed_at: Option<Timestamp>,
    /// Events given up on after `MAX_FAILURES` attempts.
    pub skipped: u64,
    /// Cleared by the next successful delivery.
    pub last_error: Option<String>,
}

pub struct Pass<'a> {
    pub root: &'a Path,
    pub state_dir: &'a Path,
    pub config: &'a Config,
    pub store: &'a Store,
    /// Minimum time before a failed event is attempted again.
    pub retry_after: Duration,
    /// Checked between deliveries; a delivery in progress is not interrupted.
    pub stop: &'a dyn Fn() -> bool,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Report {
    pub extension: String,
    pub delivered: u64,
    pub skipped: u64,
    /// The extension's line failed on an event that will be retried.
    pub failed: bool,
    /// Another process holds this extension's delivery lock.
    pub busy: bool,
    /// The pass could not run (cursor or store), distinct from a failing line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Report {
    /// Whether the extension's visible health may have changed.
    pub fn changed(&self) -> bool {
        self.delivered != 0 || self.skipped != 0 || self.failed || self.error.is_some()
    }
}

/// One delivery pass over every enabled extension with `run`, in name order.
pub fn deliver(pass: &Pass<'_>) -> Vec<Report> {
    let mut reports: Vec<_> = pass
        .config
        .extensions
        .iter()
        .filter_map(|(name, extension)| {
            let line = extension.run.as_deref().filter(|_| extension.enabled)?;
            let report = Report {
                extension: name.clone(),
                ..Report::default()
            };
            Some((name.as_str(), extension, line, report))
        })
        .collect();
    let dir = pass.state_dir.join(DIR);
    // A new extension starts at the newest event observed with this configuration; it
    // does not replay history. Save that before any line runs.
    let head = fs::create_dir_all(&dir)
        .context("create extension cursor directory")
        .and_then(|()| pass.store.latest_seq());
    for (name, _, _, report) in &mut reports {
        let started = match &head {
            Ok(head) => start(&dir, name, *head),
            Err(error) => Err(anyhow::anyhow!("{error:#}")),
        };
        if let Err(error) = started {
            report.error = Some(format!("{error:#}"));
        }
    }
    for (name, extension, line, report) in &mut reports {
        if report.error.is_some() {
            continue;
        }
        if (pass.stop)() {
            break;
        }
        if let Err(error) = deliver_one(pass, &dir, name, extension, line, report) {
            report.error = Some(format!("{error:#}"));
        }
    }
    reports
        .into_iter()
        .map(|(_, _, _, report)| report)
        .collect()
}

/// The saved cursor of an extension; None before its first pass.
pub fn status(state_dir: &Path, name: &str) -> Result<Option<Cursor>> {
    read_cursor(&cursor_path(&state_dir.join(DIR), name))
}

/// Saves `head` as the cursor of an extension that has none. Whoever holds its lock has
/// either started it already or starts it at the head it observed.
fn start(dir: &Path, name: &str, head: u64) -> Result<()> {
    let path = cursor_path(dir, name);
    if read_cursor(&path)?.is_some() {
        return Ok(());
    }
    let Some(_lock) = lock(dir, name)? else {
        return Ok(());
    };
    if read_cursor(&path)?.is_none() {
        write_cursor(
            &path,
            &Cursor {
                seq: head,
                ..Cursor::default()
            },
        )?;
    }
    Ok(())
}

fn deliver_one(
    pass: &Pass<'_>,
    dir: &Path,
    name: &str,
    extension: &Extension,
    line: &str,
    report: &mut Report,
) -> Result<()> {
    // Held by this pass, and by the supervisor of a line until that line has been stopped.
    let Some(lock) = lock(dir, name)? else {
        report.busy = true;
        return Ok(());
    };
    let path = cursor_path(dir, name);
    let Some(mut cursor) = read_cursor(&path)? else {
        // Another pass holding the lock during `start` saves it; deliver on a later pass.
        return Ok(());
    };
    if cursor.failures != 0
        && let Some(at) = cursor.failed_at
        && Timestamp::now().duration_since(at).unsigned_abs() < pass.retry_after
    {
        return Ok(());
    }
    let mut saved = cursor.clone();
    let head = pass.store.latest_seq()?;
    'pages: while cursor.seq < head {
        let events =
            pass.store
                .event_page(None, Some(cursor.seq), head.checked_add(1), false, PAGE)?;
        if events.is_empty() {
            break;
        }
        for event in events {
            if !matches(extension, &event) {
                cursor.seq = event.seq;
                continue;
            }
            if (pass.stop)() {
                break 'pages;
            }
            match run_event(pass, name, line, &event, &lock) {
                Ok(()) => {
                    cursor.seq = event.seq;
                    cursor.failing = None;
                    cursor.failures = 0;
                    cursor.failed_at = None;
                    cursor.last_error = None;
                    report.delivered += 1;
                }
                Err(error) => {
                    // `on` or `projects` may have changed since the last attempt.
                    if cursor.failing != Some(event.seq) {
                        cursor.failing = Some(event.seq);
                        cursor.failures = 0;
                    }
                    cursor.failures += 1;
                    cursor.failed_at = Some(Timestamp::now());
                    cursor.last_error = Some(format!("event {}: {error:#}", event.seq));
                    if cursor.failures < MAX_FAILURES {
                        report.failed = true;
                        return write_cursor(&path, &cursor);
                    }
                    cursor.seq = event.seq;
                    cursor.failing = None;
                    cursor.failures = 0;
                    cursor.failed_at = None;
                    cursor.skipped += 1;
                    report.skipped += 1;
                }
            }
            write_cursor(&path, &cursor)?;
            saved = cursor.clone();
        }
    }
    if cursor != saved {
        write_cursor(&path, &cursor)?;
    }
    Ok(())
}

fn matches(extension: &Extension, event: &Event) -> bool {
    let kind = extension
        .on
        .as_ref()
        .is_none_or(|kinds| kinds.iter().any(|kind| kind == event.kind.as_str()));
    let project = match &event.project {
        Some(project) => extension.includes_project(project),
        None => extension.projects.is_none(),
    };
    kind && project
}

fn run_event(pass: &Pass<'_>, name: &str, line: &str, event: &Event, lock: &File) -> Result<()> {
    let Value::Object(mut request) = serde_json::to_value(event).context("encode event")? else {
        anyhow::bail!("event is not a JSON object");
    };
    request.entry("project").or_insert(Value::Null);
    let empty = Default::default();
    let settings = event
        .project
        .as_deref()
        .and_then(|project| pass.config.projects.get(project));
    let cwd = settings.map_or_else(|| pass.root.to_path_buf(), |s| pass.root.join(&s.path));
    let context = CommandContext {
        root: pass.root,
        cwd: &cwd,
        project: event.project.as_deref(),
        extension: name,
        env: settings.map_or(&empty, |s| &s.env),
        hold: Some(lock),
    };
    run(line, request, &context, DEADLINE, MAX_STDOUT).map(drop)
}

/// Extension names are TOML keys; keep every byte outside `[A-Za-z0-9_-]` out of the path.
fn file_stem(name: &str) -> String {
    name.bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_' {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

fn cursor_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{}.json", file_stem(name)))
}

fn lock(dir: &Path, name: &str) -> Result<Option<File>> {
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(format!("{}.lock", file_stem(name))))
        .context("open extension delivery lock")?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => Err(error).context("acquire extension delivery lock"),
    }
}

fn read_cursor(path: &Path) -> Result<Option<Cursor>> {
    match fs::read(path) {
        // A damaged cursor stops delivery instead of replaying or skipping events.
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .with_context(|| format!("decode extension cursor {}", path.display())),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => {
            Err(error).with_context(|| format!("read extension cursor {}", path.display()))
        }
    }
}

fn write_cursor(path: &Path, cursor: &Cursor) -> Result<()> {
    let temp = path.with_extension("json.tmp");
    let mut file = File::create(&temp).context("create extension cursor")?;
    file.write_all(&serde_json::to_vec(cursor).context("encode extension cursor")?)
        .context("write extension cursor")?;
    file.sync_all().context("sync extension cursor")?;
    fs::rename(&temp, path).context("replace extension cursor")?;
    if let Some(dir) = path.parent() {
        File::open(dir)
            .and_then(|dir| dir.sync_all())
            .context("sync extension cursor directory")?;
    }
    Ok(())
}
