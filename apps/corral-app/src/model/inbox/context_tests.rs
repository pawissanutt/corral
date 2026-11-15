use super::context::*;
use super::{State, question_ref};
use crate::model::daemon::{DetailCache, Feed, detail_key};
use corral_protocol::detail::{DetailBody, DetailRef, DetailReply};

#[test]
fn question_loads_its_plan_and_selected_canonical_image_without_external_fetches() {
    let question: DetailBody = serde_json::from_value(serde_json::json!({
        "kind":"question", "body":{
            "key":"T-1/R1/Q1", "rev":1, "state":{"state":"open"},
            "kind":"worker_question", "blocking":true, "task":"T-1", "plan":"T-2",
            "from":{"role":"worker","session":null,"profile":null},
            "asked_at":"2026-11-15T00:00:00Z", "needed_by":null, "text":"Choose",
            "options":[], "recommendation":null,
            "place":{"goal":null,"position":null,"waiting":[]},
            "evidence":[
                {"title":"Shot", "source_ref":"T-3:R1:shot.png", "body":{
                    "kind":"image", "artifact":{"source_ref":"T-3:R1:shot.png","title":"Shot","media_type":"image/png"}
                }},
                {"title":"Link", "source_ref":"https://example.invalid/shot.png", "body":{
                    "kind":"image", "artifact":{"source_ref":"https://example.invalid/shot.png","title":"Link","media_type":"image/png"}
                }}
            ],
            "thread":[], "history":[], "answer":null, "deadline":null
        }
    })).unwrap();
    let target = question_ref("T-1/R1/Q1");
    let mut feed = Feed::default();
    feed.details.insert(
        detail_key(&target),
        DetailCache {
            target,
            pages: Vec::new(),
            loading: false,
            stale: false,
            error: None,
            merged: Some(question),
        },
    );
    let mut state = State::default();
    let refs = question_context_refs(&state, &feed, "T-1/R1/Q1");
    assert_eq!(
        refs,
        vec![
            DetailRef::Plan { id: "T-2".into() },
            DetailRef::Artifact {
                task: "T-3".into(),
                key: "T-3:R1:shot.png".into()
            },
        ]
    );
    assert_eq!(question_dependencies(&state, &feed, "T-1/R1/Q1").len(), 2);
    let image = refs[1].clone();
    feed.details.insert(
        detail_key(&image),
        DetailCache {
            target: image.clone(),
            pages: vec![DetailReply {
                etag: "image-rev".into(),
                as_of: String::new(),
                seq: 1,
                next: Some("next-image-page".into()),
                body: None,
            }],
            loading: false,
            stale: false,
            error: None,
            merged: None,
        },
    );
    let requests = question_dependencies(&state, &feed, "T-1/R1/Q1");
    assert_eq!(requests[1].target, image);
    assert_eq!(requests[1].page.as_deref(), Some("next-image-page"));
    state
        .desk
        .drafts
        .entry("T-1/R1/Q1".into())
        .or_default()
        .evidence = 1;
    assert_eq!(
        question_context_refs(&state, &feed, "T-1/R1/Q1"),
        vec![refs[0].clone()]
    );
}
