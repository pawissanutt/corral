use super::*;
use crate::content_index::ARTIFACT_EXTRACTIONS;

#[test]
fn appended_event_reuses_unchanged_artifact_text() {
    // Given an indexed artifact and one appended event, with no file changes.
    let mut f = Fixture::new();
    f.add();
    f.refresh();
    f.ctx
        .store
        .log_event(None, "probe", json!({"text": "incrementalprobe"}))
        .unwrap();
    let before = ARTIFACT_EXTRACTIONS.get();

    // When the changed fingerprint triggers a refresh.
    f.refresh();

    // Then the event is indexed without extracting the artifact again.
    assert!(f.cached("incrementalprobe"));
    assert!(f.cached("artifactprobe"));
    assert_eq!(ARTIFACT_EXTRACTIONS.get(), before);
}

#[test]
fn artifact_replacement_reextracts_only_the_changed_document() {
    // Given two cached artifacts and a replacement of just the first file.
    let mut f = Fixture::new();
    f.add();
    let mut other = f.tasks[0].artifacts[0].clone();
    other.path = "artifacts/unchanged.txt".into();
    other.sha256 = "unchanged-content-identity".into();
    fs::write(f.ctx.store.dir().join(&other.path), "unchangedprobe").unwrap();
    f.tasks[0].artifacts.push(other);
    f.save();
    f.refresh();
    fs::write(
        f.ctx.store.dir().join(&f.tasks[0].artifacts[0].path),
        "replacementprobe",
    )
    .unwrap();
    let before = ARTIFACT_EXTRACTIONS.get();

    // When metadata changes without changing the first artifact's declared SHA.
    f.refresh();

    // Then only the changed document is extracted, keeping the other cached text.
    assert!(f.cached("replacementprobe"));
    assert!(!f.cached("artifactprobe"));
    assert!(f.cached("unchangedprobe"));
    assert_eq!(ARTIFACT_EXTRACTIONS.get(), before + 1);
}

#[test]
fn changed_refresh_publishes_a_reusable_fingerprint() {
    // Given a successful incremental refresh after an event append.
    let f = Fixture::new();
    f.refresh();
    f.ctx
        .store
        .log_event(None, "probe", json!({"text": "incrementalprobe"}))
        .unwrap();
    f.refresh();
    let before = SNAPSHOTS.get();

    // When the unchanged inputs are refreshed again.
    f.refresh();

    // Then the successful changed refresh's observation skips another snapshot.
    assert_eq!(SNAPSHOTS.get(), before);
}
