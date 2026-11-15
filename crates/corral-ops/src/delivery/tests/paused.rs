use super::*;
use crate::session_control::pause::{self, PauseRecord, RECORDS};

#[test]
fn automated_inbox_input_stays_unattempted_until_resume() {
    let (_dir, ctx) = fixture();
    let inbox = Inbox::new(&ctx.store, Recipient::Pm).unwrap();
    let message = inbox
        .post(Post {
            kind: "pm_event_wake",
            urgency: Urgency::Wake,
            author: "corral",
            body: "Read the new board event",
            binding: MessageBinding::default(),
            subject: None,
            max_payload: MAX_PAYLOAD,
        })
        .unwrap();
    RECORDS
        .save(
            &ctx.store,
            "pm",
            &PauseRecord {
                id: "pm".into(),
                generation: Some(1),
                at: ctx.store.now(),
                actor: Some(corral_core::Actor::Owner),
                task: None,
                hold: None,
            },
        )
        .unwrap();
    let fake = Fake::new(SendOutcome::Accepted);

    assert_eq!(tick(&ctx, &fake), DeliveryStatus::Deferred);
    assert_eq!(tick(&ctx, &fake), DeliveryStatus::Deferred);
    assert!(fake.sent.borrow().is_empty());
    assert!(inbox.state().unwrap().open_envelope.is_none());
    assert!(inbox.terminal(message.seq).unwrap().is_none());
    assert_eq!(inbox.pending_messages().unwrap(), vec![message]);

    pause::lift_recipient_owner(&ctx, &Recipient::Pm).unwrap();
    assert_eq!(tick(&ctx, &fake), DeliveryStatus::Sent { envelope: 1 });
    assert_eq!(fake.sent.borrow().len(), 1);
    assert!(
        fake.sent.borrow()[0]
            .payload
            .contains("Read the new board event")
    );
}

#[cfg(unix)]
#[test]
fn owner_task_reply_lifts_the_addressed_session_pause() {
    use crate::delivery::omo_fixture::OmoWorld;

    let world = OmoWorld::new(corral_core::Status::NeedsMe);
    let key = corral_core::agent::ContainerRef::Worker {
        task: world.task.id.clone(),
        run: "R1".into(),
    }
    .key();
    RECORDS
        .save(
            &world.ctx.store,
            &key,
            &PauseRecord {
                id: key.clone(),
                generation: Some(1),
                at: world.ctx.store.now(),
                actor: Some(corral_core::Actor::Owner),
                task: Some(world.task.id.clone()),
                hold: None,
            },
        )
        .unwrap();
    world
        .ctx
        .store
        .set_attribution(corral_core::Actor::Owner, "test")
        .unwrap();
    std::fs::rename(
        world.temp.path().join("agent/rpc-host-daemon"),
        world.temp.path().join("agent/registry-hidden"),
    )
    .unwrap();

    crate::reply(
        &world.ctx,
        &world.task.id,
        "Continue with this owner answer",
    )
    .unwrap();

    assert!(!pause::recipient_paused(&world.ctx, &world.recipient()).unwrap());
    let messages = Inbox::new(&world.ctx.store, world.recipient())
        .unwrap()
        .messages()
        .unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].kind, "owner_reply");
    assert!(messages[0].body.contains("Continue with this owner answer"));
    assert!(world.terminal_writes().is_empty());
}
