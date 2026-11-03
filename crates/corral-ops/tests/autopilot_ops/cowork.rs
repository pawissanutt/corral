use super::*;
use corral_core::inbox::{Inbox, Recipient};
use serde_json::json;

fn cowork(fx: &Fx, confirmed: bool) {
    fx.ctx
        .store
        .create_assistant("fake", None, None, None)
        .unwrap();
    fx.ctx
        .store
        .update(&fx.id, |task| {
            task.cowork = Some(serde_json::from_value(json!({
                "session": "A-1", "scratch": "widget", "run": "R1",
                "review": "required", "owner_confirmed": confirmed
            }))?);
            // Co-work never had a worker process or worker conversation.
            let run = task.current_run_mut().unwrap();
            run.herdr = None;
            run.session = None;
            Ok(())
        })
        .unwrap();
}

#[test]
fn cowork_failures_return_to_assistant_or_actionable_owner_without_worker_recovery() {
    // One gate-failure journey; closed/missing assistant variants are cheap destination tests.
    {
        let fx = Fx::new(|r| {
            r.gate = gate(&["false"]);
            r.reviewer = Some("rev".into());
            r.accept = Some(Accept::Auto);
        });
        fx.change("widget.txt", "spins\n");
        fx.done();
        cowork(&fx, true);
        fx.sync();
        let task = fx.task();
        assert_eq!(task.status, Status::NeedsMe);
        assert!(!task.holds_dispatch_slot());
        assert!(!fx.ctx.store.head(&fx.id).unwrap().holds_dispatch_slot());
        assert!(!fx.reason().contains("worker context"), "{}", fx.reason());
        assert!(fx.reason().contains("A-1"), "{}", fx.reason());
        assert!(fx.reason().contains("corral pi --resume A-1") || fx.reason().contains("ended"));
        let inbox = Inbox::new(
            &fx.ctx.store,
            Recipient::Assistant {
                session: "A-1".into(),
            },
        )
        .unwrap();
        let messages = inbox.messages().unwrap();
        assert_eq!(messages.len(), 1);
        assert!(messages[0].body.contains("--into"));
        assert!(messages[0].body.contains(&fx.id));
        let gates = fx.events("gate_started");
        fx.commit_on_main("unrelated.txt", "moved\n", "main moves");
        fx.sync();
        assert_eq!(fx.events("gate_started"), gates);
        assert_eq!(fx.task().runs.len(), 1);
    }
}

#[test]
fn cowork_waiting_for_owner_acceptance_does_not_regate_on_main_moves() {
    let fx = Fx::new(|r| {
        r.gate = gate(&["true"]);
        r.reviewer = Some("rev".into());
        r.accept = Some(Accept::Owner);
    });
    fx.change("widget.txt", "spins\n");
    fx.done();
    cowork(&fx, false); // An old, unconfirmed finish retains owner acceptance.
    fx.sync();
    fx.verdict(Verdict::Approve, "reviewed");
    fx.sync();
    assert_eq!(fx.stage(), Some(AutoStage::Reviewed));
    let gates = fx.events("gate_started");
    let candidate = git(&fx.run().worktree, &["rev-parse", "HEAD"]);
    for n in 0..2 {
        fx.commit_on_main("unrelated.txt", &format!("{n}\n"), "main moves");
        fx.sync();
        assert_eq!(fx.events("gate_started"), gates);
        assert_eq!(git(&fx.run().worktree, &["rev-parse", "HEAD"]), candidate);
    }
}
