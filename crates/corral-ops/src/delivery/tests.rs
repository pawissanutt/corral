use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::process::{Command, Stdio};

use corral_core::{Store, inbox::*};
use tempfile::TempDir;

use super::*;

mod recovery;
mod retired;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Sent {
    key: String,
    payload: String,
    binding: SessionBinding,
}

struct Fake {
    name: &'static str,
    caps: Capabilities,
    readiness: RefCell<Readiness>,
    binding: RefCell<BindOutcome>,
    outcomes: RefCell<VecDeque<SendOutcome>>,
    observations: RefCell<Vec<Observation>>,
    reads: RefCell<Vec<(SessionBinding, ObserveCursor)>>,
    fail_observation: Cell<bool>,
    sent: RefCell<Vec<Sent>>,
}

impl Fake {
    fn new(outcome: SendOutcome) -> Self {
        Self {
            name: "fake",
            caps: Capabilities {
                idempotent_retry: false,
                admission: true,
                non_waking: false,
                max_payload: MAX_PAYLOAD,
                send_budget: None,
                transcript_reader: true,
            },
            readiness: RefCell::new(Readiness::Native),
            binding: RefCell::new(BindOutcome::Bound(binding("session-1"))),
            outcomes: RefCell::new(VecDeque::from([outcome])),
            observations: RefCell::new(Vec::new()),
            reads: RefCell::new(Vec::new()),
            fail_observation: Cell::new(false),
            sent: RefCell::new(Vec::new()),
        }
    }
}

impl DeliveryAdapter for Fake {
    fn harness(&self) -> &'static str {
        self.name
    }
    fn capabilities(&self) -> Capabilities {
        self.caps.clone()
    }
    fn readiness(&self, _: &Ctx, _: &RecipientRef) -> Result<Readiness> {
        Ok(self.readiness.borrow().clone())
    }
    fn bind(&self, _: &Ctx, _: &RecipientRef) -> Result<BindOutcome> {
        Ok(self.binding.borrow().clone())
    }
    fn send(&self, ctx: &Ctx, session: &SessionBinding, envelope: &Envelope) -> SendOutcome {
        // Prove dispatching is on disk BEFORE the adapter side effect.
        let inbox = Inbox::new(&ctx.store, Recipient::Pm).unwrap();
        let persisted = inbox.envelope(envelope.number).unwrap();
        assert_eq!(persisted.stages.last(), Some(&Stage::Dispatching));
        assert_eq!(persisted.attempts.last().unwrap().outcome, None);
        self.sent.borrow_mut().push(Sent {
            key: envelope.idempotency_key.clone(),
            payload: inbox.payload(envelope).unwrap(),
            binding: session.clone(),
        });
        self.outcomes
            .borrow_mut()
            .pop_front()
            .unwrap_or(SendOutcome::Accepted)
    }
    fn observe(
        &self,
        ctx: &Ctx,
        session: &SessionBinding,
        cursor: &mut ObserveCursor,
    ) -> Result<Vec<Observation>> {
        self.reads
            .borrow_mut()
            .push((session.clone(), cursor.clone()));
        ensure!(!self.fail_observation.get(), "fixture reader unavailable");
        cursor.last_complete_line += 1;
        let inbox = Inbox::new(&ctx.store, Recipient::Pm)?;
        let mut observed = Vec::new();
        let queued = std::mem::take(&mut *self.observations.borrow_mut());
        for observation in queued {
            let number = match &observation {
                Observation::Delivered { envelope, .. }
                | Observation::Admitted { envelope, .. }
                | Observation::Uncertain { envelope, .. } => *envelope,
            };
            if inbox.envelope(number)?.binding == *session {
                observed.push(observation);
            } else {
                self.observations.borrow_mut().push(observation);
            }
        }
        Ok(observed)
    }
}

fn binding(id: &str) -> SessionBinding {
    SessionBinding {
        session: id.into(),
        endpoint: None,
        transcript: None,
    }
}
fn fixture() -> (TempDir, Ctx) {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Ctx::from_store(Store::init(dir.path()).unwrap());
    (dir, ctx)
}
fn post(ctx: &Ctx, body: &str) -> Message {
    Inbox::new(&ctx.store, Recipient::Pm)
        .unwrap()
        .post(Post {
            kind: "owner_note",
            urgency: Urgency::Wake,
            author: "owner",
            body,
            binding: MessageBinding::default(),
            subject: None,
            max_payload: MAX_PAYLOAD,
        })
        .unwrap()
}
fn tick(ctx: &Ctx, fake: &Fake) -> DeliveryStatus {
    deliver(ctx, &Recipient::Pm, fake, None).unwrap()
}

#[test]
fn admitted_accepted_and_held_wait_without_resending() {
    for outcome in [
        SendOutcome::Admitted {
            detail: json!({"custody":"yes"}),
        },
        SendOutcome::Accepted,
        SendOutcome::Held {
            reason: "offline".into(),
        },
    ] {
        let (_dir, ctx) = fixture();
        post(&ctx, "first");
        let fake = Fake::new(outcome);
        assert_eq!(tick(&ctx, &fake), DeliveryStatus::Sent { envelope: 1 });
        post(&ctx, "second");
        assert_eq!(tick(&ctx, &fake), DeliveryStatus::Waiting);
        assert_eq!(fake.sent.borrow().len(), 1);
        let inbox = Inbox::new(&ctx.store, Recipient::Pm).unwrap();
        assert!(inbox.receipt(1, ReceiptLevel::Observed).unwrap().is_none());
        assert_eq!(inbox.envelope(1).unwrap().members, vec![1]);
    }
}

#[test]
fn rejection_fails_members_and_allows_later_messages_and_owner_retry() {
    let (_dir, ctx) = fixture();
    let first = post(&ctx, "first");
    let fake = Fake::new(SendOutcome::Rejected {
        reason: "policy".into(),
        retryable_after_fix: true,
    });
    tick(&ctx, &fake);
    let inbox = Inbox::new(&ctx.store, Recipient::Pm).unwrap();
    assert_eq!(
        inbox.terminal(first.seq).unwrap(),
        Some(TerminalState::Failed)
    );
    assert!(inbox.state().unwrap().open_envelope.is_none());
    let repost = inbox.retry(first.seq).unwrap();
    assert_ne!(repost.id, first.id);
    assert_eq!(tick(&ctx, &fake), DeliveryStatus::Sent { envelope: 2 });
    assert_eq!(fake.sent.borrow().len(), 2);
}

#[test]
fn uncertain_non_idempotent_stops_and_surfaces_until_late_observation() {
    let (_dir, ctx) = fixture();
    post(&ctx, "first");
    let fake = Fake::new(SendOutcome::Uncertain {
        reason: "socket lost".into(),
    });
    tick(&ctx, &fake);
    assert_eq!(tick(&ctx, &fake), DeliveryStatus::Uncertain);
    assert_eq!(fake.sent.borrow().len(), 1);
    assert!(
        ctx.store
            .events()
            .unwrap()
            .iter()
            .any(|e| e.kind == "delivery_uncertain")
    );
    fake.observations.borrow_mut().push(Observation::Delivered {
        envelope: 1,
        members: vec![1],
        detail: json!({"record":2}),
    });
    assert_eq!(tick(&ctx, &fake), DeliveryStatus::Idle);
    assert!(
        Inbox::new(&ctx.store, Recipient::Pm)
            .unwrap()
            .receipt(1, ReceiptLevel::Observed)
            .unwrap()
            .is_some()
    );
}

#[test]
fn idempotent_uncertainty_retries_identical_bytes_key_and_session() {
    let (_dir, ctx) = fixture();
    post(&ctx, "first");
    let mut fake = Fake::new(SendOutcome::Uncertain {
        reason: "lost".into(),
    });
    fake.caps.idempotent_retry = true;
    tick(&ctx, &fake);
    post(&ctx, "must not be repacked");
    tick(&ctx, &fake);
    let sent = fake.sent.borrow();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0], sent[1]);
    assert_eq!(
        Inbox::new(&ctx.store, Recipient::Pm)
            .unwrap()
            .envelope(1)
            .unwrap()
            .members,
        vec![1]
    );
}

#[test]
fn fallback_only_before_publication_when_native_unavailable_and_reader_exists() {
    let (_dir, ctx) = fixture();
    post(&ctx, "first");
    let fake = Fake::new(SendOutcome::Accepted);
    let mut fallback = Fake::new(SendOutcome::Accepted);
    fallback.name = "terminal";
    *fake.readiness.borrow_mut() = Readiness::Unavailable {
        reason: "not installed".into(),
    };
    assert_eq!(
        deliver(&ctx, &Recipient::Pm, &fake, Some(&fallback)).unwrap(),
        DeliveryStatus::Sent { envelope: 1 }
    );
    assert!(fake.sent.borrow().is_empty());
    assert_eq!(fallback.sent.borrow().len(), 1);
}

#[test]
fn blocked_native_or_missing_reader_never_uses_fallback() {
    for readiness in [
        Readiness::Blocked {
            reason: BlockReason::PolicyRefused,
        },
        Readiness::Blocked {
            reason: BlockReason::InboundHold,
        },
        Readiness::Blocked {
            reason: BlockReason::ProtocolUnverified,
        },
        Readiness::Unavailable {
            reason: "missing".into(),
        },
    ] {
        let (_dir, ctx) = fixture();
        post(&ctx, "first");
        let mut fake = Fake::new(SendOutcome::Accepted);
        fake.caps.transcript_reader = false;
        *fake.readiness.borrow_mut() = readiness;
        let mut fallback = Fake::new(SendOutcome::Accepted);
        fallback.name = "terminal";
        assert!(matches!(
            deliver(&ctx, &Recipient::Pm, &fake, Some(&fallback)).unwrap(),
            DeliveryStatus::Blocked { .. }
        ));
        assert!(fallback.sent.borrow().is_empty());
        assert!(
            Inbox::new(&ctx.store, Recipient::Pm)
                .unwrap()
                .envelopes()
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn published_uncertain_never_falls_back_when_native_disappears() {
    let (_dir, ctx) = fixture();
    post(&ctx, "first");
    let mut fake = Fake::new(SendOutcome::Uncertain {
        reason: "lost".into(),
    });
    fake.caps.idempotent_retry = true;
    tick(&ctx, &fake);
    *fake.readiness.borrow_mut() = Readiness::Unavailable {
        reason: "gone".into(),
    };
    let mut fallback = Fake::new(SendOutcome::Accepted);
    fallback.name = "terminal";
    assert!(matches!(
        deliver(&ctx, &Recipient::Pm, &fake, Some(&fallback)).unwrap(),
        DeliveryStatus::Blocked { .. }
    ));
    assert!(fallback.sent.borrow().is_empty());
    assert_eq!(fake.sent.borrow().len(), 1);
}

#[test]
fn replacement_orphans_every_attempted_unobserved_state() {
    for outcome in [
        SendOutcome::Admitted { detail: json!({}) },
        SendOutcome::Accepted,
        SendOutcome::Held {
            reason: "offline".into(),
        },
        SendOutcome::Uncertain {
            reason: "lost".into(),
        },
    ] {
        let (_dir, ctx) = fixture();
        post(&ctx, "first");
        let mut fake = Fake::new(outcome.clone());
        fake.caps.idempotent_retry = true;
        tick(&ctx, &fake);
        *fake.binding.borrow_mut() = BindOutcome::Bound(binding("session-2"));
        assert_eq!(tick(&ctx, &fake), DeliveryStatus::Orphaned);
        let inbox = Inbox::new(&ctx.store, Recipient::Pm).unwrap();
        assert_eq!(inbox.envelope(1).unwrap().outcome, Some(outcome));
        assert_eq!(fake.sent.borrow().len(), 1);
        inbox.recover(Recovery::Resend).unwrap();
        assert_eq!(tick(&ctx, &fake), DeliveryStatus::Sent { envelope: 2 });
        assert_ne!(fake.sent.borrow()[0].key, fake.sent.borrow()[1].key);
        assert_eq!(fake.sent.borrow()[1].binding, binding("session-2"));
    }
}

#[test]
fn unbound_or_ambiguous_binding_blocks_without_a_send() {
    for binding in [
        BindOutcome::Unbound {
            reason: "none".into(),
        },
        BindOutcome::Ambiguous {
            candidates: vec!["a".into(), "b".into()],
        },
    ] {
        let (_dir, ctx) = fixture();
        post(&ctx, "first");
        let fake = Fake::new(SendOutcome::Accepted);
        *fake.binding.borrow_mut() = binding;
        assert!(matches!(tick(&ctx, &fake), DeliveryStatus::Blocked { .. }));
        assert!(fake.sent.borrow().is_empty());
    }
}

#[test]
fn late_admission_then_uncertainty_updates_open_envelope() {
    let (_dir, ctx) = fixture();
    post(&ctx, "first");
    let fake = Fake::new(SendOutcome::Accepted);
    tick(&ctx, &fake);
    fake.observations.borrow_mut().push(Observation::Admitted {
        envelope: 1,
        detail: json!({"claim":1}),
    });
    assert_eq!(tick(&ctx, &fake), DeliveryStatus::Waiting);
    let inbox = Inbox::new(&ctx.store, Recipient::Pm).unwrap();
    assert!(inbox.receipt(1, ReceiptLevel::Admitted).unwrap().is_some());
    fake.observations.borrow_mut().push(Observation::Uncertain {
        envelope: 1,
        reason: "stale claim".into(),
    });
    assert_eq!(tick(&ctx, &fake), DeliveryStatus::Uncertain);
    assert_eq!(fake.sent.borrow().len(), 1);
}

#[test]
fn per_recipient_budget_defers_without_dropping_or_publishing_messages() {
    let (_dir, ctx) = fixture();
    let now: Timestamp = "2026-10-19T12:00:00Z".parse().unwrap();
    let mut fake = Fake::new(SendOutcome::Rejected {
        reason: "refused".into(),
        retryable_after_fix: true,
    });
    fake.caps.send_budget = Some(SendBudget {
        per_recipient_interval: Duration::from_secs(10),
        per_minute_total: 6,
    });
    post(&ctx, "first");
    deliver_at(&ctx, &Recipient::Pm, &fake, None, now).unwrap();
    post(&ctx, "second");
    assert_eq!(
        deliver_at(&ctx, &Recipient::Pm, &fake, None, now).unwrap(),
        DeliveryStatus::Deferred
    );
    assert_eq!(fake.sent.borrow().len(), 1);
    let later: Timestamp = "2026-10-19T12:00:10Z".parse().unwrap();
    assert_eq!(
        deliver_at(&ctx, &Recipient::Pm, &fake, None, later).unwrap(),
        DeliveryStatus::Sent { envelope: 2 }
    );
}

#[test]
fn total_budget_applies_across_recipients() {
    let (_dir, ctx) = fixture();
    let pm = Inbox::new(&ctx.store, Recipient::Pm).unwrap();
    let other = Inbox::new(
        &ctx.store,
        Recipient::Run {
            task: "T-001".into(),
            run: "R1".into(),
        },
    )
    .unwrap();
    let budget = SendBudget {
        per_recipient_interval: Duration::ZERO,
        per_minute_total: 1,
    };
    let now = "2026-10-19T12:00:00Z".parse().unwrap();
    assert!(budget::reserve(&ctx, &pm, "fake", Some(&budget), now).unwrap());
    assert!(!budget::reserve(&ctx, &other, "fake", Some(&budget), now).unwrap());
}

#[test]
fn crash_writer() {
    let Some(path) = std::env::var_os("CORRAL_INBOX_CRASH_FIXTURE") else {
        return;
    };
    let store = Store::open(std::path::Path::new(&path)).unwrap();
    let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
    let _deliver = inbox.delivery_lock().unwrap();
    let mut envelope = inbox
        .publish(&binding("session-1"), "fake", MAX_PAYLOAD, false)
        .unwrap()
        .unwrap();
    inbox
        .begin_attempt(&mut envelope, Timestamp::now())
        .unwrap();
    // Actual process exit: no outcome write and no Rust destructors.
    std::process::exit(23);
}

#[test]
fn process_death_after_dispatching_requires_idempotency_or_owner_decision() {
    for idempotent in [false, true] {
        let (dir, ctx) = fixture();
        post(&ctx, "first");
        let child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "delivery::tests::crash_writer"])
            .env("CORRAL_INBOX_CRASH_FIXTURE", dir.path())
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert_eq!(child.status.code(), Some(23), "{:?}", child);
        let mut fake = Fake::new(SendOutcome::Accepted);
        fake.caps.idempotent_retry = idempotent;
        let status = tick(&ctx, &fake);
        assert_eq!(
            status,
            if idempotent {
                DeliveryStatus::Sent { envelope: 1 }
            } else {
                DeliveryStatus::Uncertain
            }
        );
        assert_eq!(fake.sent.borrow().len(), usize::from(idempotent));
        let inbox = Inbox::new(&ctx.store, Recipient::Pm).unwrap();
        assert!(matches!(
            inbox.envelope(1).unwrap().attempts[0].outcome,
            Some(SendOutcome::Uncertain { .. })
        ));
        if !idempotent {
            inbox.recover(Recovery::Abandon).unwrap();
            assert_eq!(tick(&ctx, &fake), DeliveryStatus::Idle);
            assert_eq!(inbox.terminal(1).unwrap(), Some(TerminalState::Abandoned));
        }
    }
}

#[test]
fn incomplete_observation_cannot_create_any_receipt() {
    let (_dir, ctx) = fixture();
    post(&ctx, "first");
    post(&ctx, "second");
    let fake = Fake::new(SendOutcome::Accepted);
    tick(&ctx, &fake);
    fake.observations.borrow_mut().push(Observation::Delivered {
        envelope: 1,
        members: vec![1],
        detail: json!({}),
    });
    assert!(deliver(&ctx, &Recipient::Pm, &fake, None).is_err());
    assert!(
        Inbox::new(&ctx.store, Recipient::Pm)
            .unwrap()
            .receipt(1, ReceiptLevel::Observed)
            .unwrap()
            .is_none()
    );
}

#[test]
fn idle_pass_does_not_consume_send_budget() {
    let (_dir, ctx) = fixture();
    let now = "2026-10-19T12:00:00Z".parse().unwrap();
    let mut fake = Fake::new(SendOutcome::Accepted);
    fake.caps.send_budget = Some(SendBudget {
        per_recipient_interval: Duration::from_secs(10),
        per_minute_total: 1,
    });
    assert_eq!(
        deliver_at(&ctx, &Recipient::Pm, &fake, None, now).unwrap(),
        DeliveryStatus::Idle
    );
    post(&ctx, "first");
    assert_eq!(
        deliver_at(&ctx, &Recipient::Pm, &fake, None, now).unwrap(),
        DeliveryStatus::Sent { envelope: 1 }
    );
}

#[test]
fn late_custody_settles_an_interrupted_attempt_without_resend() {
    let (_dir, ctx) = fixture();
    post(&ctx, "first");
    let inbox = Inbox::new(&ctx.store, Recipient::Pm).unwrap();
    let mut envelope = inbox
        .publish(&binding("session-1"), "fake", MAX_PAYLOAD, false)
        .unwrap()
        .unwrap();
    inbox
        .begin_attempt(&mut envelope, Timestamp::now())
        .unwrap();
    let fake = Fake::new(SendOutcome::Accepted);
    fake.observations.borrow_mut().push(Observation::Admitted {
        envelope: 1,
        detail: json!({"claim":1}),
    });
    assert_eq!(tick(&ctx, &fake), DeliveryStatus::Waiting);
    assert!(fake.sent.borrow().is_empty());
    assert!(matches!(
        inbox.envelope(1).unwrap().outcome,
        Some(SendOutcome::Admitted { .. })
    ));
}

#[test]
fn unattempted_publication_rebinds_without_transferring_attempted_messages() {
    let (_dir, ctx) = fixture();
    post(&ctx, "first");
    let inbox = Inbox::new(&ctx.store, Recipient::Pm).unwrap();
    let old = inbox
        .publish(&binding("old"), "fake", MAX_PAYLOAD, false)
        .unwrap()
        .unwrap();
    let fake = Fake::new(SendOutcome::Accepted);
    assert_eq!(tick(&ctx, &fake), DeliveryStatus::Sent { envelope: 2 });
    assert_eq!(inbox.envelope(1).unwrap(), old);
    assert_eq!(fake.sent.borrow()[0].binding, binding("session-1"));
}

#[test]
fn recovered_rejection_is_failed_even_after_session_replacement() {
    let (_dir, ctx) = fixture();
    post(&ctx, "first");
    let inbox = Inbox::new(&ctx.store, Recipient::Pm).unwrap();
    let mut envelope = inbox
        .publish(&binding("old"), "fake", MAX_PAYLOAD, false)
        .unwrap()
        .unwrap();
    inbox
        .begin_attempt(&mut envelope, Timestamp::now())
        .unwrap();
    inbox
        .finish_attempt(
            &mut envelope,
            SendOutcome::Rejected {
                reason: "refused".into(),
                retryable_after_fix: true,
            },
        )
        .unwrap();
    let fake = Fake::new(SendOutcome::Accepted);
    assert_eq!(tick(&ctx, &fake), DeliveryStatus::Waiting);
    assert_eq!(inbox.terminal(1).unwrap(), Some(TerminalState::Failed));
    assert!(fake.sent.borrow().is_empty());
}

#[test]
fn settled_fallback_envelope_retires_without_its_former_adapter() {
    for terminal in [
        None,
        Some(TerminalState::Cancelled),
        Some(TerminalState::Failed),
        Some(TerminalState::Abandoned),
    ] {
        // Given a fallback envelope whose final member is settled durably by its owner.
        let (_dir, ctx) = fixture();
        let first = post(&ctx, "first");
        let native = Fake::new(SendOutcome::Accepted);
        *native.readiness.borrow_mut() = Readiness::Unavailable {
            reason: "missing".into(),
        };
        let mut fallback = Fake::new(SendOutcome::Accepted);
        fallback.name = "terminal";
        deliver(&ctx, &Recipient::Pm, &native, Some(&fallback)).unwrap();
        let inbox = Inbox::new(&ctx.store, Recipient::Pm).unwrap();
        let original = inbox.envelope(1).unwrap();
        match terminal {
            Some(state) => {
                inbox
                    .mark_terminal(first.seq, state, "owner settled", "manual")
                    .unwrap();
            }
            None => {
                inbox
                    .mark_observed(first.seq, "complete transcript checked")
                    .unwrap();
            }
        }
        post(&ctx, "second");
        *native.readiness.borrow_mut() = Readiness::Native;
        // When the next pass has the native adapter but no longer has the old fallback.
        let status = deliver(&ctx, &Recipient::Pm, &native, None).unwrap();
        // Then the settled envelope cannot block a fresh native publication.
        assert_eq!(status, DeliveryStatus::Sent { envelope: 2 });
        assert_eq!(native.sent.borrow().len(), 1);
        assert_eq!(inbox.envelope(1).unwrap(), original);
        assert_eq!(inbox.envelope(2).unwrap().members, vec![2]);
    }
}

#[test]
fn unobserved_fallback_still_requires_its_published_adapter() {
    let (_dir, ctx) = fixture();
    post(&ctx, "first");
    let native = Fake::new(SendOutcome::Accepted);
    *native.readiness.borrow_mut() = Readiness::Unavailable {
        reason: "missing".into(),
    };
    let mut fallback = Fake::new(SendOutcome::Accepted);
    fallback.name = "terminal";
    deliver(&ctx, &Recipient::Pm, &native, Some(&fallback)).unwrap();
    *native.readiness.borrow_mut() = Readiness::Native;
    assert!(matches!(
        deliver(&ctx, &Recipient::Pm, &native, None).unwrap(),
        DeliveryStatus::Blocked { .. }
    ));
    assert!(native.sent.borrow().is_empty());
    assert_eq!(
        Inbox::new(&ctx.store, Recipient::Pm)
            .unwrap()
            .state()
            .unwrap()
            .open_envelope,
        Some(1)
    );
}

#[test]
fn owner_resend_of_flushed_info_persists_send_eligibility() {
    // Given an info-only envelope sent by flush with an uncertain outcome.
    let (dir, ctx) = fixture();
    let inbox = Inbox::new(&ctx.store, Recipient::Pm).unwrap();
    let message = inbox
        .post(Post {
            kind: "main_movement",
            urgency: Urgency::Info,
            author: "owner",
            body: "main moved",
            binding: MessageBinding::default(),
            subject: None,
            max_payload: MAX_PAYLOAD,
        })
        .unwrap();
    inbox.request_flush().unwrap();
    let fake = Fake::new(SendOutcome::Uncertain {
        reason: "lost response".into(),
    });
    assert_eq!(tick(&ctx, &fake), DeliveryStatus::Sent { envelope: 1 });
    let original = inbox.envelope(1).unwrap();
    assert!(!inbox.flush_requested());
    // When the owner authorizes resend, and the delivery process later restarts.
    inbox.recover(Recovery::Resend).unwrap();
    let reopened = Ctx::from_store(Store::open(dir.path()).unwrap());
    let status = tick(&reopened, &fake);
    // Then recovery alone schedules the new send without altering original records.
    assert_eq!(status, DeliveryStatus::Sent { envelope: 2 });
    let inbox = Inbox::new(&reopened.store, Recipient::Pm).unwrap();
    assert_eq!(inbox.message(message.seq).unwrap(), message);
    assert_eq!(inbox.envelope(1).unwrap(), original);
    assert_eq!(inbox.envelope(2).unwrap().members, original.members);
    assert_ne!(
        inbox.envelope(2).unwrap().idempotency_key,
        original.idempotency_key
    );
    assert!(!inbox.flush_requested());
    assert_eq!(fake.sent.borrow().len(), 2);
}
