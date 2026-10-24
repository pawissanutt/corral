//! Event-history search records, decoding each completed event once per process.
//!
//! The history is append-only. Records of completed events are kept with the follower's
//! cursor; a later refresh decodes only the events after it. Replacement, truncation or a
//! changed anchor starts again from the beginning. A trailing unterminated event is decoded
//! on every call and never kept, as before.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::Result;
use corral_core::{ArtifactTier, Event, EventCursor, Store};

use super::{Document, Record, Source, digest};

#[derive(Default)]
struct Checkpoint {
    cursor: EventCursor,
    documents: Vec<(Document, String)>,
}

static CHECKPOINTS: Mutex<BTreeMap<PathBuf, Checkpoint>> = Mutex::new(BTreeMap::new());

fn document(offset: u64, event: Event) -> (Document, String) {
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
    (document, detail)
}

pub(super) fn records(store: &Store, records: &mut Vec<Record>) -> Result<()> {
    let key = store.dir().to_path_buf();
    let mut checkpoints = CHECKPOINTS
        .lock()
        .map_err(|e| anyhow::anyhow!("locking search events: {e}"))?;
    let mut checkpoint = checkpoints.remove(&key).unwrap_or_default();
    let mut reader = store.events_since(Some(checkpoint.cursor.clone()))?;
    if reader.read_position() != checkpoint.cursor.offset {
        // The follower restarted from the beginning: the history was replaced or rewritten.
        checkpoint.documents.clear();
    }
    loop {
        let before = reader.read_position();
        match reader.next_event_at()? {
            Some((offset, event, _)) => checkpoint.documents.push(document(offset, event)),
            None if reader.read_position() == before => break,
            None => {}
        }
    }
    checkpoint.cursor = reader.cursor().clone();
    let tail = reader
        .pending_event()
        .map(|event| document(checkpoint.cursor.offset, event));
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
    checkpoints.insert(key, checkpoint);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(kind: &str) -> String {
        format!(
            "{{\"ts\":\"2026-01-01T00:00:00Z\",\"task\":\"T-001\",\"kind\":\"{kind}\",\"detail\":{{}}}}\n"
        )
    }

    fn seen(store: &Store) -> Vec<(String, String)> {
        let mut out = Vec::new();
        records(store, &mut out).unwrap();
        out.into_iter()
            .map(|r| (r.document.reference, r.document.kind))
            .collect()
    }

    fn fresh(store: &Store) -> Vec<(String, String)> {
        CHECKPOINTS.lock().unwrap().remove(store.dir());
        seen(store)
    }

    #[test]
    fn appended_partial_and_replaced_logs_match_a_full_read() {
        // Given a log already indexed once.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::init(dir.path()).unwrap();
        store
            .fixture_replace_events(&(line("created") + &line("moved")))
            .unwrap();
        assert_eq!(seen(&store).len(), 2);

        // When complete and partial lines are appended, then the partial line completes.
        let done = line("done");
        store.fixture_append_events(&line("approved")).unwrap();
        store
            .fixture_append_events(&done[..done.len() - 1])
            .unwrap();
        let partial = seen(&store);
        store.fixture_append_events("\n").unwrap();
        let completed = seen(&store);

        // Then each refresh equals a full read, including the trailing partial line.
        assert_eq!(partial.len(), 4);
        assert_eq!(partial, fresh(&store));
        assert_eq!(completed, fresh(&store));
        assert_eq!(completed, partial);

        // When the log is replaced by a same-length log with other events.
        store
            .fixture_replace_events(&(line("comment") + &line("artifac") + &line("approved")))
            .unwrap();
        let replaced = seen(&store);

        // Then nothing indexed from the old log survives.
        assert_eq!(replaced, fresh(&store));
        assert_eq!(replaced[0].1, "comment");
    }
}
