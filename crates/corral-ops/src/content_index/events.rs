//! Event-history search records, decoding each committed event once per process.
//!
//! Database history is immutable. Records are kept with the follower's sequence cursor;
//! a later refresh decodes only newly committed events.

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

pub(super) fn document(event: Event) -> (Document, String) {
    let detail = event.detail.to_string();
    let document = Document {
        reference: format!("events:{}", event.seq),
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
    loop {
        let before = reader.read_position();
        match reader.next_event_at()? {
            Some((_, event, _)) => checkpoint.documents.push(document(event)),
            None if reader.read_position() == before => break,
            None => {}
        }
    }
    checkpoint.cursor = reader.cursor().clone();
    records.extend(
        checkpoint
            .documents
            .iter()
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
    fn committed_appends_match_a_full_read_without_indexing_staging_input() {
        // Given database events already indexed once.
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
        assert_eq!(partial, fresh(&store));
        store.fixture_append_events("\n").unwrap();
        let completed = seen(&store);

        // Then only committed events are indexed, with stable sequence references.
        assert_eq!(partial.len(), 3);
        assert_eq!(completed.len(), 4);
        assert_eq!(completed, fresh(&store));
        assert_eq!(&completed[..3], &partial);
        assert_eq!(completed[3], ("events:4".into(), "done".into()));
        assert_eq!(seen(&store), completed);
    }

    fn indexed(store: &Store) -> Vec<(String, String, String)> {
        let mut out = Vec::new();
        records(store, &mut out).unwrap();
        out.into_iter()
            .map(|r| {
                let Source::Text(text) = r.source else {
                    panic!("event records carry text");
                };
                (r.document.reference, r.document.kind, text)
            })
            .collect()
    }

    #[test]
    fn events_over_the_follower_line_limit_keep_their_reference_and_text() {
        // Given a document larger than the legacy 64 KiB follower line limit.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::init(dir.path()).unwrap();
        store.fixture_replace_events(&line("created")).unwrap();
        let marker = format!("needle {}", "x".repeat(70 * 1024));
        let large = format!(
            "{{\"ts\":\"2026-01-01T00:00:00Z\",\"task\":\"T-001\",\"kind\":\"commented\",\"detail\":{{\"text\":\"{marker}\"}}}}"
        );
        assert!(large.len() > corral_core::event_reader::EVENT_READ_LIMIT);
        let reference = "events:2".to_string();
        let detail = serde_json::json!({ "text": marker }).to_string();

        // When it is first unterminated, then completed.
        store.fixture_append_events(&large).unwrap();
        let pending = indexed(&store);
        store.fixture_append_events("\n").unwrap();
        let completed = indexed(&store);

        // Then staging remains invisible, and commit indexes the full document at its seq.
        let expected = (reference, "commented".to_string(), detail);
        assert_eq!(pending.len(), 1);
        assert_eq!(completed.last(), Some(&expected));
        assert_eq!(&completed[..1], &pending);
        CHECKPOINTS.lock().unwrap().remove(store.dir());
        assert_eq!(indexed(&store), completed);
    }

    #[test]
    fn a_cleared_fixture_history_starts_an_empty_index() {
        // Given fixture history, before opening a new index checkpoint.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::init(dir.path()).unwrap();
        store.fixture_replace_events(&line("created")).unwrap();

        // When the history disappears.
        store.fixture_remove_events().unwrap();

        // Then a fresh database index contains no events.
        assert!(fresh(&store).is_empty());
    }
}
