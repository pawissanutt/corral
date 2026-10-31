//! Canonical blob authority must not weaken the artifact reader's no-follow/type boundary.
#![cfg(unix)]
use corral_core::{backend::BlobStore, blob::FsBlobStore};
use std::io::Read;

#[test]
fn blob_reader_rejects_identical_content_through_leaf_or_ancestor_symlinks() {
    for ancestor in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let blobs = FsBlobStore::at(dir.path().join("blobs")).unwrap();
        let staged = blobs.stage(&mut &b"immutable proof"[..]).unwrap();
        let hash = staged.hash.clone();
        blobs.put_staged(staged).unwrap();
        let path = blobs.path_of(&hash);
        let selected = if ancestor {
            path.parent().unwrap().to_path_buf()
        } else {
            path.clone()
        };
        let outside = dir.path().join("outside");
        std::fs::rename(&selected, &outside).unwrap();
        std::os::unix::fs::symlink(&outside, &selected).unwrap();
        assert!(blobs.get(&hash).is_err(), "ancestor={ancestor}");
        std::fs::remove_file(&selected).unwrap();
        std::fs::rename(outside, selected).unwrap();
        let mut restored = Vec::new();
        blobs
            .get(&hash)
            .unwrap()
            .read_to_end(&mut restored)
            .unwrap();
        assert_eq!(restored, b"immutable proof");
    }
}

#[test]
fn blob_reader_rejects_nonregular_content_before_reading() {
    let dir = tempfile::tempdir().unwrap();
    let blobs = FsBlobStore::at(dir.path().join("blobs")).unwrap();
    let staged = blobs.stage(&mut &b"proof"[..]).unwrap();
    let hash = staged.hash.clone();
    blobs.put_staged(staged).unwrap();
    let path = blobs.path_of(&hash);
    std::fs::remove_file(&path).unwrap();
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .unwrap()
            .success()
    );
    // A missing O_NONBLOCK would hang at open; no writer is ever attached to this FIFO.
    assert!(blobs.get(&hash).is_err());
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(blobs.get(&hash).is_err());
}
