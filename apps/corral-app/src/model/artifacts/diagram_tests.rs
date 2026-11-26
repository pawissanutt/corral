use corral_protocol::detail::{ArtifactDiagramDetail, DetailBody, DiagramResult};
use corral_protocol::diagram::{WorkflowRecipe, validate};

use super::diagram::{self, bows, picked};
use super::{Action, State};
use crate::model::daemon::{DetailCache, Feed, detail_key};

fn recipe(edges: &[(&str, &str, &str)]) -> WorkflowRecipe {
    let mut nodes = vec![
        r#"{"id":"a","kind":"step","label":"A"}"#.to_owned(),
        r#"{"id":"b","kind":"decision","label":"B"}"#.to_owned(),
        r#"{"id":"c","kind":"end","label":"C"}"#.to_owned(),
    ];
    nodes.dedup();
    let edges = edges
        .iter()
        .map(|(id, from, to)| {
            let kind = if *from == "b" { "branch" } else { "next" };
            format!(r#"{{"id":"{id}","kind":"{kind}","from":"{from}","to":"{to}","label":"{id}"}}"#)
        })
        .collect::<Vec<_>>();
    let text = format!(
        r#"{{"schema":"corral.workflow-diagram","version":1,"subject":{{"project":"p","commit":"{}"}},"start":"a","nodes":[{}],"edges":[{}]}}"#,
        "a".repeat(40),
        nodes.join(","),
        edges.join(",")
    );
    validate(text.as_bytes()).expect("a valid recipe")
}

/// Where the viewer draws each edge's middle relative to the low-to-high line of its pair.
fn sides(recipe: &WorkflowRecipe) -> Vec<f32> {
    recipe
        .edges
        .iter()
        .zip(bows(recipe))
        .map(|(edge, bow)| if edge.from <= edge.to { bow } else { -bow })
        .collect()
}

#[test]
fn reciprocal_and_parallel_edges_never_share_a_path() {
    // a→b, b→a (Retry back), b→c twice (parallel branches), b→a again.
    let r = recipe(&[
        ("go", "a", "b"),
        ("retry", "b", "a"),
        ("pass", "b", "c"),
        ("skip", "b", "c"),
        ("again", "b", "a"),
    ]);
    let at = sides(&r);
    let pair_ab = [at[0], at[1], at[4]];
    let pair_bc = [at[2], at[3]];
    for pair in [&pair_ab[..], &pair_bc[..]] {
        for (i, x) in pair.iter().enumerate() {
            for y in &pair[i + 1..] {
                assert!((x - y).abs() > 1.0, "edges of one pair overlap: {pair:?}");
            }
        }
    }
    let lone = recipe(&[("go", "a", "b"), ("pass", "b", "c"), ("retry", "b", "a")]);
    assert_eq!(bows(&lone)[1], 0.0, "a lone edge stays straight");
}

#[test]
fn only_retries_run_back_so_the_flow_reads_from_its_start() {
    let fixture = validate(include_bytes!(
        "../../../../../docs/examples/submit-to-land.diagram.json"
    ))
    .expect("the guide's example is valid");
    let back: Vec<_> = fixture
        .edges
        .iter()
        .zip(diagram::back_edges(&fixture))
        .filter(|(_, back)| *back)
        .map(|(edge, _)| edge.id.as_str())
        .collect();
    assert_eq!(back, ["gate-fails", "review-requests-changes"]);
}

fn served(feed: &mut Feed, reference: &str, recipe: WorkflowRecipe) {
    let target = diagram::target(reference);
    let artifact = corral_types::Artifact {
        task: "T-1".into(),
        id: 1,
        sha256: "ab".repeat(32),
        bytes: 100,
        kind: corral_types::ArtifactKind::Report,
        title: "flow".into(),
        at: jiff::Timestamp::UNIX_EPOCH,
        by: corral_types::Actor::Agent,
        project: None,
        run: None,
        replaces: None,
        replaced_by: None,
    };
    feed.details.insert(
        detail_key(&target),
        DetailCache {
            transcript_seams: Default::default(),
            target,
            pages: Vec::new(),
            loading: false,
            stale: false,
            error: None,
            merged: Some(DetailBody::ArtifactDiagram(ArtifactDiagramDetail {
                artifact,
                result: DiagramResult::Ready { recipe },
            })),
        },
    );
}

#[test]
fn only_an_id_of_the_open_recipe_selects() {
    let ids = vec!["a".to_owned(), "go".to_owned()];
    assert_eq!(picked(r#"{"v":1,"pick":"go"}"#, &ids), Some("go".into()));
    assert_eq!(picked(r#"{"v":1,"pick":"elsewhere"}"#, &ids), None);
    assert_eq!(picked(r#"{"v":2,"pick":"go"}"#, &ids), None);
    assert_eq!(picked("pick:0", &ids), None);

    let mut feed = Feed::default();
    served(
        &mut feed,
        "T-1:1",
        recipe(&[("go", "a", "b"), ("pass", "b", "c"), ("retry", "b", "a")]),
    );
    let mut state = State {
        full: Some("T-1:1".into()),
        ..State::default()
    };
    let pick = |reference: &str, element: &str| Action::Pick {
        reference: reference.into(),
        element: element.into(),
    };
    state.act(pick("T-1:1", "missing"), &feed);
    state.act(pick("T-9:9", "go"), &feed);
    assert_eq!(
        state.pick, None,
        "an unknown ID or another artifact never selects"
    );
    state.act(pick("T-1:1", "go"), &feed);
    assert_eq!(state.pick, Some(("T-1:1".into(), "go".into())));
}
