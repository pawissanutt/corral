use super::*;
use corral_core::{ArtifactKind, EvidenceRecord, Run, Status, Store};

fn run(id: &str, worktree: &std::path::Path) -> Run {
    serde_json::from_value(serde_json::json!({
        "id": id, "agent": "generic", "worktree": worktree, "branch": "candidate", "base": "main",
        "base_sha": "base", "started": "2026-01-01T00:00:00Z"
    }))
    .unwrap()
}

fn evidence(criterion: &str, path: &str, hash: &str) -> EvidenceRecord {
    let head = "a".repeat(40);
    serde_json::from_value(serde_json::json!({
        "source": "worker_assertion",
        "observation": {"version": 1, "key": format!("{criterion}-obs"), "criterion": criterion, "run": "R1",
            "attempt": "R1", "status": "pass", "action": "a", "environment": "e", "expected": "x",
            "observed": "y", "artifacts": [path], "captured": "2026-01-02T00:00:00Z", "producer": "w", "live": false},
        "identity": {"version": 1, "task": "T-001", "run": "R1", "attempt": "R1", "head": head, "tree": head,
            "integration_base": head, "dispatch_base": head, "spec_digest": "s", "policy_digest": "p", "inputs_digest": "i"},
        "manifest_digest": "m",
        "artifact_hashes": [hash],
    }))
    .unwrap()
}

#[test]
fn bounded_catalog_reports_available_provenance_and_unknowns() {
    let dir = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    let ctx = crate::resources::FixedResourceProbe::ctx(Store::init(dir.path()).unwrap());
    let task = ctx
        .store
        .create("gallery", None, Status::Running, "")
        .unwrap();
    ctx.store
        .update(&task.id, |t| {
            t.runs.push(run("R1", files.path()));
            t.runs.push(run("R2", files.path()));
            Ok(())
        })
        .unwrap();
    let write = |name: &str, bytes: &[u8]| {
        let path = files.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    };
    let add = |run: &str, name: &str, bytes: &[u8], kind| {
        crate::artifacts::add_artifact_to_run(
            &ctx,
            &task.id,
            run,
            &write(name, bytes),
            Some(kind),
            None,
        )
        .unwrap()
    };
    let image = add(
        "R1",
        "shot.png",
        b"\x89PNG fixture",
        ArtifactKind::Screenshot,
    );
    let report = add("R1", "report.md", b"# report", ArtifactKind::Report);
    let absent = add("R1", "absent.log", b"gone", ArtifactKind::Log);
    let other = add("R2", "other.md", b"# other run", ArtifactKind::Report);
    let gone = corral_core::blob_ref::BlobHash::parse(&absent.sha256).unwrap();
    let legacy = ctx.store.dir().join(&absent.path);
    if legacy.exists() {
        std::fs::remove_file(&legacy).unwrap();
    }
    std::fs::remove_file(ctx.store.blob_store().unwrap().path_of(&gone)).unwrap();
    ctx.store
        .update(&task.id, |t| {
            let r1 = t.runs.iter_mut().find(|r| r.id == "R1").unwrap();
            r1.completion
                .evidence
                .push(evidence("C1", &image.path, &image.sha256));
            r1.completion
                .evidence
                .push(evidence("C2", &report.path, "stale-hash"));
            Ok(())
        })
        .unwrap();

    let hot = corral_core::work::HotScope::enter();
    let before = corral_core::work::snapshot();
    let gallery = inspect(&ctx, &task.id, "R1").unwrap();
    let work = corral_core::work::snapshot().since(&before);
    drop(hot);
    assert_eq!(
        work.total(corral_core::work::Counter::FullTaskHydrations),
        0
    );

    assert!(!gallery.more);
    assert_eq!(
        gallery.head, None,
        "a non-Git worktree has no HEAD, never a fabricated one"
    );
    let refs: Vec<_> = gallery.artifacts.iter().map(|a| a.path.as_str()).collect();
    assert_eq!(
        refs,
        [&other.path, &absent.path, &report.path, &image.path],
        "newest first"
    );
    let find = |path: &str| gallery.artifacts.iter().find(|a| a.path == path).unwrap();
    let shot = find(&image.path);
    assert!(shot.available);
    assert_eq!(
        shot.raw, None,
        "blob-backed bytes are encoded, never offered as raw"
    );
    assert_eq!(
        shot.reference,
        PinRef::of_artifact(&task.id, &image).to_string()
    );
    assert_eq!(shot.evidence.len(), 1);
    assert_eq!(shot.evidence[0].criterion, "C1");
    assert_eq!(shot.evidence[0].head, "a".repeat(40));
    assert_eq!(shot.evidence[0].hash_match, Some(true));
    assert_eq!(find(&report.path).evidence[0].hash_match, Some(false));
    assert!(!find(&absent.path).available);
    assert!(find(&absent.path).evidence.is_empty());
    assert!(
        find(&other.path).evidence.is_empty(),
        "other runs' evidence is not read"
    );
    assert!(inspect(&ctx, &task.id, "R9").is_err());
}

#[test]
fn catalog_reads_one_bounded_page() {
    let dir = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    let ctx = crate::resources::FixedResourceProbe::ctx(Store::init(dir.path()).unwrap());
    let task = ctx.store.create("many", None, Status::Running, "").unwrap();
    ctx.store
        .update(&task.id, |t| {
            t.runs.push(run("R1", files.path()));
            Ok(())
        })
        .unwrap();
    for i in 0..=MAX_ARTIFACTS {
        let path = files.path().join(format!("a{i}.txt"));
        std::fs::write(&path, format!("artifact {i}")).unwrap();
        crate::artifacts::add_artifact_to_run(&ctx, &task.id, "R1", &path, None, None).unwrap();
    }
    let gallery = inspect(&ctx, &task.id, "R1").unwrap();
    assert!(gallery.more);
    assert_eq!(gallery.artifacts.len(), MAX_ARTIFACTS as usize);
    assert!(
        gallery.artifacts[0]
            .path
            .ends_with(&format!("a{MAX_ARTIFACTS}.txt"))
    );
}

/// Builds a disposable real store for `scripts/qa-pi-artifacts.py --store real` through the
/// production registration APIs; never runs in the suite.
#[test]
#[ignore = "QA fixture builder; run explicitly with CORRAL_GALLERY_QA_ROOT"]
fn qa_real_store() {
    let Ok(root) = std::env::var("CORRAL_GALLERY_QA_ROOT") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let worktree = std::path::PathBuf::from(std::env::var("CORRAL_GALLERY_QA_WORKTREE").unwrap());
    let head = std::env::var("CORRAL_GALLERY_QA_HEAD").unwrap();
    let inputs = root.join("inputs");
    let ctx = crate::resources::FixedResourceProbe::ctx(Store::init(&root).unwrap());
    let task = ctx
        .store
        .create("real gallery QA", None, Status::Running, "")
        .unwrap();
    ctx.store
        .update(&task.id, |t| {
            t.runs.push(run("R1", &worktree));
            t.runs.push(run("R2", &worktree));
            Ok(())
        })
        .unwrap();
    let add = |run: &str, name: &str, kind| {
        crate::artifacts::add_artifact_to_run(
            &ctx,
            &task.id,
            run,
            &inputs.join(name),
            Some(kind),
            None,
        )
        .unwrap()
    };
    let image = add("R1", "résumé.png", ArtifactKind::Screenshot);
    let report = add("R1", "my report.md", ArtifactKind::Report);
    add("R1", "page.html", ArtifactKind::Report);
    let gone = add("R1", "gone.log", ArtifactKind::Log);
    add("R2", "other.md", ArtifactKind::Report);
    let blob = corral_core::blob_ref::BlobHash::parse(&gone.sha256).unwrap();
    std::fs::remove_file(ctx.store.blob_store().unwrap().path_of(&blob)).unwrap();
    ctx.store
        .update(&task.id, |t| {
            let r1 = t.runs.iter_mut().find(|r| r.id == "R1").unwrap();
            let mut current = evidence("C1", &image.path, &image.sha256);
            current.identity.head = head.clone();
            r1.completion.evidence.push(current);
            r1.completion
                .evidence
                .push(evidence("C2", &report.path, &report.sha256));
            Ok(())
        })
        .unwrap();
    println!(
        "CORRAL_GALLERY_QA {}",
        serde_json::json!({"task": task.id, "store": ctx.store.dir(), "image": image, "report": report})
    );
}
