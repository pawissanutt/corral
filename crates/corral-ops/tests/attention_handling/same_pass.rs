use corral_core::{Accept, Status, Verdict};
use serde_json::json;

use super::operating_fixture;

#[test]
fn owning_sync_assesses_dispatch_landing_answer_and_wait_after_their_mutations() {
    // Given a real candidate in independent review and two live worker obligations.
    let mut fx = operating_fixture::Fx::new();
    fx.ctx.store.config.review.accept = Some(Accept::Auto);
    fx.ctx.store.save_config().unwrap();
    let landing = fx.task("Landing", Status::Ready);
    fx.start(&landing.id);
    fx.done(&landing.id, "landed.txt");
    fx.grant(&[&landing.id]);
    corral_ops::sync_at(&fx.ctx, super::at(0)).unwrap();
    assert!(
        fx.ctx
            .store
            .get(&landing.id)
            .unwrap()
            .current_run()
            .unwrap()
            .review
            .is_some()
    );
    let question = fx.task("Question", Status::Ready);
    fx.start(&question.id);
    let question_key = format!("{}/R1/Q1", question.id);
    fx.ctx.store.update(&question.id, |task| {
        let mut host = corral_core::agent::AgentRunState::new(corral_core::agent::AgentBinding {
            role:corral_core::agent::AgentRole::Worker, store:"fixture".into(), project:"app".into(),
            generation:1, health:corral_core::agent::HostHealth::Healthy, callback:None, closing:false,
        });
        host.questions.push(serde_json::from_value(json!({
            "version":1,"key":question_key,"revision":1,"kind":"clarification","blocking":false,
            "text":"Which constraint applies?","state":"open",
            "asked":{"request_id":"ask","payload_sha256":"sha","session_id":"session","generation":1,"at":super::at(0)},
            "wake":{"route":"none","key":"wake","state":"pending"}
        }))?);
        task.current_run_mut().unwrap().agent_host = Some(Box::new(host));
        Ok(())
    }).unwrap();
    let waiting = fx.task("Wait", Status::Ready);
    fx.start(&waiting.id);
    fx.ctx.store.update(&waiting.id, |task| {
        task.current_run_mut().unwrap().obligations.waits.push(serde_json::from_value(json!({
            "request":{"version":1,"id":"channel-1","run":"R1","condition":"channel ready","owner":"worker",
                "source":"worker","deadline":super::at(1000),
                "handle":{"kind":"channel","channel_id":"channel","capability":"wait_v1"}},
            "registered_at":super::at(0),
            "observation":{"at":super::at(0),"state":"pending","detail":"waiting"},
            "receipt":null
        }))?);
        Ok(())
    }).unwrap();
    corral_ops::sync_at(&fx.ctx, super::at(0)).unwrap();
    let old_key = fx
        .ctx
        .store
        .get(&question.id)
        .unwrap()
        .attention
        .records
        .keys()
        .find(|key| key.0.contains("/question_open/"))
        .unwrap()
        .clone();
    corral_ops::verdict(
        &fx.ctx,
        &landing.id,
        Verdict::Approve,
        "Independent review passed",
    )
    .unwrap();
    let queued = fx.task("Queued", Status::Ready);
    fx.grant(&[&landing.id, &queued.id]);
    corral_ops::enqueue(&fx.ctx, &queued.id, &corral_ops::DispatchOpts::default()).unwrap();
    corral_ops::questions::answer(
        &fx.ctx,
        &question_key,
        1,
        corral_ops::questions::Actor::Owner,
        corral_ops::questions::AnswerInput {
            text: "Use the documented constraint".into(),
            ..Default::default()
        },
    )
    .unwrap();
    fx.ctx
        .store
        .update(&waiting.id, |task| {
            task.current_run_mut().unwrap().obligations.waits[0].receipt =
                Some(corral_core::ResumeReceipt {
                    version: 1,
                    id: "channel-1".into(),
                    run: "R1".into(),
                    outcome: corral_core::ReceiptOutcome::Ready,
                    delivery: corral_core::ResumeDelivery::Acknowledged,
                    observed_at: super::at(1),
                    answer: None,
                    data: json!({}),
                });
            Ok(())
        })
        .unwrap();
    // When one owning sync runs all real executors and reconciles at its end.
    corral_ops::sync_at(&fx.ctx, super::at(1)).unwrap();
    // Then its fresh assessment observes each post-mutation state.
    assert_eq!(fx.ctx.store.get(&landing.id).unwrap().status, Status::Done);
    assert!(!fx.records("submissions.jsonl").is_empty());
    assert_eq!(
        fx.ctx.store.get(&queued.id).unwrap().status,
        Status::Running,
        "queue after final sync: {:?}",
        corral_ops::dispatch_queue(&fx.ctx).unwrap(),
    );
    let answered = fx.ctx.store.get(&question.id).unwrap();
    assert!(!answered.attention.records.contains_key(&old_key));
    let events = fx.ctx.store.events().unwrap();
    let resolved: Vec<_> = events
        .iter()
        .filter(|event| event.kind == "attention_resolved" && event.detail["key"] == old_key.0)
        .collect();
    assert_eq!(resolved.len(), 1);
    assert_eq!(
        fx.ctx
            .store
            .get(&waiting.id)
            .unwrap()
            .current_run()
            .unwrap()
            .obligations
            .waits[0]
            .observation
            .state,
        corral_core::WaitState::Ready
    );
    let board = corral_ops::attention::board(&fx.fresh(), super::at(1)).unwrap();
    for id in [&landing.id, &queued.id] {
        assert!(
            board
                .tasks
                .iter()
                .find(|row| &row.task == id)
                .unwrap()
                .assessment
                .conditions
                .is_empty()
        );
    }
    assert!(
        board
            .tasks
            .iter()
            .find(|row| row.task == waiting.id)
            .unwrap()
            .assessment
            .conditions
            .iter()
            .all(|condition| condition.kind != "obligation_wait")
    );
}
