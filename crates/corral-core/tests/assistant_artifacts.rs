use corral_core::{Artifact, ArtifactKind, PinRef, Store};

#[test]
fn session_refs_round_trip_without_task_or_run_identity() {
    for reference in [
        "A-1:report.md",
        "A-12:123456ab-report.md",
        "T-042:R1:report.md",
    ] {
        assert_eq!(PinRef::parse(reference).unwrap().to_string(), reference);
    }
    for invalid in [
        "A-1:",
        "A-0:report.md",
        "A-01:report.md",
        "A-../:report.md",
        "A-1:R1:report.md",
    ] {
        assert!(PinRef::parse(invalid).is_err(), "{invalid}");
    }
    let temp = tempfile::tempdir().unwrap();
    let store = Store::init(temp.path()).unwrap();
    store.create_assistant("pi", None, None, None).unwrap();
    let artifact = Artifact {
        run: String::new(),
        kind: ArtifactKind::Report,
        title: "Report".into(),
        path: "artifacts/assistant/A-1/123456ab-report.md".into(),
        source: "/tmp/report.md".into(),
        sha256: "123456ab".repeat(8),
        bytes: 3,
        added: corral_core::clock::now(),
        tier_override: None,
        compressed_bytes: None,
        purged_at: None,
    };
    store
        .update_assistant("A-1", |s| {
            s.artifacts.push(artifact.clone());
            Ok(())
        })
        .unwrap();
    assert_eq!(
        store.get_assistant("A-1").unwrap().artifacts,
        vec![artifact.clone()]
    );
    assert_eq!(
        PinRef::of_artifact("A-1", &artifact).to_string(),
        "A-1:123456ab-report.md"
    );
    assert!(store.list().unwrap().is_empty());
    // Dispatch/completion refresh the content index while already holding the board lock.
    let mut owners = Vec::new();
    store
        .update_assistant("A-1", |_| {
            owners = store.artifact_owners()?;
            Ok(())
        })
        .unwrap();
    assert_eq!(owners.len(), 1);
    assert!(owners[0].runs.is_empty());
}
