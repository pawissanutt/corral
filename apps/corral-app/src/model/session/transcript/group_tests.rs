use corral_protocol::detail::{
    DetailBody, DetailRef, HostPresence, SessionItem, SessionMessage, SessionPage, SessionToolState,
};

use super::super::Ui;
use super::replay::transcript;
use super::{Action, MessageItem, ToolState, Transcript};
use crate::model::Id;
use crate::model::daemon::{self, DetailCache};

pub(super) const ID: &str = "B-test";

/// A served session page holding `messages`.
pub(super) fn feed_with(messages: Vec<SessionMessage>) -> daemon::Feed {
    let mut feed = daemon::Feed::default();
    let target = DetailRef::Session { id: ID.into() };
    feed.details.insert(
        daemon::detail_key(&target),
        DetailCache {
            target,
            pages: Vec::new(),
            loading: false,
            stale: false,
            error: None,
            merged: Some(DetailBody::Session(SessionPage {
                id: ID.into(),
                role: "build".into(),
                task: None,
                run: None,
                machine: None,
                messages,
                older: None,
                replyable: true,
                reply_refusal: None,
                host: HostPresence::NotRunning,
            })),
        },
    );
    feed
}

/// A message by `author` at `at`.
pub(super) fn said(author: &str, at: &str, items: Vec<SessionItem>) -> SessionMessage {
    SessionMessage {
        id: String::new(),
        author: author.into(),
        at: Some(at.into()),
        items,
        model: None,
        stop_reason: None,
        usage: None,
        input: None,
        attachments: Vec::new(),
    }
}

/// A settled call of `name` with `arguments` and the result `output`.
pub(super) fn called(
    id: &str,
    name: &str,
    arguments: &str,
    state: SessionToolState,
    output: &str,
) -> SessionItem {
    SessionItem::ToolCall {
        call_id: id.into(),
        name: name.into(),
        arguments: arguments.into(),
        summary: String::new(),
        state,
        paired: true,
        call: None,
        output: Some(output.into()),
        result: None,
        elapsed_ms: Some(300),
        diff: None,
        content: None,
    }
}

pub(super) fn shown(feed: &daemon::Feed) -> Transcript {
    let target = DetailRef::Session { id: ID.into() };
    transcript(&Id::from(ID), &Ui::default(), feed.detail(&target), feed)
}

#[test]
fn a_failed_call_ends_a_group() {
    let read = |id: &str, path: &str, state| {
        let output = if state == SessionToolState::Failed {
            "ENOENT: no such file or directory"
        } else {
            "fn main() {}\n"
        };
        called(
            id,
            "read",
            &format!(r#"{{"path":"{path}"}}"#),
            state,
            output,
        )
    };
    // Each call in its own assistant message, as Pi records them: one reply to the reader.
    let feed = feed_with(
        [
            ("c1", "src/a.rs", SessionToolState::Done),
            ("c2", "src/b.rs", SessionToolState::Done),
            ("c3", "src/gone.rs", SessionToolState::Failed),
            ("c4", "src/c.rs", SessionToolState::Done),
            ("c5", "src/d.rs", SessionToolState::Done),
        ]
        .into_iter()
        .enumerate()
        .map(|(n, (id, path, state))| {
            said(
                "assistant",
                &format!("2026-11-15T10:00:0{n}Z"),
                vec![read(id, path, state)],
            )
        })
        .collect(),
    );
    let page = shown(&feed);
    assert_eq!(
        page.messages.len(),
        1,
        "one agent's calls read as one reply"
    );
    let items = &page.messages[0].items;
    let [
        MessageItem::Group(before),
        MessageItem::Tool(failed),
        MessageItem::Group(after),
    ] = items.as_slice()
    else {
        panic!("two reads, the failed read on its own row, two reads: {items:?}");
    };
    assert_eq!(failed.state, ToolState::Failed);
    assert_eq!(before.row.state, ToolState::Ok);
    assert_eq!(after.row.state, ToolState::Ok);
    assert!(!before.open && before.rows.is_empty());
}

#[test]
fn an_open_group_stays_open_when_a_split_record_gains_an_earlier_call() {
    let read = |id: &str| {
        called(
            id,
            "read",
            r#"{"path":"src/lib.rs"}"#,
            SessionToolState::Done,
            "fn main() {}\n",
        )
    };
    let mut feed = feed_with(vec![said(
        "assistant",
        "2026-11-15T10:00:00Z",
        vec![read("c2"), read("c3")],
    )]);
    let before = shown(&feed);
    let [MessageItem::Group(group)] = before.messages[0].items.as_slice() else {
        panic!("the two reads fold into one group");
    };
    let Some(Action::Expand { key, .. }) = &group.row.action else {
        panic!("the group can be opened");
    };
    let mut ui = Ui::default();
    ui.expanded.insert(key.clone());

    let target = DetailRef::Session { id: ID.into() };
    let Some(DetailBody::Session(page)) = feed
        .details
        .get_mut(&daemon::detail_key(&target))
        .and_then(|cache| cache.merged.as_mut())
    else {
        panic!("a served session page");
    };
    page.messages[0].items.insert(0, read("c1"));
    let after = transcript(&Id::from(ID), &ui, feed.detail(&target), &feed);
    let [MessageItem::Group(group)] = after.messages[0].items.as_slice() else {
        panic!("the earlier read joins the same group");
    };
    assert!(group.open);
    assert_eq!(group.rows.len(), 3);
    assert_eq!(&group.row.fold.key, key);
}
