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
fn self_loops_of_one_node_are_placed_apart_and_never_reversed() {
    let r = recipe(&[
        ("go", "a", "b"),
        ("wait", "b", "b"),
        ("pass", "b", "c"),
        ("hold", "b", "b"),
    ]);
    assert_eq!(diagram::loops(&r), [None, Some(0), None, Some(1)]);
    assert!(
        !diagram::back_edges(&r)[1] && !diagram::back_edges(&r)[3],
        "a self link has no ends to swap"
    );
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

#[test]
fn a_failed_read_after_pending_says_so_and_stops_asking() {
    use corral_protocol::detail::{
        DiagramElementDetail, EntityResult, SelectedElement, SourceResult,
    };
    let mut feed = Feed::default();
    let r = recipe(&[("go", "a", "b"), ("pass", "b", "c"), ("retry", "b", "a")]);
    served(&mut feed, "T-1:1", r.clone());
    let target = diagram::element_target("T-1:1", &"ab".repeat(32), "go");
    feed.details.insert(
        detail_key(&target),
        DetailCache {
            transcript_seams: Default::default(),
            target,
            pages: Vec::new(),
            loading: false,
            stale: false,
            // The cache keeps the pending body from before the failed read.
            error: Some(crate::model::daemon::ClientError {
                code: "not_found".into(),
                message: "artifact T-1:1 at that digest".into(),
            }),
            merged: Some(DetailBody::ArtifactDiagramElement(DiagramElementDetail {
                reference: "T-1:1".into(),
                sha256: "ab".repeat(32),
                subject: r.subject.clone(),
                candidate: None,
                element: SelectedElement::Edge {
                    edge: r.edges[0].clone(),
                },
                source: SourceResult::NotProvided,
                entity: EntityResult::Pending,
            })),
        },
    );
    let Some(diagram::Shown::Graph(graph)) = diagram::shown(&feed, "T-1:1", Some("go")) else {
        panic!("the recipe is served ready");
    };
    assert!(
        !diagram::pending(&graph),
        "a failed read is not asked again"
    );
    assert!(matches!(
        graph.inspector.map(|i| i.state),
        Some(diagram::Evidence::Failed(_))
    ));
}

#[test]
fn escape_backs_out_of_another_artifact_left_after_a_recipe_selection() {
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
    state.act(
        Action::Pick {
            reference: "T-1:1".into(),
            element: "go".into(),
        },
        &feed,
    );
    state.act(Action::Close, &feed);
    state.act(
        Action::Cite {
            reference: "T-1:1".into(),
            section: Some("goal".into()),
        },
        &feed,
    );
    state.act(
        Action::Pick {
            reference: "T-1:1".into(),
            element: "pass".into(),
        },
        &feed,
    );
    assert_eq!(
        state.pick,
        Some(("T-1:1".into(), "go".into())),
        "a cited section draws no diagram to select in"
    );
    assert!(state.escape());
    assert_eq!(state.full, None, "the first Escape leaves the section");

    state.act(
        Action::Cite {
            reference: "T-2:4".into(),
            section: None,
        },
        &feed,
    );
    assert!(state.escape());
    assert_eq!(
        state.full, None,
        "the first Escape leaves another artifact too"
    );
}

#[test]
fn a_comparison_reads_elements_only_at_the_commit_it_pinned() {
    use super::diagram_compare as cmp;
    use corral_protocol::detail::{
        CompareOrigin, CompareResolution, CompareTarget, DetailRef, DiagramCompareDetail,
    };
    let mut feed = Feed::default();
    let r = recipe(&[("go", "a", "b"), ("pass", "b", "c"), ("retry", "b", "a")]);
    served(&mut feed, "T-1:1", r.clone());
    let sha = "ab".repeat(32);
    let mut state = State {
        full: Some("T-1:1".into()),
        ..State::default()
    };
    state.act(
        Action::Compare {
            reference: "T-1:1".into(),
            origin: Some(CompareOrigin::ObservedHead),
        },
        &feed,
    );
    let head = "c".repeat(40);
    let summary = cmp::target("T-1:1", &sha, state.compare.as_ref().unwrap(), None);
    feed.details.insert(
        detail_key(&summary),
        DetailCache {
            transcript_seams: Default::default(),
            target: summary,
            pages: Vec::new(),
            loading: false,
            stale: false,
            error: None,
            merged: Some(DetailBody::ArtifactDiagramCompare(DiagramCompareDetail {
                reference: "T-1:1".into(),
                sha256: sha.clone(),
                subject: r.subject.clone(),
                target: CompareResolution::Resolved {
                    origin: CompareOrigin::ObservedHead,
                    commit: head.clone(),
                    candidate: None,
                    current_candidate: false,
                },
                elements: Vec::new(),
                selected: None,
            })),
        },
    );
    let Some(diagram::Shown::Graph(mut graph)) = diagram::shown(&feed, "T-1:1", Some("go")) else {
        panic!("the recipe is served ready");
    };
    let unpinned = cmp::view(&feed, &graph, state.compare.as_ref().unwrap());
    assert_eq!(unpinned.pin.as_deref(), Some(head.as_str()));
    assert_eq!(
        unpinned.reads.len(),
        1,
        "no element is read before the commit is pinned"
    );
    let pin = |commit: &str| Action::Pin {
        reference: "T-1:1".into(),
        commit: commit.into(),
    };
    state.act(pin(&head), &feed);
    state.act(pin(&"d".repeat(40)), &feed);
    graph.selected = Some("go".into());
    let pinned = cmp::view(&feed, &graph, state.compare.as_ref().unwrap());
    assert!(
        matches!(
            pinned.reads.as_slice(),
            [_, DetailRef::ArtifactDiagramCompare { target: CompareTarget::Pinned { commit, .. }, element: Some(id), .. }]
                if *commit == head && id == "go"
        ),
        "a moved head never replaces the pinned commit: {:?}",
        pinned.reads
    );
}

#[test]
fn a_comparison_link_never_shows_a_later_submission_under_its_name() {
    use crate::model::review::impact::expected::{Drawn, Expected, Verdict, verdict};
    use crate::model::review::impact::{Action as ImpactAction, State as ImpactState, View};
    use corral_protocol::detail::{DetailRef, ReviewDetail};
    let review = |head: &str| -> ReviewDetail {
        serde_json::from_value(serde_json::json!({
            "task": "T-1", "diff": null, "checks": [], "max_rounds": 3, "files": [], "head": head,
            "run": {
                "id": "R2", "task": "T-1", "role": "worker", "profile": "p", "harness": "pi",
                "worktree": "/w", "branch": "corral/T-1", "base": "main",
                "base_sha": "0".repeat(40), "started": "2026-01-01T00:00:00Z",
                // A landing rewrote the run's head; the stored diff's submission is `head`.
                "head_sha": "f".repeat(40)
            }
        }))
        .expect("a review detail")
    };
    let serve = |feed: &mut Feed, head: &str| {
        let target = DetailRef::Review { task: "T-1".into() };
        feed.details.insert(
            detail_key(&target),
            DetailCache {
                transcript_seams: Default::default(),
                target,
                pages: Vec::new(),
                loading: false,
                stale: false,
                error: None,
                merged: Some(DetailBody::Review(review(head))),
            },
        );
    };
    let linked = Expected {
        head: "a".repeat(40),
        path: Some("src/review.rs".into()),
    };
    let mut feed = Feed::default();
    assert_eq!(
        verdict(&feed, "T-1", View::Diff, &linked, Drawn::default()),
        Verdict::Waiting
    );
    serve(&mut feed, &"a".repeat(40));
    assert_eq!(
        verdict(&feed, "T-1", View::Diff, &linked, Drawn::default()),
        Verdict::Same
    );
    // A newer submission superseded the one the comparison read.
    serve(&mut feed, &"b".repeat(40));
    assert!(matches!(
        verdict(&feed, "T-1", View::Diff, &linked, Drawn::default()),
        Verdict::Other(why) if why.contains("bbbbbbbbbbbb") && why.contains("aaaaaaaaaaaa")
    ));
    // The review still names the linked submission, but an expanded file's page, read on its
    // own, belongs to the newer one: the whole view is withheld.
    serve(&mut feed, &"a".repeat(40));
    let page = DetailRef::ReviewFile {
        task: "T-1".into(),
        path: "src/review.rs".into(),
    };
    feed.details.insert(
        detail_key(&page),
        DetailCache {
            transcript_seams: Default::default(),
            target: page,
            pages: Vec::new(),
            loading: false,
            stale: false,
            error: None,
            merged: Some(DetailBody::ReviewFile(corral_protocol::detail::PatchPage {
                path: "src/review.rs".into(),
                patch: "+new".into(),
                truncated: false,
                candidate: None,
                head: Some("b".repeat(40)),
            })),
        },
    );
    let patches = ["src/review.rs".to_owned()];
    let drawn = Drawn {
        patches: &patches,
        levels: &[],
    };
    assert!(matches!(
        verdict(&feed, "T-1", View::Diff, &linked, drawn),
        Verdict::Other(why) if why.contains("patch for src/review.rs")
    ));
    // A read that failed confirms nothing.
    let root = DetailRef::Review { task: "T-1".into() };
    feed.details.get_mut(&detail_key(&root)).unwrap().error =
        Some(crate::model::daemon::ClientError {
            code: "not_found".into(),
            message: "task has no submitted worker run".into(),
        });
    assert!(matches!(
        verdict(&feed, "T-1", View::Diff, &linked, Drawn::default()),
        Verdict::Other(_)
    ));
    let mut state = ImpactState::default();
    state.follow("T-1");
    state.act(ImpactAction::Expect(Some(linked)));
    state.follow("T-2");
    assert_eq!(
        state.expect, None,
        "another task's page never inherits the expectation"
    );
}
