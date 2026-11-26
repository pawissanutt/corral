use std::time::Instant;

use corral_engine::ArtifactKind;
use corral_protocol::{
    app::{ARTIFACT_DIAGRAM_COMPARE_V1, ARTIFACT_DIAGRAM_V1, DETAIL_GET},
    detail::*,
};
use corral_types::{ReferenceBasis, ReferenceKind};

use super::*;

const RECIPE: &[u8] = include_bytes!("../../../../docs/examples/submit-to-land.diagram.json");
const ANCHOR: &str = "src/review.rs";
const DEPENDENT: &str = "src/submit.rs";

/// Submit-to-land: a recipe pinned at the worker's first commit, compared with the task's later
/// submission, reports the changed, moved and removed bindings without touching the artifact.
#[test]
fn socket_diagram_compare_reports_changed_moved_and_removed_bindings_at_the_candidate() {
    let fixture = Fixture::with_repository();
    crate::system::start(fixture.shared()).unwrap();
    let socket = fixture.unix_server();
    let mut client = Client::connect_with(
        &socket,
        vec![
            ARTIFACT_DIAGRAM_V1.into(),
            ARTIFACT_DIAGRAM_COMPARE_V1.into(),
        ],
    );
    let id = client.new_task("compare a workflow", vec![]);
    assert!(matches!(
        client.op(dispatch(&id), false),
        OpReply::Done { .. }
    ));
    let engine = fixture.shared().engine().unwrap();
    let run = engine.store.list_runs(&id).unwrap().pop().unwrap();
    let tree = &run.worktree;
    fs::create_dir_all(tree.join("src")).unwrap();
    fs::write(
        tree.join("Cargo.toml"),
        "[package]\nname='demo'\nversion='0.1.0'\n",
    )
    .unwrap();
    fs::write(tree.join("src/lib.rs"), "mod review; mod submit;\n").unwrap();
    fs::write(tree.join(ANCHOR), "pub fn gate() {}\nfn other() {}\n").unwrap();
    fs::write(
        tree.join(DEPENDENT),
        "use crate::review::gate;\nfn submit() { gate(); }\n",
    )
    .unwrap();
    git(tree, &["add", "."]);
    git(tree, &["commit", "-m", "first draft"]);
    let pinned = corral_facts::resolve_revision(tree, "HEAD")
        .unwrap()
        .commit()
        .to_owned();
    let mut recipe: Value = serde_json::from_slice(RECIPE).unwrap();
    recipe["subject"] = json!({"project":"demo", "commit":pinned});
    recipe["nodes"][1]["bindings"] = json!({
        "source":{"path":ANCHOR,"span":{"start":1,"end":1}},
        "entity":{"kind":"file","path":ANCHOR}
    });
    recipe["nodes"][2]["bindings"] = json!({"source":{"path":ANCHOR,"span":{"start":2,"end":2}}});
    recipe["nodes"][3]["bindings"] = json!({"source":{"path":DEPENDENT}});
    let bytes = serde_json::to_vec(&recipe).unwrap();
    let artifact = engine
        .add_artifact_bytes(&id, &bytes, ArtifactKind::Report, "Submit to land")
        .unwrap();

    // The submitted candidate changes the gate's line, moves the review's and removes the
    // landing file, so the gate file loses its only inbound reference.
    fs::write(
        tree.join(ANCHOR),
        "// moved down\npub fn gate(ready: bool) {}\nfn other() {}\n",
    )
    .unwrap();
    fs::remove_file(tree.join(DEPENDENT)).unwrap();
    fs::write(tree.join("src/lib.rs"), "mod review;\n").unwrap();
    git(tree, &["add", "-A"]);
    git(tree, &["commit", "-m", "submitted change"]);
    let candidate = corral_facts::resolve_revision(tree, "HEAD")
        .unwrap()
        .commit()
        .to_owned();
    assert!(matches!(
        client.op(
            Op::Done {
                id: id.clone(),
                summary: "submitted".into()
            },
            false
        ),
        OpReply::Done { .. }
    ));

    let query = |target: CompareTarget, element: Option<&str>| DetailGet {
        target: DetailRef::ArtifactDiagramCompare {
            reference: artifact.reference(),
            sha256: artifact.sha256.clone(),
            target,
            element: element.map(str::to_owned),
        },
        etag: None,
        page: None,
    };
    let mut read = |query: &DetailGet| {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let reply: DetailReply = client.ok(DETAIL_GET, query);
            let Some(DetailBody::ArtifactDiagramCompare(detail)) = reply.body else {
                panic!("missing compare detail");
            };
            let pending = matches!(detail.target, CompareResolution::Pending)
                || detail
                    .selected
                    .as_ref()
                    .is_some_and(|s| s.entity_change == EntityChange::Pending);
            if !pending {
                break detail;
            }
            assert!(Instant::now() < deadline, "{detail:?}");
            fixture.shared().wait(Duration::from_millis(25));
        }
    };

    let gate = read(&query(CompareTarget::Candidate, Some("gate")));
    assert_eq!(gate.subject.commit, pinned);
    let CompareResolution::Resolved {
        origin: CompareOrigin::Candidate,
        commit,
        candidate: Some(submitted),
        current_candidate: true,
    } = &gate.target
    else {
        panic!("unresolved candidate: {:?}", gate.target);
    };
    assert_eq!((commit, &submitted.head), (&candidate, &candidate));
    let change = |id: &str| {
        gate.elements
            .iter()
            .find(|element| element.id == id)
            .map(|element| element.change.clone())
    };
    assert_eq!(
        change("gate"),
        Some(SourceChange::Changed {
            path: ANCHOR.into()
        })
    );
    assert_eq!(
        change("review"),
        Some(SourceChange::Moved {
            path: ANCHOR.into(),
            first: Some(3)
        })
    );
    assert_eq!(change("land"), Some(SourceChange::Removed));
    assert_eq!(change("submit"), None);

    let selected = gate.selected.unwrap();
    assert!(
        matches!(&selected.base_source, SourceResult::Resolved { excerpt, .. }
        if excerpt.commit == pinned && excerpt.lines == ["pub fn gate() {}"])
    );
    assert!(
        matches!(&selected.target_source, SourceResult::Resolved { excerpt, .. } if excerpt.commit == candidate)
    );
    assert_eq!(
        selected.entity_change,
        EntityChange::Changed {
            location: None,
            added: Vec::new(),
            removed: vec![DiagramRelated::FileReference {
                path: DEPENDENT.into(),
                basis: ReferenceBasis::Syntax,
                reference_kind: ReferenceKind::Use,
                count: 1,
            }],
        }
    );
    assert_eq!(
        selected.links,
        [
            CompareLink::Diff {
                task: id.clone(),
                path: ANCHOR.into()
            },
            CompareLink::Impact { task: id.clone() }
        ]
    );

    // The removed file stays inspectable at the pinned commit; the pinned target re-reads the
    // same candidate commit.
    let land = read(&query(
        CompareTarget::Pinned {
            origin: CompareOrigin::Candidate,
            commit: candidate.clone(),
        },
        Some("land"),
    ));
    assert_eq!(land.target, gate.target);
    let land = land.selected.unwrap();
    assert_eq!(land.source_change, SourceChange::Removed);
    assert!(
        matches!(&land.base_source, SourceResult::Resolved { excerpt, .. }
        if excerpt.commit == pinned && excerpt.path == DEPENDENT)
    );
    assert!(matches!(land.target_source, SourceResult::Missing { .. }));

    // The artifact keeps its bytes and pinned subject.
    let stored = engine
        .store
        .list_artifacts(Some(&id))
        .unwrap()
        .into_iter()
        .find(|row| row.reference() == artifact.reference())
        .unwrap();
    assert_eq!(stored.sha256, artifact.sha256);
    let graph: DetailReply = client.ok(
        DETAIL_GET,
        DetailGet {
            target: DetailRef::ArtifactDiagram {
                reference: artifact.reference(),
            },
            etag: None,
            page: None,
        },
    );
    assert!(matches!(
        graph.body,
        Some(DetailBody::ArtifactDiagram(ArtifactDiagramDetail {
            result: DiagramResult::Ready { recipe },
            ..
        })) if recipe.subject.commit == pinned
    ));

    // A client that did not negotiate compare is refused.
    let error = Client::connect_with(&socket, vec![ARTIFACT_DIAGRAM_V1.into()])
        .call(DETAIL_GET, query(CompareTarget::Candidate, None))
        .unwrap_err();
    assert_eq!(error.code, rpc::ErrorCode::InvalidParams);
}
