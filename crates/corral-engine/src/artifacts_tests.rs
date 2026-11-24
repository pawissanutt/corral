use std::fs;

use crate::{
    ArtifactKind, Engine, artifacts::scan_globs, as_refusal, events::EventKind, tasks::NewTask,
};

#[test]
fn stored_artifacts_with_unclassified_kinds_decode_as_legacy() {
    let root = crate::tests::tempdir();
    let engine = Engine::init(root.path(), None).unwrap();
    let task = "T-001";
    engine.store.insert_task(&crate::tests::task(task)).unwrap();
    for (id, stored_kind) in
        (1u32..).zip([None, Some(serde_json::Value::Null), Some("design".into())])
    {
        let mut document = serde_json::json!({
            "task": task, "id": id, "sha256": "unused", "bytes": 0,
            "title": "Old artifact", "at": "2026-01-01T00:00:00Z", "by": "owner"
        });
        if let Some(kind) = stored_kind {
            document["kind"] = kind;
        }
        engine
            .store
            .connection
            .execute(
                "INSERT INTO artifacts(task,ordinal,sha256,document) VALUES(?1,?2,'unused',?3)",
                rusqlite::params![task, id, document.to_string()],
            )
            .unwrap();
    }
    let artifacts = engine.list_artifacts(Some(task)).unwrap();
    assert_eq!(artifacts.len(), 3);
    for artifact in artifacts {
        assert_eq!(artifact.kind, ArtifactKind::Legacy);
    }
}

#[test]
fn tool_output_is_recorded_once_per_run_and_name() {
    let root = crate::tests::tempdir();
    let engine = Engine::init(root.path(), None).unwrap();
    engine
        .store
        .insert_task(&crate::tests::task("T-001"))
        .unwrap();
    engine
        .store
        .insert_run(&crate::tests::run("T-001", "R1"))
        .unwrap();
    engine
        .store
        .insert_run(&crate::tests::run("T-001", "R2"))
        .unwrap();
    engine
        .record_tool_output("T-001", "R1", "context", b"first")
        .unwrap();
    engine
        .record_tool_output("T-001", "R1", "context", b"second")
        .unwrap();
    engine
        .record_tool_output("T-001", "R2", "context", b"other run")
        .unwrap();
    let artifacts = engine.list_artifacts(Some("T-001")).unwrap();
    assert_eq!(artifacts.len(), 2);
    assert_eq!(
        engine.show_artifact(&artifacts[0].reference()).unwrap(),
        b"first"
    );
    let catalog = engine
        .store
        .artifacts_before(Default::default(), None, 10)
        .unwrap();
    assert!(catalog.iter().any(|a| a.run.as_deref() == Some("R1")));
    assert!(catalog.iter().any(|a| a.run.as_deref() == Some("R2")));
}

#[test]
fn artifact_add_list_show_size_and_glob_scan() {
    let root = crate::tests::tempdir();
    let mut engine = Engine::init(root.path(), None).unwrap();
    let task = engine
        .new_task(NewTask {
            title: "artifact".to_owned(),
            ..Default::default()
        })
        .unwrap();
    let file = root.path().join("report.html");
    fs::write(&file, b"report").unwrap();
    let artifact = engine
        .add_artifact(&task.id, &file, ArtifactKind::Report, None)
        .unwrap();
    assert_eq!(artifact.reference(), "T-001:1");
    assert_eq!(
        engine.show_artifact(&artifact.reference()).unwrap(),
        b"report"
    );
    let blob = engine
        .state_dir
        .join("blobs/sha256")
        .join(&artifact.sha256[..2])
        .join(&artifact.sha256[2..]);
    assert_eq!(fs::read(blob).unwrap(), b"report");
    assert_eq!(
        engine.list_artifacts(Some(&task.id)).unwrap(),
        std::slice::from_ref(&artifact)
    );
    assert_eq!(
        engine.list_artifacts(None).unwrap(),
        std::slice::from_ref(&artifact)
    );
    let second = engine
        .add_artifact_bytes(&task.id, b"report", ArtifactKind::Reference, "same bytes")
        .unwrap();
    assert_eq!(second.id, 2);
    assert_eq!(artifact.sha256, second.sha256);
    assert_eq!(
        engine
            .store
            .list_events(&task.id)
            .unwrap()
            .last()
            .unwrap()
            .kind,
        EventKind::Artifact
    );
    engine.config.artifacts.max_bytes = 3;
    assert!(
        as_refusal(
            &engine
                .add_artifact(&task.id, &file, ArtifactKind::Report, None)
                .unwrap_err()
        )
        .is_some()
    );
    assert!(
        as_refusal(
            &engine
                .add_artifact_bytes(&task.id, b"four", ArtifactKind::Report, "big")
                .unwrap_err()
        )
        .is_some()
    );
    fs::create_dir_all(root.path().join("docs/a/records")).unwrap();
    let record = root.path().join("docs/a/records/note.txt");
    fs::write(&record, b"note").unwrap();
    let scan = scan_globs(
        root.path(),
        &[
            "**/*report*.html".to_owned(),
            "docs/**/records/*".to_owned(),
            "./report.html".to_owned(),
        ],
        [
            "report.html".to_owned(),
            "docs/a/records/note.txt".to_owned(),
        ],
    )
    .unwrap();
    assert_eq!(scan, vec![record, file]);
    assert!(scan_globs(root.path(), &["../*".to_owned()], []).is_err());
    assert!(engine.show_artifact("T-001:999").is_err());
}
