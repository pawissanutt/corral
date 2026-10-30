//! Shared message/evidence assertions for recovery acceptance scenarios.
use super::*;

pub(in crate::agent::recovery) fn messages(
    fx: &Fx,
    recipient: &Recipient,
    kind: &str,
) -> Vec<Message> {
    let inbox = Inbox::new(&fx.ctx.store, recipient.clone()).unwrap();
    let all = inbox.messages().unwrap();
    all.into_iter().filter(|m| m.kind == kind).collect()
}

pub(in crate::agent::recovery) fn classes(calls: &Value) -> Vec<(String, String)> {
    let calls = calls.as_array().unwrap();
    let pair = |c: &Value| {
        (
            c["tool"].as_str().unwrap().into(),
            c["class"].as_str().unwrap().into(),
        )
    };
    calls.iter().map(pair).collect()
}
