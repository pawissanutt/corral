use super::*;
use corral_core::{
    Artifact, Store,
    work::{self, Counter, HotScope},
};

fn artifact(body: &str, kind: ArtifactKind) -> (tempfile::TempDir, Ctx, Artifact) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    let task = store
        .create("Signal", None, corral_core::Status::Done, "")
        .unwrap();
    let ctx = crate::resources::FixedResourceProbe::ctx(store);
    let source = root.path().join("input.txt");
    fs::write(&source, body).unwrap();
    let artifact =
        crate::add_artifact(&ctx, &task.id, &source, Some(kind), Some("signal")).unwrap();
    assert!(
        !ctx.store.dir().join(&artifact.path).exists(),
        "canonical publication has no mirror"
    );
    (root, ctx, artifact)
}

#[test]
fn canonical_recent_log_keeps_line_boundaries_and_exposes_full_selected_stream_io() {
    for scale in [1, 10] {
        let input = format!(
            "{}late error in src/recent.rs\nlast line",
            "ordinary output\n".repeat(5000 * scale)
        );
        let (_root, ctx, artifact) = artifact(&input, ArtifactKind::Log);
        let hot = HotScope::enter();
        let before = work::snapshot();
        let selected = artifact_text(&ctx, &artifact, ARTIFACT_BYTES)
            .unwrap()
            .unwrap();
        let report = work::snapshot().since(&before);
        drop(hot);
        let expected = recent_lines(input.as_bytes(), ARTIFACT_BYTES);
        assert_eq!(selected.as_bytes(), expected);
        assert!(selected.contains("late error in src/recent.rs"));
        assert!(selected.len() <= ARTIFACT_BYTES);
        report.check_identity().unwrap();
        assert_eq!(report.unattributed_hydrations(), 0);
        assert!(
            report.followup_calls.is_empty(),
            "no scope exemption for selected body I/O"
        );
        let io = report.cells(Counter::SelectedArtifactBytesRead);
        assert_eq!(io.total, input.len() as u64);
        assert_eq!(io.unattributed, io.total);
        assert_eq!(io.own_record + io.emitted + io.followup_sum(), 0);
        println!(
            "selected={} raw_ceiling={} bytes_streamed={} retained={} cap={ARTIFACT_BYTES}",
            artifact.sha256,
            artifact.bytes,
            io.total,
            selected.len()
        );
    }
}

#[test]
fn canonical_diff_prefix_and_imported_gzip_log_keep_the_original_logical_limits() {
    let input = format!(
        "--- a/src/first.rs\n+++ b/src/first.rs\n{}late src/last.rs\n",
        "filler\n".repeat(20000)
    );
    let (_root, mut ctx, mut artifact) = artifact(&input, ArtifactKind::Log);
    ctx.store.config.artifacts.retention.larger_than = Some("1B".into());
    ctx.store.config.artifacts.retention.action = corral_core::RetentionAction::Compress;
    // Explicit legacy imported copy, not a production mirror. Its retention metadata selects
    // the old gzip-prefix semantics; bytes still come from the verified canonical blob.
    let imported = ctx.store.dir().join(&artifact.path);
    fs::create_dir_all(imported.parent().unwrap()).unwrap();
    fs::write(imported, &input).unwrap();
    crate::artifact_gc(&ctx, false).unwrap();
    artifact = ctx.store.get("T-001").unwrap().artifacts.remove(0);
    assert!(artifact.compressed_bytes.is_some());
    assert_eq!(
        artifact_text(&ctx, &artifact, ARTIFACT_BYTES)
            .unwrap()
            .unwrap()
            .as_bytes(),
        &input.as_bytes()[..ARTIFACT_BYTES]
    );
    assert!(
        artifact_text(&ctx, &artifact, 1).unwrap().is_none(),
        "compressed input limit retained"
    );
    let (_root, ctx, artifact) = self::artifact(&input, ArtifactKind::Diff);
    assert_eq!(
        artifact_text(&ctx, &artifact, ARTIFACT_BYTES)
            .unwrap()
            .unwrap()
            .as_bytes(),
        &input.as_bytes()[..ARTIFACT_BYTES]
    );
}

#[test]
fn cold_collection_rejects_corrupt_content_without_blocking_other_valid_sources() {
    let (root, ctx, bad) = artifact("+++ b/src/corrupt.rs\n", ArtifactKind::Diff);
    let source = root.path().join("good.diff");
    fs::write(&source, "+++ b/src/valid.rs\n").unwrap();
    crate::add_artifact(
        &ctx,
        "T-001",
        &source,
        Some(ArtifactKind::Diff),
        Some("valid"),
    )
    .unwrap();
    let hash = corral_core::blob_ref::BlobHash::parse(&bad.sha256).unwrap();
    fs::write(
        ctx.store.blob_store().unwrap().path_of(&hash),
        "altered frame",
    )
    .unwrap();
    assert!(
        artifact_text(&ctx, &bad, ARTIFACT_BYTES).is_err(),
        "verification still refuses"
    );
    let actual = crate::lesson_rows::Source {
        task: ctx.store.get("T-001").unwrap(),
        usage: Default::default(),
    };
    let signals = collect(&ctx, "app", &[actual], &[], &Default::default()).unwrap();
    assert!(
        signals
            .iter()
            .any(|signal| signal.files.iter().any(|path| path == "src/valid.rs"))
    );
    assert!(
        signals
            .iter()
            .all(|signal| signal.files.iter().all(|path| path != "src/corrupt.rs"))
    );
}

#[test]
fn canonical_signal_rejects_bad_lengths_unsafe_paths_and_corruption() {
    let (_root, ctx, original) = artifact("src/real.rs\n", ArtifactKind::Diff);
    let mut changed = original.clone();
    changed.bytes += 1;
    assert!(artifact_text(&ctx, &changed, 64).is_err());
    changed = original.clone();
    changed.path = "../outside".into();
    assert!(artifact_text(&ctx, &changed, 64).unwrap().is_none());
    changed = original.clone();
    changed.purged_at = Some(jiff::Timestamp::UNIX_EPOCH);
    assert!(artifact_text(&ctx, &changed, 64).unwrap().is_none());
    let hash = corral_core::blob_ref::BlobHash::parse(&original.sha256).unwrap();
    fs::write(
        ctx.store.blob_store().unwrap().path_of(&hash),
        "altered frame",
    )
    .unwrap();
    assert!(artifact_text(&ctx, &original, 64).is_err());
}
