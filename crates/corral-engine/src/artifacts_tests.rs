use std::fs;

use crate::{Engine, artifacts::scan_globs, as_refusal, events::EventKind, tasks::NewTask};

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
        .add_artifact(&task.id, &file, Some("report"), None)
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
        .add_artifact_bytes(&task.id, b"report", None, "same bytes")
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
                .add_artifact(&task.id, &file, None, None)
                .unwrap_err()
        )
        .is_some()
    );
    assert!(
        as_refusal(
            &engine
                .add_artifact_bytes(&task.id, b"four", None, "big")
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
