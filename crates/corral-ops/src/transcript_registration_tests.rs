//! The new transcript route against the existing generic full registration behavior.
use crate::{Ctx, artifacts};
use corral_core::{Artifact, ArtifactKind, Status, Store};
use serde_json::json;

fn context(root: &std::path::Path) -> (Ctx, String) {
    let store = Store::init(root).unwrap();
    let at = "2026-10-18T12:00:00Z".parse().unwrap();
    store.fixture_pin_now(at).unwrap();
    let mut task = store
        .create("Transcript", None, Status::Running, "")
        .unwrap();
    task.runs.push(
        serde_json::from_value(json!({"id":"R1","agent":"fake","worktree":"/fixture",
        "branch":"main","base":"main","base_sha":"abc","started":at}))
        .unwrap(),
    );
    store.fixture_put_task(&task).unwrap();
    (Ctx::from_store(store), task.id)
}

#[test]
fn transcript_registration_matches_generic_first_content_order_and_retention_for_new_unchanged_and_revived_rows()
 {
    corral_core::clock::pin_thread(Some("2026-10-18T12:00:00Z".parse().unwrap()));
    for historical in [false, true] {
        for reserved_title in [false, true] {
            for existing in ["absent", "unchanged", "purged"] {
                let tmp = tempfile::tempdir().unwrap();
                let source = tmp.path().join("session.jsonl");
                let bytes = b"transcript\n";
                std::fs::write(&source, bytes).unwrap();
                let (generic, id) = context(&tmp.path().join("generic"));
                let (bounded, other) = context(&tmp.path().join("bounded"));
                assert_eq!(id, other);
                for ctx in [&generic, &bounded] {
                    ctx.store
                        .update(&id, |t| {
                            if historical {
                                let mut later = t.runs[0].clone();
                                later.id = "R2".into();
                                t.runs.push(later);
                            }
                            if existing != "absent" {
                                let a = Artifact {
                                    run: "R1".into(),
                                    kind: ArtifactKind::Report,
                                    title: if reserved_title {
                                        "PM isolated test result"
                                    } else {
                                        "first title"
                                    }
                                    .into(),
                                    path: format!("artifacts/{id}/R1/first-name.jsonl"),
                                    source: "/first/source".into(),
                                    sha256: crate::verification::digest(bytes),
                                    bytes: 11,
                                    added: corral_core::clock::now(),
                                    tier_override: None,
                                    compressed_bytes: (existing == "purged").then_some(0),
                                    purged_at: (existing == "purged")
                                        .then_some(corral_core::clock::now()),
                                };
                                t.artifacts.push(a.clone());
                                let mut duplicate = a;
                                duplicate.path.push_str("-later");
                                duplicate.title = "later live title".into();
                                duplicate.purged_at = None;
                                duplicate.compressed_bytes = None;
                                t.artifacts.push(duplicate);
                            }
                            Ok(())
                        })
                        .unwrap();
                }
                let expected = artifacts::add_artifact_to_run(
                    &generic,
                    &id,
                    "R1",
                    &source,
                    Some(ArtifactKind::Log),
                    Some("Raw agent transcript"),
                )
                .unwrap();
                let actual = artifacts::add_transcript_bytes(
                    &bounded,
                    &id,
                    "R1",
                    &source,
                    bytes,
                    "Raw agent transcript",
                )
                .unwrap();
                assert_eq!(actual, expected, "{historical} {reserved_title} {existing}");
                assert_eq!(
                    serde_json::to_value(bounded.store.get(&id).unwrap()).unwrap(),
                    serde_json::to_value(generic.store.get(&id).unwrap()).unwrap(),
                    "{historical} {reserved_title} {existing}"
                );
                let events = |ctx: &Ctx| {
                    ctx.store
                        .events_of(Some(&id), &["artifact"])
                        .unwrap()
                        .into_iter()
                        .map(|e| e.detail)
                        .collect::<Vec<_>>()
                };
                assert_eq!(events(&bounded), events(&generic));
                assert_eq!(events(&bounded).len(), usize::from(existing != "unchanged"));
                assert_eq!(crate::artifact_read(&bounded, &actual).unwrap(), bytes);
                assert!(
                    !bounded.store.dir().join(&actual.path).exists(),
                    "no legacy mirror"
                );
                assert_eq!(
                    bounded.store.head(&id).unwrap().summary.current_artifacts,
                    generic.store.head(&id).unwrap().summary.current_artifacts
                );
            }
        }
    }
}
