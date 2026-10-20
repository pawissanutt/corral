//! Event-log search records, decoding each completed line once per process.
//!
//! The log is append-only. Records of lines ending in a newline are kept with the log's inode and
//! the bytes just before the checkpoint; a later refresh of the same, longer file decodes only
//! the bytes after it. Replacement, truncation or a changed anchor starts again from byte zero.
//! A trailing partial line is decoded on every call and never kept, as before.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use corral_core::work::{Work, record};
use corral_core::{ArtifactTier, Event};

use super::{Document, INDEX_BYTES, Record, Source, digest};

const ANCHOR_BYTES: usize = 64;

#[derive(Default)]
struct Checkpoint {
    identity: Option<(u64, u64)>,
    offset: u64,
    anchor: Vec<u8>,
    documents: Vec<(Document, String)>,
}

static CHECKPOINTS: Mutex<BTreeMap<PathBuf, Checkpoint>> = Mutex::new(BTreeMap::new());

#[cfg(unix)]
fn identity(metadata: &std::fs::Metadata) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    Some((metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn identity(_: &std::fs::Metadata) -> Option<(u64, u64)> {
    None
}

fn document(offset: u64, line: &[u8]) -> Option<(Document, String)> {
    let event = serde_json::from_slice::<Event>(line).ok()?;
    let detail = event.detail.to_string();
    let document = Document {
        reference: format!("events:{offset}"),
        task: event.task.unwrap_or_default(),
        run: event
            .detail
            .get("run")
            .and_then(|r| r.as_str())
            .map(str::to_string),
        scope: if event.kind == "changes_requested" {
            "reviews"
        } else {
            "events"
        }
        .into(),
        kind: event.kind.clone(),
        tier: ArtifactTier::Normal,
        title: event.kind,
        updated: event.ts,
        fingerprint: digest(&detail),
        file_stamp: None,
        text: String::new(),
        complete: false,
        binary: false,
    };
    Some((document, detail))
}

fn resumable(file: &mut File, checkpoint: &Checkpoint, len: u64) -> Result<bool> {
    if checkpoint.identity.is_none() || len < checkpoint.offset {
        return Ok(false);
    }
    let start = checkpoint.offset - checkpoint.anchor.len() as u64;
    file.seek(SeekFrom::Start(start))
        .context("seeking search events")?;
    let mut anchor = vec![0; checkpoint.anchor.len()];
    file.read_exact(&mut anchor)
        .context("reading search event anchor")?;
    Ok(anchor == checkpoint.anchor)
}

pub(super) fn records(path: &Path, records: &mut Vec<Record>) -> Result<()> {
    let mut file = File::open(path).context("opening search events")?;
    record(Work::EventRead, 1);
    let metadata = file.metadata().context("inspecting search events")?;
    let mut checkpoints = CHECKPOINTS
        .lock()
        .map_err(|e| anyhow::anyhow!("locking search events: {e}"))?;
    let previous = checkpoints.remove(path).unwrap_or_default();
    let mut checkpoint = if previous.identity == identity(&metadata)
        && resumable(&mut file, &previous, metadata.len())?
    {
        previous
    } else {
        Checkpoint {
            identity: identity(&metadata),
            ..Checkpoint::default()
        }
    };
    file.seek(SeekFrom::Start(checkpoint.offset))
        .context("seeking search events")?;
    let mut reader = BufReader::new(file);
    let mut offset = checkpoint.offset;
    let mut anchor = checkpoint.anchor.clone();
    let mut tail = Vec::new();
    let mut line = Vec::new();
    loop {
        line.clear();
        // Bound each event allocation independently of the size of the event history.
        let size = reader
            .by_ref()
            .take(INDEX_BYTES)
            .read_until(b'\n', &mut line)
            .context("reading search event")?;
        if size == 0 {
            break;
        }
        record(Work::EventBytes, size as u64);
        record(Work::EventParse, 1);
        tail.extend(document(offset, &line));
        offset = offset.saturating_add(u64::try_from(size).context("event size overflow")?);
        anchor.extend_from_slice(&line);
        anchor.drain(..anchor.len().saturating_sub(ANCHOR_BYTES));
        if line.ends_with(b"\n") {
            checkpoint.documents.append(&mut tail);
            checkpoint.offset = offset;
            checkpoint.anchor.clone_from(&anchor);
        }
    }
    records.extend(
        checkpoint
            .documents
            .iter()
            .chain(&tail)
            .map(|(document, detail)| Record {
                document: document.clone(),
                source: Source::Text(detail.clone()),
            }),
    );
    if checkpoint.identity.is_some() {
        checkpoints.insert(path.to_path_buf(), checkpoint);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn line(kind: &str) -> String {
        format!(
            "{{\"ts\":\"2026-01-01T00:00:00Z\",\"task\":\"T-001\",\"kind\":\"{kind}\",\"detail\":{{}}}}\n"
        )
    }

    fn seen(path: &Path) -> Vec<(String, String)> {
        let mut out = Vec::new();
        records(path, &mut out).unwrap();
        out.into_iter()
            .map(|r| (r.document.reference, r.document.kind))
            .collect()
    }

    fn fresh(path: &Path) -> Vec<(String, String)> {
        CHECKPOINTS.lock().unwrap().remove(path);
        seen(path)
    }

    #[test]
    fn appended_partial_and_replaced_logs_match_a_full_read() {
        // Given a log already indexed once.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        std::fs::write(&path, line("created") + &line("moved")).unwrap();
        assert_eq!(seen(&path).len(), 2);

        // When complete and partial lines are appended, then the partial line completes.
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        let done = line("done");
        file.write_all(line("approved").as_bytes()).unwrap();
        file.write_all(&done.as_bytes()[..done.len() - 1]).unwrap();
        let partial = seen(&path);
        file.write_all(b"\n").unwrap();
        let completed = seen(&path);

        // Then each refresh equals a full read, including the trailing partial line.
        assert_eq!(partial.len(), 4);
        assert_eq!(partial, fresh(&path));
        assert_eq!(completed, fresh(&path));
        assert_eq!(completed, partial);

        // When the log is replaced by a same-length log with other events.
        std::fs::write(
            &path,
            line("comment") + &line("artifac") + &line("approved"),
        )
        .unwrap();
        let replaced = seen(&path);

        // Then nothing indexed from the old log survives.
        assert_eq!(replaced, fresh(&path));
        assert_eq!(replaced[0].1, "comment");
    }
}
