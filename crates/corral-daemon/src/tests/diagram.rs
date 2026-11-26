use std::{io::Write, time::Instant};

use corral_engine::{ArtifactKind, events::NewEvent};
use corral_protocol::{
    app::{ARTIFACT_DIAGRAM_V1, DETAIL_GET},
    detail::*,
};
use corral_types::{Artifact, ReferenceBasis, ReferenceKind};

use super::*;

const RECIPE: &[u8] = include_bytes!("../../../../docs/examples/submit-to-land.diagram.json");
const ANCHOR: &str = "src/review.rs";
const DEPENDENT: &str = "src/submit.rs";

#[test]
fn socket_diagram_gate_keeps_the_original_candidate_and_source_with_or_without_provider() {
    let fixture = Fixture::with_repository();
    let config_path = fixture.root().join(".corral-next/corral.toml");
    let with_config = |extra: &str| {
        let mut config = fs::OpenOptions::new()
            .append(true)
            .open(&config_path)
            .unwrap();
        writeln!(config, "{extra}").unwrap();
        fixture.shared().refresh_config().unwrap();
    };
    with_config("\n[artifacts]\nglobs = ['.evidence/workflow.json']");
    crate::system::start(fixture.shared()).unwrap();
    let socket = fixture.unix_server();
    let mut client = Client::connect_with(&socket, vec![ARTIFACT_DIAGRAM_V1.into()]);
    let id = client.new_task("publish a workflow", vec![]);
    assert!(matches!(
        client.op(dispatch(&id), false),
        OpReply::Done { .. }
    ));
    let engine = fixture.shared().engine().unwrap();
    let run = engine.store.list_runs(&id).unwrap().pop().unwrap();
    fs::create_dir_all(run.worktree.join("src")).unwrap();
    fs::write(
        run.worktree.join("Cargo.toml"),
        "[package]\nname='demo'\nversion='0.1.0'\n",
    )
    .unwrap();
    fs::write(run.worktree.join("src/lib.rs"), "mod review; mod submit;\n").unwrap();
    fs::write(
        run.worktree.join(ANCHOR),
        "pub fn gate() {}\nfn other() {}\n",
    )
    .unwrap();
    fs::write(
        run.worktree.join(DEPENDENT),
        "use crate::review::gate;\nfn submit() { gate(); }\n",
    )
    .unwrap();
    fs::write(
        run.worktree.join(".gitignore"),
        "/.worktrees/\n/.evidence/\n",
    )
    .unwrap();
    git(&run.worktree, &["add", "."]);
    git(&run.worktree, &["commit", "-m", "add reviewed source"]);
    let head = corral_facts::resolve_revision(&run.worktree, "HEAD")
        .unwrap()
        .commit()
        .to_owned();
    let mut recipe: Value = serde_json::from_slice(RECIPE).unwrap();
    recipe["subject"] = json!({"project":"demo", "commit":head});
    recipe["nodes"][1]["bindings"] = json!({
        "source":{"path":ANCHOR,"span":{"start":1,"end":1}},
        "entity":{"kind":"file","path":ANCHOR}
    });
    recipe["nodes"][2]["bindings"] =
        json!({"entity":{"kind":"definition","path":ANCHOR,"name":"gate"}});
    fs::create_dir(run.worktree.join(".evidence")).unwrap();
    fs::write(
        run.worktree.join(".evidence/workflow.json"),
        serde_json::to_vec(&recipe).unwrap(),
    )
    .unwrap();
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
    let artifact = engine
        .store
        .list_artifacts(Some(&id))
        .unwrap()
        .into_iter()
        .find(|artifact| artifact.title == "workflow.json")
        .unwrap();
    let graph = DetailGet {
        target: DetailRef::ArtifactDiagram {
            reference: artifact.reference(),
        },
        etag: None,
        page: None,
    };
    let error = Client::connect(&socket)
        .call(DETAIL_GET, &graph)
        .unwrap_err();
    assert_eq!(error.code, rpc::ErrorCode::InvalidParams);
    let graph: DetailReply = client.ok(DETAIL_GET, &graph);
    assert!(graph.next.is_none());
    assert!(matches!(
        graph.body,
        Some(DetailBody::ArtifactDiagram(ArtifactDiagramDetail {
            result: DiagramResult::Ready { .. },
            ..
        }))
    ));

    // Move the project checkout independently. A read at its current head would
    // return different source and no longer find this dependent.
    let repo = fixture.root().join("repo");
    fs::create_dir_all(repo.join("src")).unwrap();
    fs::write(repo.join(ANCHOR), "fn from_head() {}\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "move the project head"]);
    let later = corral_facts::resolve_revision(&repo, "HEAD").unwrap();
    // A worker can be retained across rounds. Later done events (even one
    // reusing the same head with a new base) must not reassign this attachment.
    let newer_attachment_seq = engine.store.transaction(|store| {
        let append = |kind, detail| store.append_event(&NewEvent {
            task: Some(id.clone()), run: Some(run.id.clone()), kind, actor: Actor::Agent, detail,
        });
        let watermark = store.latest_seq()?;
        let attachment = append(EventKind::Artifact, json!({"reference":format!("{id}:99")}))?;
        append(EventKind::Done, json!({"sha":head,"base_sha":later.commit(),"project":"demo","artifact_scan_after_seq":watermark}))?;
        append(EventKind::Done, json!({"sha":later.commit(),"base_sha":later.commit(),"project":"demo"}))?;
        Ok(attachment.seq.try_into()?)
    }).unwrap();
    let newer = engine
        .store
        .submitted_revision_at(&id, &run.id, &head, newer_attachment_seq)
        .unwrap()
        .unwrap();
    assert_eq!(newer.base_sha.as_deref(), Some(later.commit()));
    let gate = selection(&artifact, "gate");
    let mut read = |query: &DetailGet, until: fn(&EntityResult) -> bool| {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let response: DetailReply = client.ok(DETAIL_GET, query);
            assert!(response.next.is_none());
            let Some(DetailBody::ArtifactDiagramElement(element)) = response.body else {
                panic!("missing element detail");
            };
            if until(&element.entity) {
                break (response.etag, element);
            }
            assert!(Instant::now() < deadline, "{element:?}");
            fixture.shared().wait(Duration::from_millis(25));
        }
    };
    let (syntax_etag, gate_off) = read(&gate, |entity| !matches!(entity, EntityResult::Pending));
    assert!(
        matches!(&gate_off.source, SourceResult::Resolved { excerpt, truncated: false }
        if excerpt.commit == head && excerpt.lines == ["pub fn gate() {}"])
    );
    assert_eq!(
        gate_off.candidate,
        Some(DiagramCandidate {
            task: id.clone(),
            run: run.id,
            head: head.clone(),
            base: Some(run.base_sha)
        })
    );
    assert!(
        matches!(&gate_off.entity, EntityResult::Resolved { evidence, related, .. }
        if evidence.provider.is_none() && !evidence.provider_pending && related == &[DiagramRelated::FileReference {
            path: DEPENDENT.into(), basis: ReferenceBasis::Syntax, reference_kind: ReferenceKind::Use, count: 1,
        }])
    );
    let (_, edge) = read(&selection(&artifact, "gate-passes"), |_| true);
    assert!(matches!(edge.element, SelectedElement::Edge { .. }));
    assert_eq!(edge.source, SourceResult::NotProvided);
    assert_eq!(edge.entity, EntityResult::NotProvided);

    // The provider must be queried at the exact old SHA. Its duplicate names
    // are ambiguous and its file-level call record remains a file reference.
    fs::write(fixture.root().join("facts.sh"), format!(r#"set -eu
case "$(cat)" in
  *'"rev":"{head}"'*) ;;
  *) exit 1 ;;
esac
cat <<'FACTS'
{{"type":"definition","path":"{ANCHOR}","name":"gate","line":1,"end_line":1,"kind":"fn"}}
{{"type":"definition","path":"{ANCHOR}","name":"gate","line":2,"end_line":2,"kind":"fn"}}
{{"type":"reference","from":"{DEPENDENT}","to":"{ANCHOR}","kind":"call","count":1}}
{{"type":"done","provided":["definitions","references"],"skipped":{{"unsupported":1}},"omitted":{{}}}}
FACTS
"#)).unwrap();
    with_config("\n[extensions.fixture]\nfacts='sh ../facts.sh'");
    let (provider_etag, gate_on) = read(
        &gate,
        |entity| matches!(entity, EntityResult::Resolved { evidence, .. } if evidence.provider.is_some()),
    );
    assert_ne!(provider_etag, syntax_etag);
    assert_eq!(gate_on.source, gate_off.source);
    assert!(
        matches!(gate_on.entity, EntityResult::Resolved { evidence, related, .. }
        if evidence.provider.as_ref().unwrap().coverage.skipped["unsupported"] == 1 && related == [DiagramRelated::FileReference {
            path: DEPENDENT.into(), basis: ReferenceBasis::Resolved, reference_kind: ReferenceKind::Call, count: 1,
        }])
    );
    let (_, review) = read(&selection(&artifact, "review"), |entity| {
        !matches!(entity, EntityResult::Pending)
    });
    assert!(matches!(review.entity, EntityResult::Ambiguous { .. }));
    let cached: DetailReply = client.ok(
        DETAIL_GET,
        DetailGet {
            etag: Some(provider_etag),
            ..gate
        },
    );
    assert!(cached.body.is_none());

    // Remove only the submitted commit from this disposable repository. The
    // scanned artifact and its original Done event remain in the catalog.
    let (prefix, suffix) = head.split_at(2);
    fs::remove_file(repo.join(".git/objects").join(prefix).join(suffix)).unwrap();
    let missing: DetailReply = client.ok(DETAIL_GET, selection(&artifact, "gate"));
    let Some(DetailBody::ArtifactDiagramElement(missing)) = missing.body else {
        panic!("missing element detail");
    };
    assert_eq!(missing.candidate, gate_off.candidate);
    assert!(matches!(missing.source, SourceResult::Missing { .. }));
    assert!(matches!(missing.entity, EntityResult::Missing { .. }));
}

#[test]
fn socket_diagram_checks_scope_and_digest_before_serving_evidence() {
    let fixture = Fixture::with_repository();
    let socket = fixture.unix_server();
    let mut client = Client::connect_with(&socket, vec![ARTIFACT_DIAGRAM_V1.into()]);
    let id = client.new_task("diagram scope", vec![]);
    let engine = fixture.shared().engine().unwrap();
    let attach = |recipe: &Value| {
        engine
            .add_artifact_bytes(
                &id,
                &serde_json::to_vec(recipe).unwrap(),
                ArtifactKind::Report,
                "Diagram",
            )
            .unwrap()
    };
    let mut recipe: Value = serde_json::from_slice(RECIPE).unwrap();
    let out_of_scope = attach(&recipe);
    recipe["subject"] = json!({"project":"demo", "commit":"1".repeat(40)});
    let missing = attach(&recipe);
    let element = |reply: DetailReply| match reply.body {
        Some(DetailBody::ArtifactDiagramElement(element)) => element,
        _ => panic!("expected element detail"),
    };
    let forbidden = element(client.ok(DETAIL_GET, selection(&out_of_scope, "gate")));
    assert!(matches!(forbidden.source, SourceResult::Unavailable { .. }));
    assert!(matches!(forbidden.entity, EntityResult::Unavailable { .. }));
    let gone = element(client.ok(DETAIL_GET, selection(&missing, "gate")));
    assert!(matches!(gone.source, SourceResult::Missing { .. }));
    assert!(matches!(gone.entity, EntityResult::Missing { .. }));
    let error = client
        .call(DETAIL_GET, selection(&missing, "unknown"))
        .unwrap_err();
    assert_eq!(error.code, rpc::ErrorCode::NotFound);
    let stale = Artifact {
        sha256: out_of_scope.sha256,
        ..missing
    };
    let error = client
        .call(DETAIL_GET, selection(&stale, "gate"))
        .unwrap_err();
    assert_eq!(error.code, rpc::ErrorCode::NotFound);
}

fn selection(artifact: &Artifact, element: &str) -> DetailGet {
    DetailGet {
        target: DetailRef::ArtifactDiagramElement {
            reference: artifact.reference(),
            sha256: artifact.sha256.clone(),
            element: element.into(),
        },
        etag: None,
        page: None,
    }
}
