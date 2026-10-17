use corral_core::{Status, Store};

use super::*;

fn fixture() -> (tempfile::TempDir, Ctx, String) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    let task = store.create("Report", None, Status::Backlog, "").unwrap();
    let ctx = Ctx {
        store,
        herdr: corral_herdr::Herdr::new(Some("corral-test".into())),
        resource_probe: crate::resources::FixedResourceProbe::arc(),
    };
    (root, ctx, task.id)
}

#[test]
fn raster_thumbnail_uses_immutable_snapshot_not_source() {
    // Given a raster artifact whose worktree source has subsequently disappeared.
    let (root, ctx, id) = fixture();
    let source = root.path().join("chart.PNG");
    std::fs::write(&source, b"image bytes").unwrap();
    let artifact = crate::add_artifact(&ctx, &id, &source, None, None).unwrap();
    std::fs::remove_file(source).unwrap();
    // When the gallery resolves its thumbnail.
    let path = artifact_thumbnail(&ctx, &artifact).unwrap().unwrap();
    // Then the original immutable snapshot is returned.
    assert_eq!(std::fs::read(path).unwrap(), b"image bytes");
}

#[test]
fn html_capture_is_cached_and_missing_preview_is_explicit() {
    // Given a snapshotted HTML artifact and a browser process that emits a PNG.
    let (root, ctx, id) = fixture();
    let source = root.path().join("report with spaces.html");
    std::fs::write(&source, "<h1>Report</h1>").unwrap();
    let artifact = crate::add_artifact(&ctx, &id, &source, None, None).unwrap();
    assert_eq!(artifact_thumbnail(&ctx, &artifact).unwrap(), None);
    // An immutable executable cannot retain a writer inherited by another test's fork.
    let browser = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/thumbnail-browser.sh");
    // When completion captures the page.
    capture(&ctx, &artifact, &browser).unwrap();
    // Then its cached PNG is available, survives source deletion, and needs no new browser process.
    std::fs::remove_file(source).unwrap();
    let path = artifact_thumbnail(&ctx, &artifact).unwrap().unwrap();
    assert!(
        std::fs::read(path)
            .unwrap()
            .starts_with(b"\x89PNG\r\n\x1a\n")
    );
    capture(&ctx, &artifact, Path::new("/nonexistent/browser")).unwrap();
}

#[test]
fn failed_capture_never_publishes_partial_thumbnail() {
    // Given a browser that exits successfully but produces no image.
    let (root, ctx, id) = fixture();
    let source = root.path().join("report.html");
    std::fs::write(&source, "<h1>Report</h1>").unwrap();
    let artifact = crate::add_artifact(&ctx, &id, &source, None, None).unwrap();
    // When thumbnail generation fails.
    assert!(capture(&ctx, &artifact, Path::new("/bin/true")).is_err());
    // Then the original is still readable and the gallery has no misleading cache file.
    assert_eq!(artifact_thumbnail(&ctx, &artifact).unwrap(), None);
    assert!(crate::artifact_path(&ctx, &artifact).unwrap().is_file());
}
