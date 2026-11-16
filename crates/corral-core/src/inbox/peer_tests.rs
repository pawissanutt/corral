use super::*;
use crate::Store;
use crate::inbox::{MessageBinding, Urgency};

#[test]
fn peer_quota_is_generation_scoped_atomic_and_replays_do_not_debit_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
    let post = |body: &'static str, max_payload| Post {
        kind: "agent_message",
        urgency: Urgency::Wake,
        author: "T-001_R1",
        body,
        binding: MessageBinding::default(),
        subject: None,
        max_payload,
    };
    let first = inbox
        .post_agent(post("context", 8192), 1, "rq-first", "hash")
        .unwrap();
    assert!(!first.owner_kind());
    assert_eq!(
        inbox
            .post_agent(post("context", 8192), 1, "rq-first", "hash")
            .unwrap()
            .id,
        first.id
    );
    for n in 1..AGENT_MESSAGES_PER_GENERATION {
        inbox
            .post_agent(post("context", 8192), 1, &format!("rq-{n}"), "hash")
            .unwrap();
    }
    let next = inbox.state_head().unwrap().next_seq;
    let refused = inbox
        .post_agent(post("context", 8192), 1, "rq-61", "hash")
        .unwrap_err();
    assert_eq!(
        refused.downcast_ref::<AgentPostRefusal>(),
        Some(&AgentPostRefusal::Limit)
    );
    assert_eq!(inbox.state_head().unwrap().next_seq, next);
    // Bounds run before both budget admission and sequence reservation.
    assert!(
        inbox
            .post_agent(post("oversize", 1), 2, "rq-large", "hash")
            .is_err()
    );
    assert_eq!(inbox.state_head().unwrap().next_seq, next);
    let fresh = inbox
        .post_agent(post("context", 8192), 2, "rq-new-generation", "hash")
        .unwrap();
    assert_eq!(fresh.seq, next);
}
