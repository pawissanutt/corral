//! Real session owners need neither task history nor legacy snapshot files.
use corral_core::blob_ref::BlobHash;
use corral_core::query::{ContentContext, ContentOwner, Query, SearchContext};
use corral_core::{Artifact, ArtifactKind, Follow, Pin, PinTarget, Store};
use serde_json::json;

fn artifact(store: &Store, name: &str, bytes: &[u8]) -> Artifact {
    let hash = BlobHash::of(bytes);
    store
        .put_evidence("catalog-proof", name, bytes, |blob| json!({"blob":blob}))
        .unwrap();
    Artifact {
        run: String::new(),
        kind: ArtifactKind::Report,
        title: "Report".into(),
        path: format!("artifacts/assistant/A-1/{}-{name}.md", &hash.as_str()[..8]),
        source: "/input/report.md".into(),
        sha256: hash.to_string(),
        bytes: bytes.len() as u64,
        added: corral_core::clock::now(),
        tier_override: None,
        compressed_bytes: None,
        purged_at: None,
    }
}

#[test]
fn typed_session_pins_follow_actual_registration_order_and_blob_availability() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::init(temp.path()).unwrap();
    store.create_assistant("pi", None, None, None).unwrap();
    let first = artifact(&store, "first", b"first");
    let mut second = artifact(&store, "second", b"second");
    second.added = first.added - jiff::SignedDuration::from_secs(1);
    store
        .update_assistant("A-1", |session| {
            session.artifacts = vec![first.clone(), second.clone()];
            Ok(())
        })
        .unwrap();
    assert!(!store.dir().join(&first.path).exists());
    assert!(!store.dir().join(&second.path).exists());
    let owners = store.artifact_owners().unwrap();
    assert_eq!(owners.len(), 1);
    assert!(owners[0].task().is_none());
    assert_eq!(
        owners[0].session().unwrap().artifacts,
        vec![first.clone(), second.clone()]
    );
    assert!(store.list().unwrap().is_empty());
    let target = corral_core::PinRef::of_artifact("A-1", &first).to_string();
    let pin = Pin {
        target: target.clone(),
        label: None,
        follow: Some(Follow::Latest),
    };
    for resolved in [
        store.resolve_pin_owners(&pin, &owners).unwrap(),
        store.resolve_pin_head(&pin).unwrap(),
    ] {
        assert!(
            matches!(resolved, PinTarget::Artifact {artifact, followed:true, ..} if artifact == second)
        );
    }
    store.pin(&target, None, true).unwrap();
    let pinned = store.resolved_pins().unwrap();
    assert!(
        matches!(&pinned[0].1, PinTarget::Artifact {artifact, followed:true, ..} if artifact == &second)
    );
    let hash = BlobHash::parse(&second.sha256).unwrap();
    std::fs::remove_file(store.blob_store().unwrap().path_of(&hash)).unwrap();
    assert!(matches!(
        store.resolve_pin_head(&pin).unwrap(),
        PinTarget::Broken(_)
    ));
}

#[test]
fn content_metadata_matches_actual_session_or_workspace_without_constructing_tasks() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::init(temp.path()).unwrap();
    store.create_assistant("pi", None, None, None).unwrap();
    let report = artifact(&store, "report", b"needle");
    store
        .update_assistant("A-1", |session| {
            session.artifacts.push(report);
            Ok(())
        })
        .unwrap();
    let owners = store.artifact_owners().unwrap();
    let now = corral_core::clock::now();
    let content = || ContentContext {
        title: "Report",
        text: "needle",
        tests: "",
        errors: "",
        verdicts: "",
        kind: "report",
        updated: now,
    };
    let query = Query::parse("status:backlog id:a-* has:artifact kind:report needle", now).unwrap();
    assert!(query.matches_owned_content(
        ContentOwner::from(&owners[0]),
        SearchContext::default(),
        content()
    ));
    assert!(!query.matches_owned_content(
        ContentOwner::Workspace,
        SearchContext::default(),
        content()
    ));
    let profile = Query::parse("profile:pi", now).unwrap();
    assert!(
        !profile.matches_owned_content(
            ContentOwner::from(&owners[0]),
            SearchContext::default(),
            content()
        ),
        "a session does not claim a task run profile"
    );
    let text = Query::parse("needle", now).unwrap();
    assert!(text.matches_owned_content(
        ContentOwner::Workspace,
        SearchContext::default(),
        content()
    ));
}
