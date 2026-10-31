use corral_core::{ArtifactKind, ArtifactTier, Follow, PinRef, PinTarget, RetentionAction, Store};
use corral_ops::{
    ContentSearch, Ctx, add_session_artifact, all_artifacts, artifact_gc, artifact_read,
    resolve_artifact, search_content, set_artifact_tier, storage_stats,
};

fn fixture() -> (tempfile::TempDir, Ctx) {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::init(temp.path()).unwrap();
    store.config.artifacts.retention.action = RetentionAction::Purge;
    store.config.artifacts.retention.larger_than = Some("1B".into());
    store.create_assistant("pi", None, None, None).unwrap();
    (temp, Ctx::from_store(store))
}

#[test]
fn session_retention_pins_follow_and_revival() {
    let (temp, ctx) = fixture();
    let path = temp.path().join("output.log");
    std::fs::write(&path, "first session artifact\n").unwrap();
    let first = add_session_artifact(&ctx, "A-1", &path, None, None).unwrap();
    assert_eq!(first.run, "");
    assert_eq!(
        first,
        add_session_artifact(&ctx, "A-1", &path, None, None).unwrap()
    );
    let reference = PinRef::of_artifact("A-1", &first).to_string();
    assert_eq!(resolve_artifact(&ctx, &reference).unwrap(), first);
    ctx.store.pin("A-1:output.log", None, true).unwrap();
    std::fs::write(&path, "second session artifact\n").unwrap();
    let second = add_session_artifact(&ctx, "A-1", &path, None, None).unwrap();
    let pins = ctx.store.resolved_pins().unwrap();
    assert_eq!(pins[0].0.follow, Some(Follow::Latest));
    assert!(
        matches!(&pins[0].1, PinTarget::Artifact {artifact, followed: true, ..} if artifact.path == second.path)
    );
    assert_eq!(resolve_artifact(&ctx, "1").unwrap(), second);
    // Both the follow anchor and its current target survive a purge policy.
    assert!(artifact_gc(&ctx, false).unwrap().actions.is_empty());
    ctx.store.unpin("1").unwrap();
    let record_before = std::fs::read(ctx.store.assistant_path("A-1").unwrap()).unwrap();
    let dry = artifact_gc(&ctx, true).unwrap();
    assert_eq!(dry.actions.len(), 2);
    assert_eq!(
        record_before,
        std::fs::read(ctx.store.assistant_path("A-1").unwrap()).unwrap()
    );
    let real = artifact_gc(&ctx, false).unwrap();
    assert_eq!(dry.bytes_saved, real.bytes_saved);
    assert!(
        resolve_artifact(&ctx, &reference)
            .unwrap()
            .purged_at
            .is_some()
    );
    assert!(
        artifact_read(&ctx, &resolve_artifact(&ctx, &reference).unwrap())
            .unwrap()
            .starts_with(b"Artifact content purged")
    );
    let revived = add_session_artifact(&ctx, "A-1", &path, None, None).unwrap();
    assert_eq!(revived.path, second.path);
    assert!(revived.purged_at.is_none());
    set_artifact_tier(&ctx, "A-1:output.log", ArtifactTier::Keep).unwrap();
    assert!(artifact_gc(&ctx, false).unwrap().actions.is_empty());
    assert_eq!(ctx.store.get_assistant("A-1").unwrap().artifacts.len(), 2);
    assert_eq!(storage_stats(&ctx).unwrap().tasks[0].task_id, "A-1");
}

#[test]
fn session_and_task_artifacts_share_content_without_sharing_ownership() {
    let (temp, ctx) = fixture();
    let task = ctx
        .store
        .create("Task", None, corral_core::Status::Backlog, "")
        .unwrap();
    let path = temp.path().join("shared.log");
    std::fs::write(&path, "shared artifact bytes\n").unwrap();
    let task_artifact = corral_ops::add_artifact(&ctx, &task.id, &path, None, None).unwrap();
    let session_artifact = add_session_artifact(&ctx, "A-1", &path, None, None).unwrap();
    assert_ne!(task_artifact.path, session_artifact.path);
    let stats = storage_stats(&ctx).unwrap();
    assert_eq!(stats.tasks.len(), 2);
    assert_eq!(stats.original_bytes, task_artifact.bytes * 2);
    assert_eq!(stats.stored_bytes, task_artifact.bytes);
    assert_eq!(ctx.store.get(&task.id).unwrap().artifacts.len(), 1);
    assert_eq!(ctx.store.get_assistant("A-1").unwrap().artifacts.len(), 1);
}

#[test]
fn session_compression_search_and_end_age() {
    let (temp, mut ctx) = fixture();
    ctx.store.config.artifacts.retention.action = RetentionAction::Compress;
    ctx.store.config.artifacts.retention.larger_than = None;
    ctx.store.config.artifacts.retention.older_than = "1d".into();
    let path = temp.path().join("output.log");
    // The tail lies beyond the extracted-text cap, exercising the shared streaming scanner.
    let body = format!(
        "{}overflow_tail_marker\n",
        "session_search_needle\n".repeat(20_000)
    );
    std::fs::write(&path, &body).unwrap();
    let a = add_session_artifact(&ctx, "A-1", &path, Some(ArtifactKind::Log), None).unwrap();
    assert!(
        artifact_gc(&ctx, false).unwrap().actions.is_empty(),
        "open session is not old"
    );
    ctx.store.end_assistant("A-1").unwrap();
    ctx.store
        .update_assistant("A-1", |s| {
            s.ended = Some("2000-01-01T00:00:00Z".parse().unwrap());
            Ok(())
        })
        .unwrap();
    let gc = artifact_gc(&ctx, false).unwrap();
    assert_eq!(gc.actions[0].action, "compress");
    let compressed = resolve_artifact(&ctx, "A-1:output.log").unwrap();
    assert!(compressed.compressed_bytes.is_some());
    assert_eq!(artifact_read(&ctx, &compressed).unwrap(), body.as_bytes());
    let hits = search_content(
        &ctx,
        "overflow_tail_marker",
        &ContentSearch {
            scope: "artifacts".into(),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].artifact_ref.as_deref(),
        Some(PinRef::of_artifact("A-1", &a).to_string().as_str())
    );
    let rows = all_artifacts(&ctx).unwrap();
    let search = corral_ops::ArtifactSearch::load(&ctx).unwrap();
    assert!(
        search
            .matches(&ctx, &rows[0], &["overflow_tail_marker"], "")
            .unwrap()
    );
    assert!(
        ctx.store.list().unwrap().is_empty(),
        "session projections must never create task files"
    );
}
