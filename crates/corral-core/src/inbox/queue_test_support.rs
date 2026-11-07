use super::*;
use crate::{Run, Status, Store};

pub fn fixture(kind: &str) -> (tempfile::TempDir, Store, Recipient) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    let recipient = match kind {
        "pm" => Recipient::Pm,
        "assistant" => Recipient::Assistant {
            session: "fixture".into(),
        },
        _ => {
            let task = store.create("Queue", None, Status::Running, "").unwrap();
            store
                .update(&task.id, |task| {
                    task.runs
                        .push(serde_json::from_value::<Run>(serde_json::json!({
                            "id":"R1", "agent":"fixture", "worktree":"/fixture", "branch":"fixture",
                            "base":"main", "base_sha":"abc", "started":"2026-10-25T00:00:00Z"
                        }))?);
                    Ok(())
                })
                .unwrap();
            Recipient::Run {
                task: task.id,
                run: "R1".into(),
            }
        }
    };
    (root, store, recipient)
}

pub fn post(body: &str) -> Post<'_> {
    Post {
        kind: "note",
        urgency: Urgency::Wake,
        author: "owner",
        body,
        binding: MessageBinding::default(),
        subject: None,
        max_payload: MAX_PAYLOAD,
    }
}

pub fn scope(inbox: &Inbox<'_>) -> ExecutionScope {
    let envelope = inbox
        .publish(
            &SessionBinding {
                session: "fixture".into(),
                endpoint: None,
                transcript: None,
            },
            "corral-pi",
            MAX_PAYLOAD,
            false,
        )
        .unwrap()
        .unwrap();
    ExecutionScope {
        envelope: envelope.number,
        generation: 3,
        binding: envelope.binding.clone(),
        bind: "a".repeat(64),
        idempotency_key: envelope.idempotency_key.clone(),
        text_sha256: envelope.payload_sha256.clone(),
        text_bytes: inbox.payload(&envelope).unwrap().len() as u64,
    }
}

pub fn queued(scope: &ExecutionScope, seq: u64) -> ExecutionRecord {
    ExecutionRecord {
        seq,
        scope: scope.clone(),
        item: "q-g3-aaaaaaaaaaaa-1".into(),
        state: ExecutionState::Queued,
        turn: None,
        op_key: None,
        actor: None,
        reason: None,
    }
}
