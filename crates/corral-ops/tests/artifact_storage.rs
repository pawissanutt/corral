use std::fs;
use std::io::Write;
use std::path::Path;

use corral_core::{Artifact, ArtifactKind, ArtifactTier, Event, RetentionAction, Status, Store};
use corral_ops::{
    Ctx, Preview, add_artifact, artifact_gc, artifact_path, artifact_preview, artifact_read,
    resolve_artifact, set_artifact_tier, storage_stats,
};
use serde_json::json;

struct Fixture {
    tmp: tempfile::TempDir,
    ctx: Ctx,
}

impl Fixture {
    fn new(action: RetentionAction) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = Store::init(tmp.path()).unwrap();
        store.config.artifacts.retention.action = action;
        store.config.artifacts.retention.larger_than = Some("1B".into());
        Self {
            tmp,
            ctx: Ctx::from_store(store),
        }
    }

    fn task(&self) -> String {
        self.ctx
            .store
            .create("Storage fixture", None, Status::Done, "")
            .unwrap()
            .id
    }

    fn add(&self, task: &str, name: &str, bytes: &[u8], kind: ArtifactKind) -> Artifact {
        let path = self.tmp.path().join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        add_artifact(&self.ctx, task, &path, Some(kind), None).unwrap()
    }

    fn current(&self, task: &str, a: &Artifact) -> Artifact {
        self.ctx
            .store
            .get(task)
            .unwrap()
            .artifacts
            .into_iter()
            .find(|other| other.path == a.path)
            .unwrap()
    }

    fn reference(task: &str, a: &Artifact) -> String {
        corral_core::PinRef::of_artifact(task, a).to_string()
    }

    fn path(&self, a: &Artifact) -> std::path::PathBuf {
        self.ctx.store.dir().join(&a.path)
    }

    fn finish_at(&self, task: &str, at: &str) {
        let event = Event {
            task: Some(task.into()),
            ts: at.parse().unwrap(),
            kind: "moved".into(),
            detail: json!({"to": "done"}),
        };
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(self.ctx.store.dir().join("events.jsonl"))
            .unwrap();
        writeln!(file, "{}", serde_json::to_string(&event).unwrap()).unwrap();
    }
}

#[cfg(unix)]
fn same_inode(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let a = fs::metadata(a).unwrap();
    let b = fs::metadata(b).unwrap();
    (a.dev(), a.ino()) == (b.dev(), b.ino())
}

#[test]
#[cfg(unix)]
fn identical_contents_share_storage_across_tasks() {
    // Given separate tasks with identical source bytes.
    let f = Fixture::new(RetentionAction::Keep);
    let first = f.task();
    let second = f.task();
    let bytes = b"shared bytes";
    let a = f.add(&first, "one.txt", bytes, ArtifactKind::Evidence);
    // When the second snapshot is attached.
    let b = f.add(&second, "two.txt", bytes, ArtifactKind::Evidence);
    // Then references stay distinct while storage is shared.
    assert_ne!(a.path, b.path);
    assert!(same_inode(&f.path(&a), &f.path(&b)));
    let stats = storage_stats(&f.ctx).unwrap();
    assert_eq!(stats.original_bytes, 24);
    assert_eq!(stats.stored_bytes, 12);
    assert_eq!(stats.tasks.len(), 2);
}

#[test]
fn gzip_keeps_stable_paths_and_transparent_text_reads() {
    // Given a compressible text snapshot.
    let f = Fixture::new(RetentionAction::Compress);
    let task = f.task();
    let bytes = "storage line\n".repeat(300);
    let a = f.add(&task, "output.log", bytes.as_bytes(), ArtifactKind::Log);
    // When explicit GC runs.
    let report = artifact_gc(&f.ctx, false).unwrap();
    // Then metadata preserves original bytes and all text access is decoded.
    let current = f.current(&task, &a);
    assert_eq!(current.path, a.path);
    assert_eq!(current.bytes, a.bytes);
    assert_eq!(fs::read(f.path(&a)).unwrap()[..2], [0x1f, 0x8b]);
    assert_eq!(
        current.compressed_bytes,
        Some(fs::metadata(f.path(&a)).unwrap().len())
    );
    assert_eq!(artifact_read(&f.ctx, &current).unwrap(), bytes.as_bytes());
    assert_eq!(
        artifact_preview(&f.ctx, &current).unwrap(),
        Preview::Text(bytes.clone())
    );
    assert!(!f.ctx.store.state_dir().join("artifact-decoded").exists());
    assert_eq!(
        report.bytes_saved,
        a.bytes - current.compressed_bytes.unwrap()
    );
    let path = artifact_path(&f.ctx, &current).unwrap();
    assert_eq!(path.extension().unwrap(), "log");
    assert_eq!(fs::read(path).unwrap(), bytes.as_bytes());
}

#[test]
fn dry_run_predicts_exact_dedup_and_compression_without_writes() {
    // Given legacy independent identical copies.
    let f = Fixture::new(RetentionAction::Compress);
    let first = f.task();
    let second = f.task();
    let a = f.add(&first, "a.log", &vec![b'A'; 12_000], ArtifactKind::Log);
    let b = f.add(&second, "b.log", &vec![b'A'; 12_000], ArtifactKind::Log);
    let independent = f.tmp.path().join("independent");
    fs::copy(f.path(&b), &independent).unwrap();
    fs::rename(independent, f.path(&b)).unwrap();
    let events = fs::read(f.ctx.store.dir().join("events.jsonl")).unwrap();
    let before = storage_stats(&f.ctx).unwrap().stored_bytes;
    let task_before = fs::read(f.ctx.store.get(&first).unwrap().path).unwrap();
    let cache = f.ctx.store.state_dir().join("artifact-decoded");
    fs::create_dir_all(&cache).unwrap();
    fs::write(cache.join("sentinel"), "retained during dry run").unwrap();
    // When GC is simulated.
    let dry = artifact_gc(&f.ctx, true).unwrap();
    // Then no metadata, content or cache changes; execution matches the estimate.
    assert_eq!(f.current(&first, &a), a);
    assert_eq!(
        fs::read(f.ctx.store.get(&first).unwrap().path).unwrap(),
        task_before
    );
    assert_eq!(
        fs::read(f.ctx.store.dir().join("events.jsonl")).unwrap(),
        events
    );
    assert_eq!(fs::read(f.path(&a)).unwrap(), vec![b'A'; 12_000]);
    assert!(cache.join("sentinel").exists());
    assert!(dry.actions.iter().any(|a| a.action == "dedup"));
    let applied = artifact_gc(&f.ctx, false).unwrap();
    assert_eq!(dry.bytes_saved, applied.bytes_saved);
    assert_eq!(
        dry.stored_after,
        storage_stats(&f.ctx).unwrap().stored_bytes
    );
    assert_eq!(dry.bytes_saved, before - dry.stored_after);
    assert!(!cache.exists());
    #[cfg(unix)]
    assert!(same_inode(&f.path(&a), &f.path(&b)));
}

#[test]
fn purge_leaves_resolvable_metadata_and_a_readable_stub() {
    // Given an eligible artifact.
    let f = Fixture::new(RetentionAction::Purge);
    let task = f.task();
    let a = f.add(&task, "old.html", &vec![b'X'; 2000], ArtifactKind::Report);
    set_artifact_tier(&f.ctx, &Fixture::reference(&task, &a), ArtifactTier::Normal).unwrap();
    let reference = Fixture::reference(&task, &a);
    // When content is purged.
    artifact_gc(&f.ctx, false).unwrap();
    // Then stable refs and metadata survive and all access has meaningful text.
    let current = resolve_artifact(&f.ctx, &reference).unwrap();
    assert!(current.purged_at.is_some());
    assert_eq!(current.sha256, a.sha256);
    assert_eq!(current.bytes, a.bytes);
    assert!(!f.path(&a).exists());
    assert!(!artifact_read(&f.ctx, &current).unwrap().is_empty());
    assert!(matches!(
        artifact_preview(&f.ctx, &current).unwrap(),
        Preview::Text(_)
    ));
    let stub = artifact_path(&f.ctx, &current).unwrap();
    assert_eq!(stub.extension().unwrap(), "txt");
    assert_eq!(
        fs::read(stub).unwrap(),
        artifact_read(&f.ctx, &current).unwrap()
    );
    assert_eq!(storage_stats(&f.ctx).unwrap().stored_bytes, 0);
}

#[test]
fn keep_pins_follow_latest_and_screenshots_are_protected() {
    // Given keep content, a followed pin, and a screenshot referenced by a report.
    let f = Fixture::new(RetentionAction::Purge);
    let task = f.task();
    let plan = f.add(&task, "plan.md", b"protected plan", ArtifactKind::Plan);
    let anchor = f.add(&task, "report.txt", b"first report", ArtifactKind::Evidence);
    f.ctx
        .store
        .pin(&Fixture::reference(&task, &anchor), None, true)
        .unwrap();
    let latest = f.add(
        &task,
        "report.txt",
        b"latest report",
        ArtifactKind::Evidence,
    );
    let image = f.add(
        &task,
        "screenshot.png",
        b"png content",
        ArtifactKind::Screenshot,
    );
    let pinned = f.add(&task, "pinned.log", b"pinned content", ArtifactKind::Log);
    f.ctx
        .store
        .pin(&Fixture::reference(&task, &pinned), None, false)
        .unwrap();
    // When retention runs.
    let report = artifact_gc(&f.ctx, false).unwrap();
    // Then both anchors and latest targets remain untouched.
    assert!(report.actions.is_empty());
    for a in [plan, anchor, latest, image, pinned] {
        assert_eq!(f.current(&task, &a), a);
        assert!(f.path(&a).is_file());
    }
}

#[test]
#[cfg(unix)]
fn compressing_legacy_duplicates_does_not_mutate_protected_copy() {
    // Given a protected copy and two legacy independent copies of the same bytes.
    let f = Fixture::new(RetentionAction::Compress);
    let keep_task = f.task();
    let first = f.task();
    let second = f.task();
    let bytes = vec![b'K'; 10_000];
    let keep = f.add(&keep_task, "plan.md", &bytes, ArtifactKind::Plan);
    let a = f.add(&first, "a.log", &bytes, ArtifactKind::Log);
    let b = f.add(&second, "b.log", &bytes, ArtifactKind::Log);
    for copy in [&a, &b] {
        let independent = f.tmp.path().join("independent");
        fs::copy(f.path(copy), &independent).unwrap();
        fs::rename(independent, f.path(copy)).unwrap();
    }
    let before = storage_stats(&f.ctx).unwrap().stored_bytes;
    // When dedup and compression run.
    let dry = artifact_gc(&f.ctx, true).unwrap();
    artifact_gc(&f.ctx, false).unwrap();
    // Then gzip copies share storage but the protected original is still raw.
    assert_eq!(fs::read(f.path(&keep)).unwrap(), bytes);
    assert_eq!(f.current(&keep_task, &keep), keep);
    assert!(!same_inode(&f.path(&keep), &f.path(&a)));
    assert!(same_inode(&f.path(&a), &f.path(&b)));
    assert_eq!(
        artifact_read(&f.ctx, &f.current(&second, &b)).unwrap(),
        bytes
    );
    assert_eq!(
        dry.bytes_saved,
        before - storage_stats(&f.ctx).unwrap().stored_bytes
    );
}

#[test]
fn retention_uses_task_finish_events_not_artifact_age() {
    // Given a newly added artifact on an old finished task.
    let mut f = Fixture::new(RetentionAction::Purge);
    f.ctx.store.config.artifacts.retention.larger_than = None;
    let task = f.task();
    f.finish_at(&task, "2000-01-01T00:00:00Z");
    let a = f.add(
        &task,
        "new.log",
        b"new artifact on old task",
        ArtifactKind::Log,
    );
    // When age-only retention runs after artifact attachment updated the task.
    artifact_gc(&f.ctx, false).unwrap();
    // Then the finishing event still controls retention.
    assert!(f.current(&task, &a).purged_at.is_some());
}

#[test]
fn old_artifact_on_active_task_is_not_age_eligible() {
    // Given an active task with an ancient artifact and no size threshold.
    let mut f = Fixture::new(RetentionAction::Purge);
    f.ctx.store.config.artifacts.retention.larger_than = None;
    let task = f.task();
    let a = f.add(&task, "old.log", b"still needed", ArtifactKind::Log);
    f.ctx
        .store
        .update(&task, |t| {
            t.status = Status::Running;
            t.artifacts[0].added = "2000-01-01T00:00:00Z".parse().unwrap();
            Ok(())
        })
        .unwrap();
    // When age-only retention runs.
    let report = artifact_gc(&f.ctx, false).unwrap();
    // Then artifact age alone cannot trigger it.
    assert!(report.actions.is_empty());
    assert!(f.path(&a).exists());
}

#[test]
fn tier_selection_and_explicit_override_are_honored() {
    // Given only transient tiers selected and a normal artifact explicitly kept.
    let mut f = Fixture::new(RetentionAction::Purge);
    f.ctx.store.config.artifacts.retention.tiers = vec![ArtifactTier::Transient];
    let task = f.task();
    let normal = f.add(
        &task,
        "output.txt",
        b"normal report",
        ArtifactKind::Evidence,
    );
    let transient = f.add(&task, "trace.log", b"transient log", ArtifactKind::Log);
    let keep = f.add(&task, "keep.log", b"explicit keep log", ArtifactKind::Log);
    let changed = set_artifact_tier(
        &f.ctx,
        &Fixture::reference(&task, &keep),
        ArtifactTier::Keep,
    )
    .unwrap();
    assert_eq!(changed.tier(), ArtifactTier::Keep);
    // When purge runs.
    artifact_gc(&f.ctx, false).unwrap();
    // Then only the selected, unprotected tier is purged.
    assert!(f.path(&normal).exists());
    assert!(f.path(&keep).exists());
    assert!(f.current(&task, &transient).purged_at.is_some());
}

#[test]
fn newer_diff_supersedes_only_inferred_same_run_diff() {
    // Given an explicitly kept diff and an inferred diff.
    let f = Fixture::new(RetentionAction::Keep);
    let task = f.task();
    let keep = f.add(&task, "keep.diff", b"kept diff", ArtifactKind::Diff);
    set_artifact_tier(
        &f.ctx,
        &Fixture::reference(&task, &keep),
        ArtifactTier::Keep,
    )
    .unwrap();
    let first = f.add(&task, "first.diff", b"first diff", ArtifactKind::Diff);
    // When another diff arrives in the same run.
    let latest = f.add(&task, "latest.diff", b"latest diff", ArtifactKind::Diff);
    // Then only the inferred prior diff becomes transient.
    assert_eq!(f.current(&task, &keep).tier(), ArtifactTier::Keep);
    assert_eq!(f.current(&task, &first).tier(), ArtifactTier::Transient);
    assert_eq!(latest.tier(), ArtifactTier::Normal);
}

#[test]
fn materialized_reports_keep_nested_relative_assets_and_extensions() {
    // Given a report referencing a nested screenshot.
    let f = Fixture::new(RetentionAction::Compress);
    let task = f.task();
    let report = format!("<img src=\"images/screenshot.png\">{}", " ".repeat(4000));
    let html = f.add(
        &task,
        "report.html",
        report.as_bytes(),
        ArtifactKind::Report,
    );
    let image = f.add(
        &task,
        "images/screenshot.png",
        b"image bytes",
        ArtifactKind::Screenshot,
    );
    set_artifact_tier(
        &f.ctx,
        &Fixture::reference(&task, &html),
        ArtifactTier::Normal,
    )
    .unwrap();
    artifact_gc(&f.ctx, false).unwrap();
    // When an external report path is materialized.
    let path = artifact_path(&f.ctx, &f.current(&task, &html)).unwrap();
    // Then its extension and source-relative asset paths remain usable.
    assert_eq!(path.extension().unwrap(), "html");
    assert_eq!(fs::read(&path).unwrap(), report.as_bytes());
    assert_eq!(
        fs::read(path.parent().unwrap().join("images/screenshot.png")).unwrap(),
        artifact_read(&f.ctx, &image).unwrap()
    );
    assert!(matches!(
        artifact_preview(&f.ctx, &f.current(&task, &html)).unwrap(),
        Preview::External { .. }
    ));
}

#[test]
fn compressed_images_and_binary_files_get_decoded_preview_paths() {
    // Given explicitly non-screenshot raster content and an external binary.
    let f = Fixture::new(RetentionAction::Compress);
    let task = f.task();
    let image = f.add(
        &task,
        "raster.png",
        &vec![b'I'; 8000],
        ArtifactKind::Evidence,
    );
    let binary = f.add(
        &task,
        "document.pdf",
        &vec![0; 8000],
        ArtifactKind::Evidence,
    );
    set_artifact_tier(
        &f.ctx,
        &Fixture::reference(&task, &image),
        ArtifactTier::Normal,
    )
    .unwrap();
    artifact_gc(&f.ctx, false).unwrap();
    // When previews route each encoded snapshot.
    let image_preview = artifact_preview(&f.ctx, &f.current(&task, &image)).unwrap();
    let binary_preview = artifact_preview(&f.ctx, &f.current(&task, &binary)).unwrap();
    // Then previews expose original decoded bytes with usable extensions.
    let Preview::Image { path, .. } = image_preview else {
        panic!("expected raster preview")
    };
    assert_eq!(Path::new(&path).extension().unwrap(), "png");
    assert_eq!(fs::read(path).unwrap(), vec![b'I'; 8000]);
    let Preview::External { path, .. } = binary_preview else {
        panic!("expected binary preview")
    };
    assert_eq!(Path::new(&path).extension().unwrap(), "pdf");
    assert_eq!(fs::read(path).unwrap(), vec![0; 8000]);
}

#[test]
fn storage_warnings_report_soft_workspace_and_file_limits() {
    // Given configured soft limits smaller than stored content.
    let mut f = Fixture::new(RetentionAction::Keep);
    f.ctx.store.config.artifacts.max_total = "5B".into();
    f.ctx.store.config.artifacts.max_file = "3B".into();
    let task = f.task();
    f.add(&task, "large.txt", b"ten bytes!", ArtifactKind::Evidence);
    // When storage stats are requested.
    let stats = storage_stats(&f.ctx).unwrap();
    // Then both thresholds warn without blocking attachment.
    assert_eq!(stats.warnings.len(), 2);
    assert_eq!(stats.stored_bytes, 10);
    assert!(serde_json::to_value(&stats).unwrap().is_object());
}

#[test]
fn resolver_accepts_refs_paths_and_followed_numeric_pins() {
    // Given a followed pin and two snapshots of the same source.
    let f = Fixture::new(RetentionAction::Keep);
    let task = f.task();
    let old = f.add(&task, "output.txt", b"old output", ArtifactKind::Evidence);
    f.ctx
        .store
        .pin(&Fixture::reference(&task, &old), None, true)
        .unwrap();
    let latest = f.add(&task, "output.txt", b"new output", ArtifactKind::Evidence);
    // When all supported references are resolved.
    let targets = [
        resolve_artifact(&f.ctx, "1").unwrap(),
        resolve_artifact(&f.ctx, &latest.path).unwrap(),
        resolve_artifact(&f.ctx, f.path(&latest).to_str().unwrap()).unwrap(),
        resolve_artifact(&f.ctx, &format!("{task}:R0:output.txt")).unwrap(),
    ];
    // Then they resolve to the latest snapshot without changing stable refs.
    assert!(targets.iter().all(|a| a.path == latest.path));
    assert_eq!(
        resolve_artifact(&f.ctx, &Fixture::reference(&task, &old)).unwrap(),
        old
    );
}

#[test]
fn workspace_and_project_per_tier_policies_control_retention() {
    // Given workspace normal=keep/transient=purge and a project normal=compress override.
    let mut f = Fixture::new(RetentionAction::Purge);
    f.ctx.store.config.artifacts.retention.per_tier.insert(
        ArtifactTier::Normal,
        corral_core::RetentionOverride {
            action: Some(RetentionAction::Keep),
            ..Default::default()
        },
    );
    let mut project_policy = f.ctx.store.config.artifacts.retention.clone();
    project_policy.per_tier.insert(
        ArtifactTier::Normal,
        corral_core::RetentionOverride {
            action: Some(RetentionAction::Compress),
            ..Default::default()
        },
    );
    f.ctx.store.config.projects.insert(
        "project".into(),
        corral_core::Project {
            artifact_retention: Some(project_policy),
            post_land: Vec::new(),
            path: ".".into(),
            default_profile: None,
            base: None,
            review: Default::default(),
        },
    );
    let global = f.task();
    let project = f
        .ctx
        .store
        .create("Project task", Some("project"), Status::Done, "")
        .unwrap()
        .id;
    let normal = f.add(
        &global,
        "normal.diff",
        &vec![b'N'; 4000],
        ArtifactKind::Diff,
    );
    let transient = f.add(&global, "trace.log", &vec![b'T'; 4000], ArtifactKind::Log);
    let overridden = f.add(
        &project,
        "project.diff",
        &vec![b'P'; 4000],
        ArtifactKind::Diff,
    );
    // When explicit retention runs.
    artifact_gc(&f.ctx, false).unwrap();
    // Then the effective tier policy is applied after selecting the project policy.
    assert_eq!(f.current(&global, &normal), normal);
    assert!(f.current(&global, &transient).purged_at.is_some());
    assert!(f.current(&project, &overridden).compressed_bytes.is_some());
}

#[test]
#[cfg(unix)]
fn new_snapshot_reuses_already_compressed_workspace_content() {
    // Given an already compressed snapshot.
    let f = Fixture::new(RetentionAction::Compress);
    let first = f.task();
    let a = f.add(&first, "first.log", &vec![b'D'; 4000], ArtifactKind::Log);
    artifact_gc(&f.ctx, false).unwrap();
    let a = f.current(&first, &a);
    let second = f.task();
    // When another task captures identical content.
    let b = f.add(&second, "second.log", &vec![b'D'; 4000], ArtifactKind::Log);
    // Then it reuses encoded bytes without losing transparent reads.
    assert!(same_inode(&f.path(&a), &f.path(&b)));
    assert_eq!(b.compressed_bytes, a.compressed_bytes);
    assert_eq!(artifact_read(&f.ctx, &b).unwrap(), vec![b'D'; 4000]);
}

#[test]
fn kept_report_materializes_assets_without_modifying_snapshots() {
    // Given a kept HTML report and its nested image snapshot.
    let f = Fixture::new(RetentionAction::Keep);
    let task = f.task();
    let report = f.add(
        &task,
        "kept-report.html",
        b"<img src=\"images/pic.png\">",
        ArtifactKind::Report,
    );
    let image = f.add(
        &task,
        "images/pic.png",
        b"kept picture",
        ArtifactKind::Screenshot,
    );
    // When an opener asks for the kept report.
    let path = artifact_path(&f.ctx, &report).unwrap();
    // Then nested asset paths work and the original stored bytes are untouched.
    assert_eq!(
        fs::read(path.parent().unwrap().join("images/pic.png")).unwrap(),
        b"kept picture"
    );
    assert_eq!(
        fs::read(path).unwrap(),
        artifact_read(&f.ctx, &report).unwrap()
    );
    assert_eq!(fs::read(f.path(&image)).unwrap(), b"kept picture");
}

#[test]
fn open_path_and_open_artifact_launch_decoded_files_in_clean_subprocess() {
    // Given a hermetic subprocess with no inherited live gate or herdr context.
    const CHILD: &str = "CORRAL_STORAGE_OPENER_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let root = tempfile::tempdir().unwrap();
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        for (key, _) in std::env::vars_os() {
            let name = key.to_string_lossy();
            if name.starts_with("CORRAL_GATE_") || name.starts_with("HERDR_") {
                command.env_remove(key);
            }
        }
        let output = command
            .env(CHILD, "1")
            .env("CORRAL_ROOT", root.path())
            .args([
                "--exact",
                "open_path_and_open_artifact_launch_decoded_files_in_clean_subprocess",
                "--nocapture",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let root = std::path::PathBuf::from(std::env::var_os("CORRAL_ROOT").unwrap());
    let mut store = Store::init(&root).unwrap();
    store.config.artifacts.retention.larger_than = Some("1B".into());
    let output = root.join("opened.txt");
    for ext in ["txt", "html", "png"] {
        store
            .config
            .viewers
            .insert(ext.into(), format!("/bin/cp {{path}} {}", output.display()));
    }
    let ctx = Ctx::from_store(store);
    let task = ctx.store.create("Opener", None, Status::Done, "").unwrap();
    let source = root.join("text.txt");
    let bytes = vec![b'O'; 5000];
    fs::write(&source, &bytes).unwrap();
    let a = add_artifact(&ctx, &task.id, &source, Some(ArtifactKind::Evidence), None).unwrap();
    artifact_gc(&ctx, false).unwrap();
    let a = resolve_artifact(&ctx, &a.path).unwrap();
    // When the real viewer entry points launch their configured argv.
    corral_ops::open_path(&ctx, &ctx.store.dir().join(&a.path), true).unwrap();
    assert_eq!(fs::read(&output).unwrap(), bytes);
    corral_ops::open_artifact(&ctx, &a, true).unwrap();
    // Then the viewer receives decoded bytes, not a gzip stream.
    assert_eq!(fs::read(output).unwrap(), bytes);
}

#[test]
fn reattaching_purged_content_revives_the_same_reference() {
    // Given a purged snapshot and its unchanged source.
    let f = Fixture::new(RetentionAction::Purge);
    let task = f.task();
    let a = f.add(
        &task,
        "revive.log",
        b"revive these bytes",
        ArtifactKind::Log,
    );
    artifact_gc(&f.ctx, false).unwrap();
    // When the same content is attached again.
    let revived =
        add_artifact(&f.ctx, &task, &f.tmp.path().join("revive.log"), None, None).unwrap();
    // Then the stable reference is restored without a duplicate record.
    assert_eq!(revived.path, a.path);
    assert!(revived.purged_at.is_none());
    assert_eq!(
        artifact_read(&f.ctx, &revived).unwrap(),
        b"revive these bytes"
    );
    assert_eq!(f.ctx.store.get(&task).unwrap().artifacts.len(), 1);
}

#[test]
#[cfg(unix)]
fn special_files_are_rejected_before_reading_content() {
    // Given a socket, which is neither a directory nor a regular file.
    let f = Fixture::new(RetentionAction::Keep);
    let task = f.task();
    let socket = f.tmp.path().join("socket");
    let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    // When attachment is attempted.
    let result = add_artifact(&f.ctx, &task, &socket, None, None);
    // Then the admission boundary rejects it without creating metadata.
    assert!(result.is_err());
    assert!(f.ctx.store.get(&task).unwrap().artifacts.is_empty());
}

#[test]
fn materialized_old_revision_does_not_resolve_to_latest_source_bytes() {
    // Given two compressed snapshots of the same source path.
    let f = Fixture::new(RetentionAction::Compress);
    let task = f.task();
    let old = f.add(
        &task,
        "output.html",
        &vec![b'A'; 4000],
        ArtifactKind::Evidence,
    );
    f.add(
        &task,
        "output.html",
        &vec![b'B'; 4000],
        ArtifactKind::Evidence,
    );
    artifact_gc(&f.ctx, false).unwrap();
    // When the earlier revision is explicitly opened.
    let path = artifact_path(&f.ctx, &f.current(&task, &old)).unwrap();
    // Then its source-layout path contains the requested revision.
    assert_eq!(fs::read(path).unwrap(), vec![b'A'; 4000]);
}
