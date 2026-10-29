//! Artifact and evidence import (plan T-253 section 12, apply import checks): registered
//! snapshots import as their decoded content, every live registration is accounted for, and
//! copies of artifact content carry no reference that would outlive artifact retention.

#[path = "migrate_support/mod.rs"]
mod support;

use std::io::{Read, Write};

use corral_core::backend::{BlobQuery, BlobStore, PageReq, StoreBackend};
use corral_core::blob::FsBlobStore;
use corral_core::blob_ref::BlobHash;
use corral_core::sqlite::{SqliteBackend, SqliteOptions};
use sha2::{Digest, Sha256};
use support::{FENCE, Legacy, NewArtifact};

const TASK: &str = "T-001-finished-work.md";

fn hash(bytes: &[u8]) -> BlobHash {
    BlobHash::parse(&format!("{:x}", Sha256::digest(bytes))).unwrap()
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

fn backend(legacy: &Legacy) -> SqliteBackend {
    SqliteBackend::open(&legacy.corral, SqliteOptions::default()).unwrap()
}

/// `(refs, unreferenced)` of the blob holding `content`, or `None` when no blob does.
fn blob_refs(legacy: &Legacy, content: &[u8]) -> Option<(u64, bool)> {
    let query = BlobQuery {
        hash: Some(hash(content)),
        state: None,
    };
    let page = backend(legacy)
        .view()
        .unwrap()
        .blobs(&query, PageReq::first(4))
        .unwrap();
    page.items
        .first()
        .map(|row| (row.refs, row.unreferenced_since.is_some()))
}

fn legacy_row(legacy: &Legacy, path: &str) -> corral_core::sqlite::migrate::LegacyFileRow {
    backend(legacy)
        .legacy_file_rows()
        .unwrap()
        .into_iter()
        .find(|row| row.path == path)
        .unwrap_or_else(|| panic!("{path} has no legacy_files row"))
}

#[test]
fn compressed_and_plain_snapshots_import_as_their_decoded_content() {
    // Given a gzip snapshot (compressed_bytes set) beside the fixture's plain report.
    let legacy = Legacy::new();
    let content = b"saved evidence\n".repeat(8);
    let stored = gzip(&content);
    let path = legacy.register_artifact(NewArtifact {
        task_file: TASK,
        kind: "report",
        file: "notes.md",
        content: &content,
        stored: Some(&stored),
        compressed: true,
        purged: false,
    });
    // When the store is migrated.
    assert_eq!(legacy.migrate().apply().unwrap().state, "published");
    // Then each artifact's blob is its decoded content, referenced once (its artifact record).
    for decoded in [content.as_slice(), support::REPORT] {
        assert_eq!(blob_refs(&legacy, decoded), Some((1, false)));
        let mut read = Vec::new();
        FsBlobStore::open(&legacy.corral)
            .unwrap()
            .get(&hash(decoded))
            .unwrap()
            .read_to_end(&mut read)
            .unwrap();
        assert_eq!(read, decoded);
    }
    // And the legacy row keeps the raw gzip file's identity apart from the decoded content.
    let row = legacy_row(&legacy, &path);
    assert_eq!(row.disposition, "artifact");
    assert_eq!(row.sha256.as_deref(), Some(hash(&stored).as_str()));
    assert_eq!(row.bytes, stored.len() as u64);
    assert!(row.blob.is_none());
}

#[test]
fn a_corrupt_snapshot_refuses_naming_its_task_and_artifact() {
    let content = b"the recorded content\n".repeat(4);
    let mut truncated = gzip(&content);
    truncated.truncate(truncated.len() / 2);
    let wrong = b"other content!!\n".repeat(4);
    for (stored, compressed) in [(truncated, true), (wrong, false)] {
        // Given a live artifact whose snapshot does not decode to its recorded content.
        let legacy = Legacy::new();
        let path = legacy.register_artifact(NewArtifact {
            task_file: TASK,
            kind: "report",
            file: "bad.md",
            content: &content,
            stored: Some(&stored),
            compressed,
            purged: false,
        });
        // When the store is migrated, the import refuses naming the task, run and artifact.
        let error = format!("{:#}", legacy.migrate().apply().unwrap_err());
        assert!(
            error.contains("T-001 R1 artifact") && error.contains(&path),
            "{error}"
        );
        // Then nothing is published.
        assert_eq!(legacy.journal().unwrap()["state"], "importing");
        assert!(!legacy.exists("db"));
    }
}

#[test]
fn a_missing_live_snapshot_refuses_before_the_fence() {
    // Given a live registered artifact whose snapshot is missing.
    let legacy = Legacy::new();
    let content = b"evidence that was never saved\n";
    let path = legacy.register_artifact(NewArtifact {
        task_file: TASK,
        kind: "report",
        file: "lost.md",
        content,
        stored: None,
        compressed: false,
        purged: false,
    });
    // When the dry run reports, it lists the artifact and is not ok.
    let dry = legacy.migrate().dry_run().unwrap();
    assert!(!dry.ok);
    assert_eq!(dry.missing_artifacts.len(), 1);
    assert!(dry.missing_artifacts[0].contains(&path));
    // Then apply refuses naming it before the fence; nothing changes.
    let error = format!("{:#}", legacy.migrate().apply().unwrap_err());
    assert!(
        error.contains("T-001 R1 artifact") && error.contains(&path),
        "{error}"
    );
    assert!(legacy.journal().is_none());
    assert_eq!(legacy.fence().as_deref(), Some(FENCE));
}

#[test]
fn purged_stubs_import_without_their_content() {
    // Given two purged artifacts: one stub without a snapshot, one whose snapshot remains.
    let legacy = Legacy::new();
    let gone = b"purged and removed\n";
    let left = b"purged but left behind\n";
    legacy.register_artifact(NewArtifact {
        task_file: TASK,
        kind: "log",
        file: "gone.log",
        content: gone,
        stored: None,
        compressed: false,
        purged: true,
    });
    let path = legacy.register_artifact(NewArtifact {
        task_file: TASK,
        kind: "log",
        file: "left.log",
        content: left,
        stored: Some(left),
        compressed: false,
        purged: true,
    });
    // When the store is migrated, it publishes (a purged stub promises no content).
    assert_eq!(legacy.migrate().apply().unwrap().state, "published");
    // Then neither purged content is stored, and the remaining snapshot is listed without it.
    assert_eq!(blob_refs(&legacy, gone), None);
    assert_eq!(blob_refs(&legacy, left), None);
    let row = legacy_row(&legacy, &path);
    assert_eq!(row.disposition, "artifact_purged");
    assert!(row.blob.is_none());
}

#[test]
fn copies_of_artifact_content_carry_no_reference_of_their_own() {
    // Given a live transcript artifact, a purged one, and legacy review copies of both beside
    // an unrelated review note.
    let legacy = Legacy::new();
    let live = b"agent transcript\n".repeat(16);
    let purged = b"an old transcript\n".repeat(16);
    legacy.register_artifact(NewArtifact {
        task_file: TASK,
        kind: "log",
        file: "agent.transcript.log",
        content: &live,
        stored: Some(&live),
        compressed: false,
        purged: false,
    });
    legacy.register_artifact(NewArtifact {
        task_file: TASK,
        kind: "log",
        file: "old.transcript.log",
        content: &purged,
        stored: None,
        compressed: false,
        purged: true,
    });
    let copy = "state/review-watch/T-001-R1/agent.transcript.log";
    let purged_copy = "state/review-watch/T-001-R1/old.transcript.log";
    legacy.put(copy, &live);
    legacy.put(purged_copy, &purged);
    // When the store is migrated.
    legacy.migrate().apply().unwrap();
    // Then the transcript's only reference is its artifact record, which retention drops; the
    // copy is listed as one, the purged content is not revived, and other evidence is kept.
    assert_eq!(blob_refs(&legacy, &live), Some((1, false)));
    let row = legacy_row(&legacy, copy);
    assert_eq!(
        (row.disposition.as_str(), row.blob.is_none()),
        ("artifact_copy", true)
    );
    let row = legacy_row(&legacy, purged_copy);
    assert_eq!(
        (row.disposition.as_str(), row.blob.is_none()),
        ("purged_artifact_copy", true)
    );
    assert_eq!(blob_refs(&legacy, &purged), None);
    let note = legacy_row(&legacy, "state/reviews/T-001-R1-notes.md");
    assert_eq!(note.disposition, "evidence");
    assert_eq!(blob_refs(&legacy, b"Looks good.\n"), Some((1, false)));
}
