use super::*;
use corral_core::{
    Status, Store,
    backend::{ManualClock, OpKind},
    sqlite::{SqliteBackend, SqliteOptions},
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

#[test]
fn slow_blob_preparation_does_not_pin_the_artifact_publication_view() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .args([
                "-c",
                "user.name=test",
                "-c",
                "user.email=test@invalid",
                "-c",
                "core.hooksPath=/dev/null",
                "-C",
            ])
            .arg(&repo)
            .args(args)
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(repo.join(".gitignore"), ".evidence/\n").unwrap();
    git(&["add", ".gitignore"]);
    git(&["commit", "-qm", "fixture"]);
    std::fs::create_dir(repo.join(".evidence")).unwrap();
    for name in ["one", "two"] {
        std::fs::write(repo.join(".evidence").join(name), name).unwrap();
    }
    let store = Store::init(dir.path()).unwrap();
    let task = store.create("scan", None, Status::Running, "").unwrap();
    store
        .update(&task.id, |task| {
            task.runs.push(serde_json::from_value(json!({
                "id":"R1", "agent":"worker", "worktree":repo,
                "branch":"main", "base":"main", "base_sha":git(&["rev-parse", "HEAD"]),
                "started":store.now()
            }))?);
            Ok(())
        })
        .unwrap();
    let clock = Arc::new(ManualClock::new(jiff::Timestamp::now()));
    let puts = Arc::new(AtomicUsize::new(0));
    let (step_clock, step_puts) = (clock.clone(), puts.clone());
    let backend = SqliteBackend::open(
        store.dir(),
        SqliteOptions {
            clock,
            barrier: Some(Arc::new(move |point| {
                if point.op == OpKind::BlobReserve && point.what == "committed" {
                    step_puts.fetch_add(1, Ordering::Relaxed);
                    step_clock.advance(Duration::from_secs(2));
                }
            })),
            ..SqliteOptions::default()
        },
    )
    .unwrap();
    let mut store = store.with_backend(Box::new(backend));
    store.config.storage.view_max_age = 1; // Explicit seam, independent of the global default.
    store.config.artifacts.globs = vec![".evidence/**".into()];
    let ctx = crate::resources::FixedResourceProbe::ctx(store);
    let added = scan_artifacts(&ctx, &task.id).unwrap();
    assert_eq!(puts.load(Ordering::Relaxed), 2);
    assert_eq!(added.len(), 2);
    assert_eq!(
        ctx.store.artifact_rows(&task.id, Some("R1")).unwrap().len(),
        2
    );
}
