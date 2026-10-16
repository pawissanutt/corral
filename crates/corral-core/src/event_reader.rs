//! Bounded, resumable following of the append-only event log.

use std::fs::{self, File, Metadata};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::Event;

/// Maximum bytes consumed by one `next_event` call, and maximum retained line size.
pub const EVENT_READ_LIMIT: usize = 64 * 1024;
const ANCHOR_LEN: usize = 64;

/// Position immediately after a completed log line, safe to persist as JSON.
///
/// Identity detects replacement; the bounded trailing anchor detects truncate-and-regrow.
/// An offset-only cursor deliberately refers to the file currently at the path.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventCursor {
    pub offset: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub anchor: Vec<u8>,
}

/// Incremental event reader. Does not wait: callers must pace calls returning `None`.
///
/// Each call consumes at most [`EVENT_READ_LIMIT`] bytes. Incomplete lines are retained;
/// malformed and oversized lines are skipped. `None` can mean EOF or exhausted read budget.
/// Rotation/truncation resets to byte zero of the current file; an unlinked old file is
/// not drained. A missing path is treated as temporarily empty.
/// An oversized line without a newline cannot be checkpointed: restarting from the last
/// completed line rescans that prefix, still within the per-call byte and memory limits.
pub struct EventReader {
    path: PathBuf,
    cursor: EventCursor,
    position: u64,
    anchor: Vec<u8>,
    partial: Vec<u8>,
    skipping: bool,
}

fn identity(metadata: &Metadata) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        format!("{}:{}", metadata.dev(), metadata.ino())
    }
    #[cfg(not(unix))]
    {
        format!("{:?}", metadata.created())
    }
}

fn open_regular(path: &Path) -> Result<Option<File>> {
    let metadata = match fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("inspecting {}", path.display())),
    };
    if !metadata.is_file() {
        bail!("event log is not a regular file: {}", path.display());
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    // Prevent a swapped-in FIFO from blocking between metadata and open on Linux.
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(0x800); // O_NONBLOCK
    }
    let file = match options.open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("opening {}", path.display())),
    };
    if !file
        .metadata()
        .context("inspecting opened event log")?
        .is_file()
    {
        bail!("event log is not a regular file: {}", path.display());
    }
    Ok(Some(file))
}

fn trailing(file: &mut File, position: u64) -> Result<Vec<u8>> {
    let len = position.min(u64::try_from(ANCHOR_LEN)?);
    file.seek(SeekFrom::Start(position - len))
        .context("seeking event log anchor")?;
    let mut bytes = vec![0; usize::try_from(len)?];
    file.read_exact(&mut bytes)
        .context("reading event log anchor")?;
    Ok(bytes)
}

impl EventReader {
    /// Opens at a persisted cursor, or at current EOF when `cursor` is `None`.
    ///
    /// # Errors
    /// Returns an error for non-regular files, invalid anchors or filesystem failures.
    pub fn open(path: &Path, cursor: Option<EventCursor>) -> Result<Self> {
        let mut reader = Self {
            path: path.to_path_buf(),
            cursor: cursor.clone().unwrap_or_default(),
            position: 0,
            anchor: Vec::new(),
            partial: Vec::new(),
            skipping: false,
        };
        if reader.cursor.anchor.len() > ANCHOR_LEN {
            bail!("event cursor anchor exceeds {ANCHOR_LEN} bytes");
        }
        reader.position = reader.cursor.offset;
        reader.anchor = reader.cursor.anchor.clone();
        reader.skipping = !reader.anchor.is_empty() && reader.anchor.last() != Some(&b'\n');
        if cursor.is_none()
            && let Some(mut file) = open_regular(path)?
        {
            let metadata = file.metadata().context("inspecting event log")?;
            reader.position = metadata.len();
            reader.anchor = trailing(&mut file, reader.position)?;
            reader.cursor = EventCursor {
                offset: reader.position,
                identity: Some(identity(&metadata)),
                anchor: reader.anchor.clone(),
            };
            // Do not interpret the suffix of a pre-existing incomplete line as a new event.
            reader.skipping = reader.position > 0 && reader.anchor.last() != Some(&b'\n');
        } else {
            reader.refresh()?;
        }
        Ok(reader)
    }

    /// Opens at an explicit byte offset in the current log (normally an emitted end offset).
    ///
    /// # Errors
    /// Returns the same filesystem errors as [`Self::open`].
    pub fn from_offset(path: &Path, offset: u64) -> Result<Self> {
        Self::open(
            path,
            Some(EventCursor {
                offset,
                ..EventCursor::default()
            }),
        )
    }

    /// Last completed line cursor (including skipped lines), or the starting cursor.
    ///
    /// A partial line never advances this checkpoint. Event tuples always carry the exact
    /// end cursor for that event, with no subsequent lines consumed.
    pub fn cursor(&self) -> &EventCursor {
        &self.cursor
    }

    /// Bytes consumed in the current file, including incomplete and skipped lines.
    ///
    /// After `next_event` returns `None`, a changed position means bounded progress:
    /// call again to drain. An unchanged position means the caller should wait.
    /// Unlike `cursor()`, this is not a safe persistence checkpoint for partial lines.
    pub fn read_position(&self) -> u64 {
        self.position
    }

    fn refresh(&mut self) -> Result<Option<File>> {
        let Some(mut file) = open_regular(&self.path)? else {
            return Ok(None);
        };
        let metadata = file.metadata().context("inspecting event log")?;
        let id = identity(&metadata);
        let replaced = self.cursor.identity.as_ref().is_some_and(|old| old != &id);
        let truncated = metadata.len() < self.position;
        let changed = !replaced
            && !truncated
            && !self.anchor.is_empty()
            && trailing(&mut file, self.position)? != self.anchor;
        if replaced || truncated || changed {
            self.position = 0;
            self.anchor.clear();
            self.partial.clear();
            self.skipping = false;
            self.cursor = EventCursor::default();
        }
        self.cursor.identity = Some(id);
        file.seek(SeekFrom::Start(self.position))
            .context("seeking event log")?;
        Ok(Some(file))
    }

    /// Returns one event and its exact end cursor, without consuming subsequent events.
    ///
    /// # Errors
    /// Returns an error for non-regular files or filesystem failures.
    pub fn next_event(&mut self) -> Result<Option<(Event, EventCursor)>> {
        let Some(mut file) = self.refresh()? else {
            return Ok(None);
        };
        let mut budget = EVENT_READ_LIMIT;
        let mut chunk = [0; 4096];
        while budget > 0 {
            let n = file
                .read(&mut chunk[..budget.min(4096)])
                .context("reading event log")?;
            if n == 0 {
                break;
            }
            for &byte in &chunk[..n] {
                self.position = self
                    .position
                    .checked_add(1)
                    .context("event offset overflow")?;
                if self.anchor.len() == ANCHOR_LEN {
                    self.anchor.remove(0);
                }
                self.anchor.push(byte);
                if byte == b'\n' {
                    let event = if self.skipping {
                        None
                    } else {
                        serde_json::from_slice::<Event>(&self.partial).ok()
                    };
                    self.partial.clear();
                    self.skipping = false;
                    self.cursor.offset = self.position;
                    self.cursor.anchor = self.anchor.clone();
                    if let Some(event) = event {
                        return Ok(Some((event, self.cursor.clone())));
                    }
                } else if !self.skipping {
                    if self.partial.len() == EVENT_READ_LIMIT {
                        self.partial.clear();
                        self.skipping = true;
                    } else {
                        self.partial.push(byte);
                    }
                }
            }
            budget -= n;
        }
        Ok(None)
    }
}

/// Normalizes actionable kinds, including autopilot's `detail.to` transitions.
pub fn actionable_kind(event: &Event) -> Option<&str> {
    let kind = match event.kind.as_str() {
        "autopilot" => event.detail.get("to")?.as_str()?,
        "plan_awaiting_owner" => "needs_me",
        "plan_escalated" | "plan_failed" => "escalated",
        "plan_decomposed" => "done",
        kind => kind,
    };
    match kind {
        "done" | "needs_me" | "stuck" | "blocked" | "escalated" | "lessons_due" => Some(kind),
        "config_changed"
            if event.detail.get("pm_relevant").and_then(|v| v.as_bool()) == Some(true) =>
        {
            Some(kind)
        }
        _ => None,
    }
}

/// Shared single-line notification, suitable for wait and persistent push.
pub fn format_event_line(event: &Event) -> String {
    let task = event.task.as_deref().unwrap_or("-");
    let kind = actionable_kind(event).unwrap_or(&event.kind);
    let mut line = format!("{task} {kind}");
    if let Some(run) = event.detail.get("run").and_then(|v| v.as_str()) {
        line.push_str(&format!(" {run}"));
    }
    if let (Some(added), Some(removed)) = (
        event.detail.get("added").and_then(|v| v.as_u64()),
        event.detail.get("removed").and_then(|v| v.as_u64()),
    ) {
        line.push_str(&format!(" +{added}/-{removed}"));
    }
    if let Some(reason) = event.detail.get("reason").and_then(|v| v.as_str()) {
        line.push_str(&format!(": {reason}"));
    }
    if event.kind == "config_changed" {
        if let Some(changes) = event.detail.get("changes").and_then(|v| v.as_array()) {
            let keys: Vec<_> = changes
                .iter()
                .filter_map(|v| v.get("key").and_then(|v| v.as_str()))
                .collect();
            line.push_str(&format!(": {}", keys.join(", ")));
        }
        line.push_str("; applies at next dispatch/gate/sync; running agents unchanged");
    }
    line.replace(['\n', '\r'], " ")
}

#[cfg(test)]
#[path = "event_reader_tests.rs"]
mod tests;
