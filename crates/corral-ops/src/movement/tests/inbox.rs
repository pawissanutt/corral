use super::*;
use corral_core::inbox::{DeliveryMode, Inbox, Recipient};

fn native(fx: &Fx) -> Recipient {
    fx.ctx
        .store
        .update(&fx.task.id, |task| {
            let run = task.current_run_mut().unwrap();
            run.harness = Some("pi-native".into());
            run.delivery = DeliveryMode::Inbox;
            Ok(())
        })
        .unwrap();
    Recipient::Run {
        task: fx.task.id.clone(),
        run: "R1".into(),
    }
}

#[test]
fn pi_notice_posts_once_with_context_even_when_transport_is_unbound() {
    let fx = Fx::new();
    let recipient = native(&fx);
    fx.worker_change("file.rs");
    fx.land("file.rs", "fn landed() {}\n");
    fx.tick(0);
    fx.tick(2);
    let first = fx.state();
    for secs in [3, 4, 5] {
        fx.tick(secs);
    }
    let inbox = Inbox::new(&fx.ctx.store, recipient.clone()).unwrap();
    let messages = inbox.messages().unwrap();
    assert_eq!(messages.len(), 1);
    let message = &messages[0];
    assert_eq!(message.kind, "main_movement");
    assert_eq!(message.binding.run.as_deref(), Some("R1"));
    assert_eq!(
        message.subject.as_ref().unwrap()["notice"],
        first.notices[0].id
    );
    let context = first.notices[0].artifact.as_deref().unwrap();
    assert_eq!(message.subject.as_ref().unwrap()["context"], context);
    assert!(message.body.contains(context));
    assert!(Path::new(context).is_file());
    assert_eq!(fx.state().notices.len(), 1);
    assert_eq!(fx.state().notices[0].state, DeliveryState::Queued);
    assert!(
        fx.sent().is_empty(),
        "Pi must never fall back to pane prompt"
    );
    // Delivery failure is visible in the existing inbox alert/event stream.
    assert!(
        fx.ctx
            .store
            .events()
            .unwrap()
            .iter()
            .any(|e| e.kind == "delivery_blocked")
    );
    assert!(
        fx.ctx
            .store
            .get(&fx.task.id)
            .unwrap()
            .current_run()
            .unwrap()
            .pending_reply
            .is_none()
    );
}

#[test]
fn pi_interrupted_enqueue_is_uncertain_without_repost() {
    for posted in [false, true] {
        let fx = Fx::new();
        let recipient = native(&fx);
        fx.worker_change("file.rs");
        let landing = fx.land("file.rs", "fn landed() {}\n");
        let mut state = fx.queued(landing);
        let path = state_path(&fx.ctx, &fx.task.id);
        if posted {
            delivery::deliver(&fx.ctx, &path, &mut state, 0, true).unwrap();
        }
        state.notices[0].state = DeliveryState::Sending;
        save(&path, &state).unwrap();
        // Recover from disk just as a new sync process does.
        let mut recovered = fx.state();
        for _ in 0..2 {
            delivery::deliver(&fx.ctx, &path, &mut recovered, 0, true).unwrap();
        }
        assert_eq!(recovered.notices[0].state, DeliveryState::Uncertain);
        assert_eq!(
            Inbox::new(&fx.ctx.store, recipient)
                .unwrap()
                .messages()
                .unwrap()
                .len(),
            usize::from(posted)
        );
        assert!(fx.sent().is_empty());
    }
}

#[test]
fn pi_replaced_pane_cancels_before_enqueue() {
    let fx = Fx::new();
    let recipient = native(&fx);
    fx.worker_change("file.rs");
    let landing = fx.land("file.rs", "fn landed() {}\n");
    let mut state = fx.queued(landing);
    fx.ctx
        .store
        .update(&fx.task.id, |task| {
            task.current_run_mut()
                .unwrap()
                .herdr
                .as_mut()
                .unwrap()
                .pane_id = "w1:p2".into();
            Ok(())
        })
        .unwrap();
    delivery::deliver(
        &fx.ctx,
        &state_path(&fx.ctx, &fx.task.id),
        &mut state,
        0,
        true,
    )
    .unwrap();
    assert_eq!(state.notices[0].state, DeliveryState::Cancelled);
    assert!(
        Inbox::new(&fx.ctx.store, recipient)
            .unwrap()
            .messages()
            .unwrap()
            .is_empty()
    );
    assert!(fx.sent().is_empty());
}
