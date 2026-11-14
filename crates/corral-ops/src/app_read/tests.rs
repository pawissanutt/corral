use super::*;
use corral_protocol::detail::{Burnup, BurnupPoint, DetailBody};

#[test]
fn detail_pages_reassemble_every_typed_array_item() -> Result<()> {
    let points: Vec<_> = (0..3000)
        .map(|index| BurnupPoint {
            at: "2026-11-14T00:00:00Z".to_owned(),
            total: index,
            landed: index / 2,
        })
        .collect();
    let full = DetailBody::Burnup(Burnup {
        scope: "all".to_owned(),
        points: points.clone(),
    });
    let mut offset = 0;
    let mut assembled = Vec::new();
    loop {
        let (body, next) = paging::body_page(full.clone(), offset)?;
        let reply = paging::reply(body, next, "a".repeat(64), "selector".to_owned(), now(), 1)?;
        assert!(paging::json_len(&reply)? < corral_protocol::app::APP_CONTROL_MAX);
        let Some(DetailBody::Burnup(page)) = reply.body else {
            panic!("wrong detail body");
        };
        assembled.extend(page.points);
        let Some(next) = next else {
            break;
        };
        assert!(next > offset);
        offset = next;
    }
    assert_eq!(assembled, points);
    Ok(())
}

#[test]
fn continuation_rejects_another_selector_or_content_revision() -> Result<()> {
    let reply = paging::reply(
        DetailBody::Burnup(Burnup {
            scope: "all".to_owned(),
            points: vec![],
        }),
        Some(1),
        "a".repeat(64),
        "task-one".to_owned(),
        now(),
        1,
    )?;
    let cursor = paging::decode(reply.next.as_deref(), "task-one")?;
    assert!(paging::check(cursor.as_ref(), &"b".repeat(64)).is_err());
    assert!(paging::decode(reply.next.as_deref(), "task-two").is_err());
    assert!(paging::decode(Some("{}"), "task-one").is_err());
    Ok(())
}

#[test]
fn utf8_bounds_and_oversized_metadata_fail_explicitly() -> Result<()> {
    assert_eq!(short("aéz", 2), "a");
    assert_eq!(short("aéz", 3), "aé");
    let body = DetailBody::Burnup(Burnup {
        scope: "x".repeat(70 * 1024),
        points: vec![],
    });
    assert!(paging::body_page(body, 0).is_err());
    Ok(())
}

#[test]
fn fixed_task_rails_remain_valid_on_every_page() -> Result<()> {
    use corral_protocol::{
        board::{RailGlyph, RailStep},
        detail::{ArtifactRef, TaskDetail},
    };
    let full = DetailBody::Task(TaskDetail {
        id: "T-1".into(),
        owner_revision: "canonical".into(),
        title: "Task".into(),
        status: "in_progress".into(),
        body_excerpt: String::new(),
        stages: std::array::from_fn(|_| RailStep {
            glyph: RailGlyph::Unavailable,
            sha7: None,
            at: None,
        }),
        dependencies: vec![],
        progress: None,
        owner_next: None,
        hold: None,
        park: None,
        artifacts: (0..1000)
            .map(|index| ArtifactRef {
                source_ref: format!("T-1:R1:{index}"),
                title: "x".repeat(100),
                media_type: None,
            })
            .collect(),
        run: None,
        plan_context: None,
        log_tail: vec![],
    });
    let mut offset = 0;
    let mut count = 0;
    loop {
        let (body, next) = paging::body_page(full.clone(), offset)?;
        let DetailBody::Task(page) = body else {
            panic!("wrong body");
        };
        assert_eq!(page.stages.len(), 4);
        count += page.artifacts.len();
        let Some(next) = next else {
            break;
        };
        offset = next;
    }
    assert_eq!(count, 1000);
    Ok(())
}

#[test]
fn large_annotation_preserves_all_messages_with_same_canonical_identity() -> Result<()> {
    use corral_protocol::{
        cmd::annotation::AnnotationSide,
        detail::{AnnotationMessage, ReviewAnnotation, ReviewDetail},
    };
    let full = DetailBody::Review(ReviewDetail {
        task: "T-1".into(),
        candidate_sha: None,
        base: None,
        state: "review".into(),
        verdicts: vec![],
        files: vec![],
        report: None,
        acceptance: vec![],
        gate_log_tail: vec![],
        artifacts: vec![],
        approvable: false,
        annotations: vec![ReviewAnnotation {
            id: "thread-1".into(),
            revision: 1,
            run: "R1".into(),
            base: "base".into(),
            head: "head".into(),
            path: "file.rs".into(),
            side: AnnotationSide::New,
            range: None,
            resolved: false,
            stale: false,
            messages: (0..64)
                .map(|index| AnnotationMessage {
                    by: "owner".into(),
                    at: format!("2026-11-14T00:00:{index:02}Z"),
                    text: "x".repeat(4096),
                })
                .collect(),
        }],
    });
    let mut offset = 0;
    let mut messages = 0;
    loop {
        let (body, next) = paging::body_page(full.clone(), offset)?;
        assert!(paging::json_len(&body)? < corral_protocol::app::APP_CONTROL_MAX - 2048);
        let DetailBody::Review(page) = body else {
            panic!("wrong body");
        };
        for annotation in page.annotations {
            assert_eq!(annotation.id, "thread-1");
            messages += annotation.messages.len();
        }
        let Some(next) = next else {
            break;
        };
        offset = next;
    }
    assert_eq!(messages, 64);
    Ok(())
}

#[test]
fn assistant_feed_usage_reads_bound_transcripts_without_a_stored_snapshot() -> Result<()> {
    use corral_core::Store;
    use corral_protocol::board::CostBasis;

    let tmp = tempfile::tempdir()?;
    let store = Store::init(tmp.path())?;
    let session = store.create_assistant("pi", None, None, None)?;
    let transcript = tmp.path().join("session.jsonl");
    std::fs::write(
        &transcript,
        concat!(
            "{\"type\":\"session\",\"id\":\"sid\"}\n",
            "{\"type\":\"message\",\"id\":\"m1\",",
            "\"message\":{\"role\":\"assistant\",\"provider\":\"llama.cpp\",",
            "\"usage\":{\"input\":23,\"output\":7,\"cost\":{\"total\":0}}}}\n"
        ),
    )?;
    store.update_assistant_container(&session.id, |container| {
        for n in 1..=2 {
            container
                .state
                .generations
                .push(serde_json::from_value(serde_json::json!({
                    "n": n, "bind_nonce_sha256": "nonce", "pane": "pane",
                    "created": session.created,
                    "bound": {
                        "host_cap_sha256": "host", "ctl_cap_sha256": "ctl", "session_id": "sid",
                        "session_file": transcript, "pid": 123, "start_time": "start",
                        "request_id": "hello", "at": session.created
                    }
                }))?);
        }
        container.state.binding.generation = 2;
        Ok(())
    })?;
    let unknown = store.create_assistant("pi", None, None, None)?;
    let ctx = crate::resources::FixedResourceProbe::ctx(store);
    for ended in [false, true] {
        if ended {
            ctx.store.end_assistant(&session.id)?;
        }
        assert!(ctx.store.get_assistant(&session.id)?.usage.is_none());
        let assistants = assistant_rows(&ctx)?;
        let assistant = assistants.iter().find(|row| row.id == session.id).unwrap();
        assert_eq!(assistant.cost_usd, Some(0.0));
        assert_eq!(assistant.cost_basis, Some(CostBasis::Reported));
        let agents = agents_rows(&ctx)?;
        let agent = agents.iter().find(|row| row.id == session.id).unwrap();
        assert_eq!(
            agent.tokens,
            Some(30),
            "resumed generations must not duplicate usage"
        );
        assert_eq!(agent.cost_usd, Some(0.0));
        assert_eq!(agent.cost_basis, Some(CostBasis::Reported));
        let missing = agents.iter().find(|row| row.id == unknown.id).unwrap();
        assert_eq!(missing.tokens, None);
        assert_eq!(missing.cost_usd, None);
        assert_eq!(missing.cost_basis, None);
    }
    Ok(())
}
