use std::fs;

use corral_core::{Artifact, ArtifactKind, Status, Store, Task, render_task};
use serde_json::json;

use super::*;
use crate::content_index::SNAPSHOTS;
use crate::{ContentSearch, search_content};

struct Fixture {
    _tmp: tempfile::TempDir,
    ctx: Ctx,
    tasks: Vec<Task>,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::init(tmp.path()).unwrap();
        let task = store
            .create("Refresh", None, Status::Done, "original")
            .unwrap();
        let tasks = vec![store.get(&task.id).unwrap()];
        Self {
            _tmp: tmp,
            ctx: crate::resources::FixedResourceProbe::ctx(store),
            tasks,
        }
    }

    fn refresh(&self) {
        refresh_search_index_with_tasks(&self.ctx, &self.tasks).unwrap();
    }

    fn save(&self) {
        // No event or updated timestamp change: isolate the indexed field itself.
        fs::write(&self.tasks[0].path, render_task(&self.tasks[0])).unwrap();
    }

    fn add(&mut self) {
        let artifact = Artifact {
            run: "R0".into(),
            kind: ArtifactKind::Evidence,
            title: "Output".into(),
            path: "artifacts/output.txt".into(),
            source: String::new(),
            sha256: "test-content-identity".into(),
            bytes: 13,
            added: self.tasks[0].created,
            tier_override: None,
            compressed_bytes: None,
            purged_at: None,
        };
        let path = self.ctx.store.dir().join(&artifact.path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "artifactprobe").unwrap();
        self.tasks[0].artifacts.push(artifact);
        self.save();
    }

    fn rebuilds(&self) {
        let before = SNAPSHOTS.get();
        self.refresh();
        assert_eq!(SNAPSHOTS.get(), before + 1, "refresh must invoke snapshot");
    }

    fn cached(&self, token: &str) -> bool {
        let path = self.ctx.store.state_dir().join("content-search.json");
        let cache: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        cache["documents"]
            .as_object()
            .unwrap()
            .values()
            .any(|doc| doc["text"].as_str().unwrap().contains(token))
    }

    fn searchable(&self, token: &str) -> bool {
        !search_content(&self.ctx, token, &ContentSearch::default())
            .unwrap()
            .is_empty()
    }
}

#[test]
fn unchanged_refresh_skips_snapshot_and_preserves_index_files() {
    // Given a successful refresh, including its persisted fingerprint.
    let f = Fixture::new();
    f.refresh();
    let index = f.ctx.store.state_dir().join("content-search.json");
    let stamp = file_stamp(&index).unwrap();
    let fingerprint =
        file_stamp(&f.ctx.store.state_dir().join("content-search.fingerprint")).unwrap();
    let before = SNAPSHOTS.get();

    // When refreshing with the same task listing and a reopened context.
    refresh_search_index_with_tasks(&Ctx::from_store(f.ctx.store.clone()), &f.tasks).unwrap();

    // Then neither snapshot nor either file write is attempted.
    assert_eq!(SNAPSHOTS.get(), before);
    assert_eq!(file_stamp(&index).unwrap(), stamp);
    assert_eq!(
        file_stamp(&f.ctx.store.state_dir().join("content-search.fingerprint")).unwrap(),
        fingerprint
    );
}

#[test]
fn body_change_rebuilds_and_becomes_searchable() {
    // Given an indexed task.
    let mut f = Fixture::new();
    f.refresh();
    f.tasks[0].body = "bodyprobe".into();
    f.save();

    // When only its body changes, with no event or timestamp change.
    f.rebuilds();

    // Then refresh itself indexed the body before search can repair the cache.
    assert!(f.cached("bodyprobe"));
    assert!(f.searchable("bodyprobe"));
}

#[test]
fn new_artifact_rebuilds_and_becomes_searchable() {
    // Given an indexed task without artifacts.
    let mut f = Fixture::new();
    f.refresh();
    f.add();

    // When attaching a new artifact without an event.
    f.rebuilds();

    // Then the refreshed index and search both contain its content.
    assert!(f.cached("artifactprobe"));
    assert!(f.searchable("artifactprobe"));
}

#[test]
fn removed_artifact_rebuilds_and_disappears_from_search() {
    // Given indexed artifact text whose metadata remains on the task.
    let mut f = Fixture::new();
    f.add();
    f.refresh();
    fs::remove_file(f.ctx.store.dir().join(&f.tasks[0].artifacts[0].path)).unwrap();

    // When only the snapshot file disappears.
    f.rebuilds();

    // Then cached text is dropped rather than reused by its original SHA.
    assert!(!f.cached("artifactprobe"));
    assert!(!f.searchable("artifactprobe"));
}

#[test]
fn purged_artifact_rebuilds_and_disappears_from_search() {
    // Given indexed artifact text with an unchanged file.
    let mut f = Fixture::new();
    f.add();
    f.refresh();
    f.tasks[0].artifacts[0].purged_at = Some(f.tasks[0].updated);
    f.save();

    // When only retention metadata marks it purged.
    f.rebuilds();

    // Then refresh removes its cached content and search excludes it.
    assert!(!f.cached("artifactprobe"));
    assert!(!f.searchable("artifactprobe"));
}

#[test]
fn modified_artifact_rebuilds_without_reusing_its_declared_sha_text() {
    // Given indexed text with a declared SHA unchanged by an external replacement.
    let mut f = Fixture::new();
    f.add();
    f.refresh();
    fs::write(
        f.ctx.store.dir().join(&f.tasks[0].artifacts[0].path),
        "replacementprobe",
    )
    .unwrap();

    // When file metadata, but not task metadata, changes.
    f.rebuilds();

    // Then canonical replacement bytes replace cached SHA-identified text.
    assert!(f.cached("replacementprobe"));
    assert!(!f.cached("artifactprobe"));
    assert!(f.searchable("replacementprobe"));
}

#[test]
fn appended_event_rebuilds_and_becomes_searchable() {
    // Given an unchanged task and a built index.
    let f = Fixture::new();
    f.refresh();
    f.ctx
        .store
        .log_event(
            Some(&f.tasks[0].id),
            "changes_requested",
            json!({
                "text": "eventprobe", "run": "R0"
            }),
        )
        .unwrap();

    // When only the event history changes.
    f.rebuilds();

    // Then the event is indexed before searching.
    assert!(f.cached("eventprobe"));
    assert!(f.searchable("eventprobe"));
}

#[test]
fn explicit_reindex_rebuilds_even_with_unchanged_inputs() {
    // Given unchanged indexed content.
    let f = Fixture::new();
    f.refresh();
    let before = SNAPSHOTS.get();

    // When explicit reindex is requested through content search.
    search_content(
        &f.ctx,
        "original",
        &ContentSearch {
            reindex: true,
            ..Default::default()
        },
    )
    .unwrap();

    // Then a snapshot still runs regardless of the refresh fingerprint.
    assert_eq!(SNAPSHOTS.get(), before + 1);
}

#[test]
fn refresh_skips_snapshot_after_an_explicit_canonical_reindex() {
    // Given a canonical reindex built through the search path.
    let f = Fixture::new();
    search_content(
        &f.ctx,
        "original",
        &ContentSearch {
            reindex: true,
            ..Default::default()
        },
    )
    .unwrap();
    let before = SNAPSHOTS.get();

    // When refreshing the same inputs.
    f.refresh();

    // Then the explicit build's fingerprint prevents another snapshot.
    assert_eq!(SNAPSHOTS.get(), before);
}

#[test]
fn sync_refresh_relists_when_tasks_change_after_its_sample() {
    // Given sync's task listing, sampled before reconciliation.
    let f = Fixture::new();
    f.refresh();
    let stamp = task_stamp(&f.ctx).unwrap();
    f.ctx
        .store
        .update(&f.tasks[0].id, |task| {
            task.body = "reconciledprobe".into();
            Ok(())
        })
        .unwrap();

    // When sync refreshes using its now stale listing.
    refresh_after_sync(&f.ctx, &f.tasks, &stamp).unwrap();

    // Then the current body, not the stale sampled body, is indexed.
    assert!(f.cached("reconciledprobe"));
}
