//! Read the actual immutable body of a newly published live-fixture artifact.
use corral_core::backend::BlobStore;
use corral_core::blob_ref::BlobHash;
use corral_core::{Artifact, Store};
use std::io::Read;

pub fn canonical_text(store: &Store, artifact: &Artifact) -> String {
    assert!(
        !store.dir().join(&artifact.path).exists(),
        "new publication has no mirror"
    );
    let hash = BlobHash::parse(&artifact.sha256).unwrap();
    let blobs = store.blob_store().unwrap();
    let mut reader = blobs
        .get(&hash)
        .unwrap()
        .take(artifact.bytes.saturating_add(1));
    let mut text = String::new();
    reader.read_to_string(&mut text).unwrap();
    assert_eq!(
        text.len() as u64,
        artifact.bytes,
        "exact canonical decoded bytes, verified through EOF"
    );
    text
}
