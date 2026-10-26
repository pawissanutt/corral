//! Parsed snapshots of the log, checkpointed only after complete lines.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};

use crate::Event;
use crate::event_reader::{ANCHOR_LEN, EventCursor, identity, trailing};

const WRITTEN_LINES: usize = 32;
const WRITTEN_LINE_BYTES: usize = 64 * 1024;

#[derive(Debug, Default)]
pub(super) struct EventCache {
    snapshot: Mutex<CachedEvents>,
    written: Mutex<VecDeque<(String, Event)>>,
    #[cfg(test)]
    decodes: std::sync::atomic::AtomicUsize,
}

#[derive(Debug, Default)]
struct CachedEvents {
    cursor: EventCursor,
    /// Valid events before the checkpoint, excluding any unterminated final line.
    committed_events: usize,
    events: Arc<Vec<Event>>,
}

impl EventCache {
    /// Keep a bounded set of values already encoded by this writer, never log positions.
    pub(super) fn remember(&self, line: String, event: Event) -> Result<()> {
        if line.len() > WRITTEN_LINE_BYTES {
            return Ok(());
        }
        let mut written = self
            .written
            .lock()
            .map_err(|e| anyhow::anyhow!("locking written events: {e}"))?;
        if written.len() == WRITTEN_LINES {
            written.pop_front();
        }
        written.push_back((line, event));
        Ok(())
    }

    pub(super) fn read(&self, path: &Path) -> Result<Arc<Vec<Event>>> {
        // Serialize observations as well as updates across Store clones.
        let mut cached = self
            .snapshot
            .lock()
            .map_err(|e| anyhow::anyhow!("locking event cache: {e}"))?;
        let mut file = match File::open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                *cached = CachedEvents::default();
                return Ok(Arc::clone(&cached.events));
            }
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        let metadata = file
            .metadata()
            .with_context(|| format!("inspecting {}", path.display()))?;
        let id = identity(&file, &metadata)?;
        let reusable = cached.cursor.identity.as_ref() == Some(&id)
            && metadata.len() >= cached.cursor.offset
            && trailing(&mut file, cached.cursor.offset)? == cached.cursor.anchor;
        let offset = if reusable { cached.cursor.offset } else { 0 };
        file.seek(SeekFrom::Start(offset))
            .with_context(|| format!("seeking {}", path.display()))?;
        let mut text = String::new();
        file.read_to_string(&mut text)
            .with_context(|| format!("reading {}", path.display()))?;
        crate::work::record(crate::work::Work::EventRead, 1);
        crate::work::record(crate::work::Work::EventBytes, text.len() as u64);
        if reusable && text.is_empty() && cached.committed_events == cached.events.len() {
            return Ok(Arc::clone(&cached.events));
        }

        let committed_bytes = text.rfind('\n').map_or(0, |position| position + 1);
        let offset = offset
            .checked_add(u64::try_from(committed_bytes).context("event prefix length overflow")?)
            .context("event offset overflow")?;
        if !reusable {
            *cached = CachedEvents::default();
        }
        // Derive the new anchor from the bytes actually parsed, not another file read.
        let mut anchor = cached.cursor.anchor.clone();
        anchor.extend_from_slice(
            &text.as_bytes()[committed_bytes.saturating_sub(ANCHOR_LEN)..committed_bytes],
        );
        if anchor.len() > ANCHOR_LEN {
            anchor.drain(..anchor.len() - ANCHOR_LEN);
        }
        let committed_events = cached.committed_events;
        let written = self
            .written
            .lock()
            .map_err(|e| anyhow::anyhow!("locking written events: {e}"))?;
        // Retained snapshots stay immutable; only a changed log may copy their prefix.
        let events = Arc::make_mut(&mut cached.events);
        events.truncate(committed_events);
        events.extend(
            text[..committed_bytes]
                .lines()
                .filter_map(|l| self.decode(l, &written)),
        );
        let committed_events = events.len();
        events.extend(
            text[committed_bytes..]
                .lines()
                .filter_map(|l| self.decode(l, &written)),
        );
        cached.committed_events = committed_events;
        cached.cursor = EventCursor {
            offset,
            identity: Some(id),
            anchor,
        };
        Ok(Arc::clone(&cached.events))
    }

    fn decode(&self, line: &str, written: &VecDeque<(String, Event)>) -> Option<Event> {
        if let Some((_, event)) = written.iter().rev().find(|(bytes, _)| bytes == line) {
            return Some(event.clone());
        }
        #[cfg(test)]
        self.decodes
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        crate::work::record(crate::work::Work::EventParse, 1);
        serde_json::from_str(line).ok()
    }
}

#[cfg(test)]
#[path = "event_cache_tests.rs"]
mod tests;
