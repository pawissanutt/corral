use super::*;
use crate::content_index::ARTIFACT_EXTRACTIONS;
use corral_core::{CommentAuthor, TaskComment};

#[test]
fn overflow_artifacts_are_not_extracted_into_a_full_cache() {
    // Given 4096 newer specs/comments and an older artifact outside the cache.
    let mut f = Fixture::new();
    f.add();
    f.tasks[0].artifacts[0].added = "2000-01-01T00:00:00Z".parse().unwrap();
    let newer = "2099-01-01T00:00:00Z".parse().unwrap();
    f.tasks[0].updated = newer;
    f.tasks[0].comments = (0..4095)
        .map(|_| TaskComment {
            author: CommentAuthor::Owner,
            at: newer,
            text: "newer context".into(),
        })
        .collect();
    let before = ARTIFACT_EXTRACTIONS.get();

    // When refreshing a cache filled by those newer documents.
    f.refresh();

    // Then no artifact bytes are read merely to discard them at the cache ceiling.
    assert_eq!(ARTIFACT_EXTRACTIONS.get(), before);
    assert!(!f.cached("artifactprobe"));
}
