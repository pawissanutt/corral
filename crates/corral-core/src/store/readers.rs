//! Store-owned readers of task documents and event history.
//!
//! Callers outside the store reach the task files and the event log only through these
//! methods, so a later backend can serve the same reads without the file layout.

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

use super::{
    Store, file_id_number, parse_task, parse_task_record, render_task, render_task_record,
};
use crate::event_reader::{EventCursor, EventReader};
use crate::model::{Event, Task};

impl Store {
    pub(super) fn events_path(&self) -> PathBuf {
        self.dir.join("events.jsonl")
    }

    /// Every event of one task, oldest first.
    ///
    /// # Errors
    /// Returns event-history read errors.
    pub fn events_for(&self, task: &str) -> Result<Vec<Event>> {
        Ok(self
            .events_shared()?
            .iter()
            .filter(|event| event.task.as_deref() == Some(task))
            .cloned()
            .collect())
    }

    /// Events of the given kinds, oldest first; `task` narrows them to one task.
    ///
    /// # Errors
    /// Returns event-history read errors.
    pub fn events_of(&self, task: Option<&str>, kinds: &[&str]) -> Result<Vec<Event>> {
        Ok(self
            .events_shared()?
            .iter()
            .filter(|event| task.is_none_or(|task| event.task.as_deref() == Some(task)))
            .filter(|event| kinds.contains(&event.kind.as_str()))
            .cloned()
            .collect())
    }

    /// [`Store::events_of`] for the workspace at `root` without loading its configuration,
    /// so configuration inspection works while the current file does not parse.
    ///
    /// # Errors
    /// Returns event-history read errors.
    pub fn events_of_at(root: &Path, task: Option<&str>, kinds: &[&str]) -> Result<Vec<Event>> {
        let mut reader = EventReader::from_offset(&root.join(".corral/events.jsonl"), 0)?;
        let mut events = Vec::new();
        loop {
            let before = reader.read_position();
            match reader.next_event()? {
                Some((event, _)) => {
                    if task.is_none_or(|task| event.task.as_deref() == Some(task))
                        && kinds.contains(&event.kind.as_str())
                    {
                        events.push(event);
                    }
                }
                None if reader.read_position() == before => break,
                None => {}
            }
        }
        Ok(events)
    }

    /// A bounded, resumable follower of the event history after `after`; `None` starts at the
    /// current end, `Some(EventCursor::default())` at the beginning.
    ///
    /// # Errors
    /// Returns the follower's open errors.
    pub fn events_since(&self, after: Option<EventCursor>) -> Result<EventReader> {
        EventReader::open(&self.events_path(), after)
    }

    /// [`Store::events_since`] for readers that must keep events up to `max_line_bytes` of
    /// stored form, such as the content index; reads stay bounded per call.
    ///
    /// # Errors
    /// Returns the follower's open errors.
    pub fn events_since_bounded(
        &self,
        after: Option<EventCursor>,
        max_line_bytes: usize,
    ) -> Result<EventReader> {
        Ok(EventReader::open(&self.events_path(), after)?.with_line_limit(max_line_bytes))
    }

    /// The newest complete events within the last `max_bytes` of stored history, oldest first.
    /// A history that is not a regular file yields none.
    ///
    /// # Errors
    /// Returns event-history read errors.
    pub fn recent_events(&self, max_bytes: u64) -> Result<Vec<Event>> {
        let path = self.events_path();
        if max_bytes == 0 {
            return Ok(Vec::new());
        }
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() => {}
            Ok(_) => return Ok(Vec::new()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(error).with_context(|| format!("inspecting {}", path.display()));
            }
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
        let metadata = file.metadata().context("inspecting opened event history")?;
        if !metadata.is_file() {
            return Ok(Vec::new());
        }
        let start = metadata.len().saturating_sub(max_bytes);
        let mut partial_line = false;
        if start > 0 {
            file.seek(SeekFrom::Start(start - 1))
                .context("seeking recent events")?;
            let mut previous = [0];
            file.read_exact(&mut previous)
                .context("reading recent event boundary")?;
            partial_line = previous != *b"\n";
        }
        let mut bytes = Vec::new();
        file.take(max_bytes)
            .read_to_end(&mut bytes)
            .with_context(|| format!("reading {}", path.display()))?;
        if partial_line {
            let end = bytes
                .iter()
                .position(|b| *b == b'\n')
                .map_or(bytes.len(), |n| n + 1);
            bytes.drain(..end);
        }
        Ok(bytes
            .split_inclusive(|b| *b == b'\n')
            .filter_map(|line| serde_json::from_slice(line).ok())
            .collect())
    }

    /// Opaque identity of the stored event history; it changes whenever the history does.
    ///
    /// # Errors
    /// Returns metadata errors other than an absent history.
    pub fn events_stamp(&self) -> Result<String> {
        let mut hash = Sha256::new();
        stamp_file(&mut hash, &self.events_path())?;
        Ok(format!("{:x}", hash.finalize()))
    }

    /// Opaque identity of every stored task document, including unreadable ones; it changes
    /// whenever a task is created, rewritten or removed.
    ///
    /// # Errors
    /// Returns directory or metadata errors.
    pub fn tasks_stamp(&self) -> Result<String> {
        let mut paths = fs::read_dir(self.dir.join("tasks"))
            .context("listing stored tasks")?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::io::Result<Vec<_>>>()
            .context("reading stored task entries")?;
        paths.retain(|path| path.extension().is_some_and(|ext| ext == "md"));
        paths.sort();
        let mut hash = Sha256::new();
        for path in paths {
            hash.update(serde_json::to_vec(&path).context("serializing task stamp")?);
            stamp_file(&mut hash, &path)?;
        }
        Ok(format!("{:x}", hash.finalize()))
    }

    /// Task numbers already taken, including those of stored documents that do not parse:
    /// `list` skips them, but they still reserve their id.
    ///
    /// # Errors
    /// Returns directory errors or an invalid stored id.
    pub fn reserved_task_numbers(&self) -> Result<BTreeSet<u64>> {
        let directory = self.dir.join("tasks");
        let mut numbers = BTreeSet::new();
        for entry in
            fs::read_dir(&directory).with_context(|| format!("reading {}", directory.display()))?
        {
            let entry = entry.context("reading task directory entry")?;
            if entry
                .file_type()
                .context("reading task file type")?
                .is_dir()
            {
                continue;
            }
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "md") {
                continue;
            }
            if let Some(number) = file_id_number(&path) {
                numbers.insert(number);
            }
        }
        Ok(numbers)
    }

    /// Where a task stored under `file_stem` (for example `T-042-plan`) lives.
    pub fn task_location(&self, file_stem: &str) -> PathBuf {
        self.dir.join("tasks").join(format!("{file_stem}.md"))
    }

    /// The Markdown form of a task (YAML frontmatter, then the body), as `corral edit` and
    /// staged journals carry it.
    pub fn render_task_markdown(task: &Task) -> String {
        render_task(task)
    }

    /// Parses [`Store::render_task_markdown`] output; the result has no stored location.
    ///
    /// # Errors
    /// Refuses malformed frontmatter or unsupported schemas.
    pub fn parse_task_markdown(text: &str) -> Result<Task> {
        parse_task(text, Path::new(""))
    }

    /// The Markdown form of a complete record, recovery evidence included.
    ///
    /// # Errors
    /// Refuses invalid recovery evidence or serialization failures.
    pub fn render_task_record_markdown(record: &crate::TaskRecord) -> Result<String> {
        render_task_record(record)
    }

    /// Parses [`Store::render_task_record_markdown`] output without losing recovery evidence.
    ///
    /// # Errors
    /// Refuses malformed frontmatter, unsupported schemas and invalid recovery records.
    pub fn parse_task_record_markdown(text: &str) -> Result<crate::TaskRecord> {
        parse_task_record(text, Path::new(""))
    }
}

fn stamp_file(hash: &mut Sha256, path: &Path) -> Result<()> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            hash.update([0]);
            return Ok(());
        }
        Err(error) => {
            return Err(error).with_context(|| format!("inspecting {}", path.display()));
        }
    };
    hash.update([1]);
    hash.update(metadata.len().to_le_bytes());
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        hash.update(metadata.dev().to_le_bytes());
        hash.update(metadata.ino().to_le_bytes());
        hash.update(metadata.mtime().to_le_bytes());
        hash.update(metadata.mtime_nsec().to_le_bytes());
        hash.update(metadata.ctime().to_le_bytes());
        hash.update(metadata.ctime_nsec().to_le_bytes());
    }
    #[cfg(not(unix))]
    {
        let modified = metadata
            .modified()
            .context("reading stored input mtime")?
            .duration_since(std::time::UNIX_EPOCH)
            .context("stored input mtime precedes epoch")?;
        hash.update(modified.as_secs().to_le_bytes());
        hash.update(modified.subsec_nanos().to_le_bytes());
    }
    Ok(())
}
