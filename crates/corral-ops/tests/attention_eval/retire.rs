//! S12 T-128 replay (in-crate). A real `reconcile_all_with` pass snapshots every task, then a
//! delivery to an earlier task reopens a later task's ended run through `request_changes`. The
//! pass reaches that later task with its stale ended-run snapshot: the retire fence must re-read
//! and cancel nothing. The unfenced control replays the stale retire the fence prevents.
use std::cell::{Cell, RefCell};

use corral_core::Status;
use corral_core::inbox::{
    Envelope, Inbox, MAX_PAYLOAD, MessageBinding, ObserveCursor, Post, Recipient, SendOutcome,
    TerminalState, Urgency,
};
use serde_json::json;

use super::super::{Window, stats};
use crate::Ctx;
use crate::attention::handling_tests::fixture::{Fx, at};
use crate::delivery::{
    BindOutcome, Capabilities, DeliveryAdapter, Observation, Readiness, RecipientRef,
    SessionBinding,
};

/// Records sends; the first send to `trigger` reopens `reopen` mid-pass, as T-128 did.
struct Transport {
    trigger: String,
    reopen: String,
    fired: Cell<bool>,
    sent: RefCell<Vec<String>>,
}

impl Transport {
    fn sent_to(&self, recipient: &Recipient) -> usize {
        let key = recipient.key().unwrap();
        self.sent.borrow().iter().filter(|k| **k == key).count()
    }
}

impl DeliveryAdapter for Transport {
    fn harness(&self) -> &'static str {
        "fixture"
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            idempotent_retry: false,
            admission: false,
            non_waking: false,
            max_payload: MAX_PAYLOAD,
            send_budget: None,
            transcript_reader: false,
        }
    }
    fn readiness(&self, _: &Ctx, _: &RecipientRef) -> anyhow::Result<Readiness> {
        Ok(Readiness::Native)
    }
    fn bind(&self, _: &Ctx, recipient: &RecipientRef) -> anyhow::Result<BindOutcome> {
        Ok(BindOutcome::Bound(SessionBinding {
            session: recipient.key()?,
            endpoint: None,
            transcript: None,
        }))
    }
    fn send(&self, ctx: &Ctx, binding: &SessionBinding, _: &Envelope) -> SendOutcome {
        self.sent.borrow_mut().push(binding.session.clone());
        if binding.session == self.trigger && !self.fired.replace(true) {
            crate::request_changes(ctx, &self.reopen, "fix the findings").unwrap();
        }
        SendOutcome::Accepted
    }
    fn observe(
        &self,
        _: &Ctx,
        _: &SessionBinding,
        _: &mut ObserveCursor,
    ) -> anyhow::Result<Vec<Observation>> {
        Ok(Vec::new())
    }
}

fn run(fx: &Fx, ended: bool) -> corral_core::Run {
    // Bound to the fake engine's live pane with this client's socket: an owned, not legacy, run.
    let socket = fx.ctx.herdr.socket_path().unwrap();
    let mut run: corral_core::Run = serde_json::from_value(json!({
        "id": "R1", "agent": "fixture", "worktree": "/missing-fixture-worktree",
        "branch": "corral/T", "base": "main", "base_sha": "abc", "started": at(0),
        "delivery": "inbox",
        "herdr": {"workspace_id": "worker", "pane_id": "worker:p1", "socket": socket}
    }))
    .unwrap();
    if ended {
        run.outcome = Some(corral_core::RunOutcome::Done);
        run.ended = Some(at(0));
    }
    run
}

struct World {
    fx: Fx,
    live: Recipient,
    reopened: Recipient,
    transport: Transport,
}

fn world() -> World {
    let fx = Fx::new();
    let store = &fx.ctx.store;
    let live = store.create("Live", None, Status::Running, "").unwrap();
    store
        .update(&live.id, |task| {
            task.runs.push(run(&fx, false));
            Ok(())
        })
        .unwrap();
    let ended = store.create("T-128", None, Status::Review, "").unwrap();
    store
        .update(&ended.id, |task| {
            task.runs.push(run(&fx, true));
            Ok(())
        })
        .unwrap();
    let recipient = |task: &str| Recipient::Run {
        task: task.into(),
        run: "R1".into(),
    };
    let (live, reopened) = (recipient(&live.id), recipient(&ended.id));
    Inbox::new(store, live.clone())
        .unwrap()
        .post(Post {
            kind: "owner_reply",
            urgency: Urgency::Wake,
            author: "owner",
            body: "continue",
            binding: MessageBinding::default(),
            subject: None,
            max_payload: MAX_PAYLOAD,
        })
        .unwrap();
    let transport = Transport {
        trigger: live.key().unwrap(),
        reopen: ended.id,
        fired: Cell::new(false),
        sent: RefCell::new(Vec::new()),
    };
    World {
        fx,
        live,
        reopened,
        transport,
    }
}

fn changes(world: &World) -> corral_core::inbox::Message {
    let messages = Inbox::new(&world.fx.ctx.store, world.reopened.clone())
        .unwrap()
        .messages()
        .unwrap();
    messages
        .into_iter()
        .find(|message| message.kind == "changes_requested")
        .unwrap()
}

fn window(end: i64) -> Window {
    Window {
        start: "2000-01-01T00:00:00Z".parse().unwrap(),
        end: at(end),
    }
}

#[test]
fn s12_fenced_pass_delivers_reopened_changes_and_the_unfenced_control_retries_its_recipient() {
    // Given T-128: one pass snapshots an ended run that a concurrent delivery reopens.
    let world = world();
    let ctx = world.fx.reopen();
    crate::delivery::owner::reconcile_all_with(&ctx, &world.transport).unwrap();
    assert!(
        world.transport.fired.get(),
        "the reopening ran inside the pass"
    );
    let posted = changes(&world);
    let inbox = Inbox::new(&ctx.store, world.reopened.clone()).unwrap();
    // Then the fenced retire cancelled nothing, and the next pass delivers to the reopened run.
    assert_eq!(inbox.terminal(posted.seq).unwrap(), None);
    assert_eq!(world.transport.sent_to(&world.reopened), 0);
    crate::delivery::owner::reconcile_all_with(&ctx, &world.transport).unwrap();
    assert_eq!(world.transport.sent_to(&world.reopened), 1);
    assert_eq!(world.transport.sent_to(&world.live), 1);
    crate::sync_at(&world.fx.reopen(), at(0)).unwrap();
    let fenced = stats(&world.fx.reopen(), window(0)).unwrap();
    assert!(
        fenced
            .conditions
            .values()
            .all(|row| row.kind != "delivery_cancelled"),
        "{:?}",
        fenced.conditions.keys().collect::<Vec<_>>()
    );

    // Given the same race, when the stale snapshot's retire is replayed without the fence.
    let control = self::world();
    let ctx = control.fx.reopen();
    crate::delivery::owner::reconcile_all_with(&ctx, &control.transport).unwrap();
    let stale = changes(&control);
    Inbox::new(&ctx.store, control.reopened.clone())
        .unwrap()
        .mark_terminal(
            stale.seq,
            TerminalState::Cancelled,
            "stale run-ended snapshot",
            "replay",
        )
        .unwrap();
    crate::sync_at(&control.fx.reopen(), at(0)).unwrap();
    // Then one pass shows a cancelled-never-attempted condition whose retry reaches the
    // original recipient, attributed to exactly that condition.
    let key = format!(
        "{}/delivery_cancelled/{}",
        stale.recipient.task().unwrap(),
        stale.id
    );
    let before = stats(&control.fx.reopen(), window(0)).unwrap();
    assert!(
        before.conditions.contains_key(&key),
        "{:?}",
        before.conditions.keys()
    );
    assert_eq!(control.transport.sent_to(&control.reopened), 0);
    crate::delivery::retry(&ctx.store, &control.reopened, stale.seq).unwrap();
    crate::delivery::owner::reconcile_all_with(&ctx, &control.transport).unwrap();
    assert_eq!(control.transport.sent_to(&control.reopened), 1);
    crate::sync_at(&control.fx.reopen(), at(60)).unwrap();
    let after = stats(&control.fx.reopen(), window(60)).unwrap();
    assert_eq!(after.actions.by_condition.get(&key), Some(&1));
    assert_eq!(after.actions.duplicates, 0);
}
