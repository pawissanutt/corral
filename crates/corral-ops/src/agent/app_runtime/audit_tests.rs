use super::*;
use corral_protocol::assistant::{EntryCard, StopOutcome, ThreadEntry, TurnCompletion};
use serde_json::json;

#[test]
fn chunk_metadata_identifies_one_turn_and_canonical_stream_coverage() {
    let fx = crate::agent::test_support::fx();
    let head = fx.ctx.store.app_thread("quick").unwrap();
    let text = "界".repeat(3000);
    let (saved, _) = fx
        .ctx
        .store
        .append_app_message(message(&head.session, text.clone()), "chunked", &[])
        .unwrap();
    fx.ctx
        .store
        .update_app_message(&saved.id, |message| {
            message.state = "working".into();
            message.turn = Some(7);
            message.reply = Some(text.clone());
            Ok(())
        })
        .unwrap();
    let (rows, _, _) = thread(&fx.ctx, "quick", None, 100).unwrap();
    for kind in [EntryKind::Message, EntryKind::Answer] {
        let parts: Vec<_> = rows.iter().filter(|entry| entry.kind == kind).collect();
        assert_eq!(parts.len(), 2);
        let anchor = if kind == EntryKind::Message {
            saved.id.clone()
        } else {
            format!("{}/answer/0", saved.id)
        };
        for (index, entry) in parts.iter().enumerate() {
            let part = entry.part.as_ref().unwrap();
            assert_eq!(
                (&part.id, part.index, part.total, part.bytes),
                (&anchor, index as u32, 2, text.len() as u64)
            );
            assert_eq!(part.complete, kind == EntryKind::Message);
        }
        assert_eq!(
            parts
                .iter()
                .filter_map(|entry| entry.text.as_deref())
                .collect::<String>(),
            text
        );
    }
    let live = live(&fx.ctx, "quick").unwrap().unwrap();
    assert_eq!(live.entry, format!("{}/answer/0", saved.id));
    fx.ctx
        .store
        .update_app_message(&saved.id, |message| {
            message.state = "done".into();
            Ok(())
        })
        .unwrap();
    let (rows, _, _) = thread(&fx.ctx, "quick", None, 100).unwrap();
    assert!(
        rows.iter()
            .all(|entry| entry.part.as_ref().unwrap().complete)
    );
    // Legacy wire records are readable without claiming a completion reason or cost basis.
    let mut old = serde_json::to_value(&rows[0]).unwrap();
    for field in ["part", "completion", "cost_basis"] {
        old.as_object_mut().unwrap().remove(field);
    }
    let old: ThreadEntry = serde_json::from_value(old).unwrap();
    assert!(old.part.is_none() && old.completion.is_none() && old.cost_basis.is_none());
}

#[cfg(target_os = "linux")]
#[test]
fn failed_stop_is_retryable_and_normal_completion_does_not_claim_abort() {
    let host = PauseHost::new(true);
    let ctx = &host.fx.ctx;
    let ack = send(
        ctx,
        &host.device,
        &request(&host.session, "stoppable", AskTarget::Assistant),
    )
    .unwrap();
    let generation = ctx
        .store
        .get_assistant_container(&host.session)
        .unwrap()
        .state
        .binding
        .generation;
    let message = ctx
        .store
        .update_app_message(&ack.entry, |message| {
            message.state = "working".into();
            message.turn = Some(7);
            message.generation = Some(generation);
            Ok(())
        })
        .unwrap();
    let mut runtime = start(ctx, &host.session).unwrap();
    host.cancel_results
        .lock()
        .unwrap()
        .push_back(json!({"cancelled": false, "reason": "provider_failed"}));
    assert_eq!(
        stop(ctx, &host.device, &host.session, 7).unwrap().outcome,
        StopOutcome::Failed
    );
    assert_eq!(ctx.store.app_message(&ack.entry).unwrap().state, "working");
    assert_eq!(
        stop(ctx, &host.device, &host.session, 7).unwrap().outcome,
        StopOutcome::Accepted
    );
    assert_eq!(
        stop(ctx, &host.device, &host.session, 7).unwrap().outcome,
        StopOutcome::AlreadyStopped
    );
    // Settlement can win the race against a cancellation acknowledgement. The actual host
    // outcome, not any of those Stop outcomes, is the source of the completed answer status.
    host.events.lock().unwrap().push(json!({
        "seq": 1, "kind": "owner_done", "entry": message.id,
        "input": message.input, "turn": 7, "aborted": false,
    }));
    pump(ctx, &mut runtime).unwrap();
    let (rows, _, _) = thread(ctx, &host.session, None, 100).unwrap();
    let answer = rows
        .iter()
        .find(|entry| entry.kind == EntryKind::Answer)
        .unwrap();
    assert_eq!(answer.completion, Some(TurnCompletion::Completed));
    assert!(answer.part.as_ref().unwrap().complete);
}

#[test]
fn free_text_question_exposes_revision_bound_answer_path_until_a_winner_settles_it() {
    use crate::agent::test_support as ts;
    let fx = ts::fx();
    let head = fx.ctx.store.app_thread("quick").unwrap();
    let mut host = ts::bind(
        &fx,
        &crate::agent::BindTarget::Assistant {
            session: head.session.clone(),
        },
        "owner-pane",
        "text-question",
    );
    ts::committed(&ts::hello(&fx, &host));
    ts::committed(&ts::send(
        &fx,
        &ts::event_input(&host, 1, "turn_started", 1),
        "event",
    ));
    host.identity.turn = Some(1);
    let ask_id = crate::agent::tool_request_id(&host.identity.session_id, "free-text");
    let receipt = ts::committed(&ts::send(
        &fx,
        &ts::envelope(
            &host,
            "owner_ask",
            &ask_id,
            json!({"action": "open", "text": "What should the report cover?", "blocking": true}),
            false,
        ),
        "owner_ask",
    ));
    let key = receipt.result["key"].as_str().unwrap().to_owned();
    let mut owner_message = message(&head.session, "Prepare a report".into());
    owner_message.turn = Some(1);
    owner_message.generation = Some(host.identity.generation);
    fx.ctx
        .store
        .append_app_message(owner_message, "question", &[])
        .unwrap();
    let (rows, _, _) = thread(&fx.ctx, "quick", None, 100).unwrap();
    let asked = rows.iter().find(|entry| entry.id == key).unwrap();
    assert!(
        matches!(&asked.card, Some(EntryCard::Question { accepts_text: true, options, answer: None, .. }) if options.is_empty())
    );
    assert!(
        asked
            .links
            .iter()
            .any(|link| link.kind == "question" && link.id == key && link.rev == Some(1))
    );
    let input = crate::questions::AnswerInput {
        text: "Cover the daemon audit".into(),
        option: None,
        ..Default::default()
    };
    crate::questions::answer(
        &fx.ctx,
        &key,
        1,
        crate::questions::Actor::Owner,
        input.clone(),
    )
    .unwrap();
    assert!(
        crate::questions::answer(&fx.ctx, &key, 1, crate::questions::Actor::Owner, input).is_err()
    );
    let (rows, _, _) = thread(&fx.ctx, "quick", None, 100).unwrap();
    let answered = rows.iter().find(|entry| entry.id == key).unwrap();
    assert!(
        matches!(&answered.card, Some(EntryCard::Question { accepts_text: false, answer: Some(answer), .. }) if answer.option.is_none())
    );
}

#[test]
fn ask_cost_joins_structural_turn_accounting_and_preserves_unknown() {
    use crate::agent::test_support as ts;
    let fx = ts::fx();
    let head = fx.ctx.store.app_thread("quick").unwrap();
    let host = ts::bind(
        &fx,
        &crate::agent::BindTarget::Assistant {
            session: head.session.clone(),
        },
        "owner-pane",
        "priced-ask",
    );
    ts::committed(&ts::hello(&fx, &host));
    let generation = host.identity.generation;
    let transcript = [
        json!({"type": "session", "id": "priced-ask"}),
        json!({"type": "custom", "customType": "corral_cost_turn", "data": {"phase": "start", "generation": generation, "turn": 1}}),
        json!({"type": "message", "id": "first-response", "message": {"role": "assistant", "usage": {"input": 10, "output": 5, "cost": {"total": 0.12}}}}),
        json!({"type": "message", "id": "second-response", "message": {"role": "assistant", "usage": {"input": 10, "output": 5, "cost": {"total": 0.23}}}}),
        json!({"type": "custom", "customType": "corral_cost_turn", "data": {"phase": "end", "generation": generation, "turn": 1}}),
    ].into_iter().map(|line| format!("{line}\n")).collect::<String>();
    std::fs::create_dir_all(&host.sessions).unwrap();
    std::fs::write(host.sessions.join("session.jsonl"), transcript).unwrap();
    let mut owner_message = message(&head.session, "A priced ask".into());
    owner_message.turn = Some(1);
    owner_message.generation = Some(generation);
    owner_message.state = "done".into();
    owner_message.reply = Some("x".repeat(8 * 1024));
    let (saved, _) = fx
        .ctx
        .store
        .append_app_message(owner_message, "priced", &[])
        .unwrap();
    let (rows, _, _) = thread(&fx.ctx, "quick", None, 100).unwrap();
    let answers: Vec<_> = rows
        .iter()
        .filter(|entry| entry.kind == EntryKind::Answer)
        .collect();
    assert!((answers[0].cost_usd.unwrap() - 0.35).abs() < 1e-10);
    assert_eq!(
        answers[0].cost_basis,
        Some(corral_protocol::board::CostBasis::Reported)
    );
    assert!(answers[1].cost_usd.is_none() && answers[1].cost_basis.is_none());
    fx.ctx
        .store
        .update_app_message(&saved.id, |message| {
            message.turn = Some(2);
            Ok(())
        })
        .unwrap();
    let (rows, _, _) = thread(&fx.ctx, "quick", None, 100).unwrap();
    assert!(
        rows.iter()
            .filter(|entry| entry.kind == EntryKind::Answer)
            .all(|entry| entry.cost_usd.is_none() && entry.cost_basis.is_none())
    );
}

#[test]
fn stale_ready_without_a_live_host_projects_open_with_a_meaningful_title() {
    let fx = crate::agent::test_support::fx();
    let session = fx
        .ctx
        .store
        .create_assistant("pi", None, None, None)
        .unwrap();
    fx.ctx.store.app_thread(&session.id).unwrap();
    fx.ctx
        .store
        .append_app_message(
            message(
                &session.id,
                "Investigate daemon accounting\nExtra context".into(),
            ),
            "title",
            &[],
        )
        .unwrap();
    fx.ctx
        .store
        .set_app_thread_state(&session.id, "ready", None)
        .unwrap();
    let row = projection::row(&fx.ctx, &session.id).unwrap();
    assert_eq!(row.title, "Investigate daemon accounting");
    assert_eq!(row.state, corral_protocol::rows::AssistantState::Cold);
    assert_eq!(row.live_in, corral_protocol::rows::AssistantLocation::None);
}
