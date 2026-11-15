use corral_protocol::detail::{
    DetailBody, DetailRef, SessionItem, SessionMessage, SessionPage, SessionToolState,
};

use super::replay::transcript;
use super::{MessageItem, More, State, Transcript};
use crate::model::Id;
use crate::model::daemon::{self, DetailCache};
use crate::model::hue::Hue;
use crate::model::session_feed::{ItemFetch, LiveSession, LiveTool, LiveToolState};

const ID: &str = "T-1/run/R1";

fn feed(items: Vec<SessionItem>) -> daemon::Feed {
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
                role: "worker".into(),
                task: None,
                run: None,
                machine: None,
                messages: vec![SessionMessage {
                    author: "assistant".into(),
                    at: Some("2026-11-15T10:00:00Z".into()),
                    items,
                    model: None,
                    stop_reason: None,
                    usage: None,
                }],
                older: None,
                replyable: false,
                reply_refusal: None,
            })),
        },
    );
    feed
}

fn shown(feed: &daemon::Feed) -> Transcript {
    let target = DetailRef::Session { id: ID.into() };
    transcript(&Id::from(ID), &State::default(), feed.detail(&target), feed)
}

#[test]
fn continued_text_shows_only_its_preview_after_the_continuation_arrives() {
    let full = "The complete reply, longer than its preview.".to_owned();
    let mut feed = feed(vec![
        SessionItem::Text {
            text: "The complete…".into(),
        },
        SessionItem::Continuation {
            token: "t1".into(),
            bytes: 70,
        },
    ]);
    feed.items.insert(
        "t1".into(),
        ItemFetch::Ready(SessionItem::Text { text: full.clone() }),
    );
    let page = shown(&feed);
    let [MessageItem::Prose(prose)] = page.messages[0].items.as_slice() else {
        panic!("one text item, the continuation folded into it");
    };
    assert_eq!(prose.markdown, full);
    assert_eq!(prose.more, More::Whole);
}

#[test]
fn live_tool_row_is_drawn_twice_once_the_recorded_transcript_holds_it() {
    let mut feed = feed(vec![SessionItem::ToolCall {
        call_id: "c1".into(),
        name: "bash".into(),
        arguments: "{\"command\": \"ls\"}".into(),
        summary: "ls".into(),
        state: SessionToolState::Done,
        paired: true,
        call: None,
        output: Some("a\nb".into()),
        result: None,
        elapsed_ms: Some(400),
        diff: None,
        content: None,
    }]);
    feed.live.insert(
        Id::from(ID),
        LiveSession {
            open: true,
            tools: vec![LiveTool {
                call: "c1".into(),
                tool: "bash".into(),
                args: Some("{\"command\": \"ls\"}".into()),
                args_truncated: false,
                partial: Some("a\nb".into()),
                state: LiveToolState::Done,
                elapsed_ms: Some(400),
                settled_at: None,
            }],
            ..Default::default()
        },
    );
    let page = shown(&feed);
    let rows = page
        .messages
        .iter()
        .chain(&page.live)
        .flat_map(|message| &message.items)
        .filter(|item| matches!(item, MessageItem::Tool(_)))
        .count();
    assert_eq!(rows, 1);
}

#[test]
fn provider_error_item_is_dropped() {
    let feed = feed(vec![SessionItem::ProviderError {
        message: "429 rate limited".into(),
        facts: None,
    }]);
    let page = shown(&feed);
    let [MessageItem::Marker(marker)] = page.messages[0].items.as_slice() else {
        panic!("the provider error is one marker row");
    };
    assert_eq!(marker.hue, Hue::Fail);
    assert!(marker.words.contains("429 rate limited"));
}

#[test]
fn recorded_tool_calls_settle_from_their_result_by_call_id() {
    use crate::model::board::Mark;

    let call = |id: &str, arguments: &str, state| SessionItem::ToolCall {
        call_id: id.into(),
        name: "corral_session".into(),
        arguments: arguments.into(),
        summary: arguments.into(),
        state,
        paired: false,
        call: None,
        output: None,
        result: None,
        elapsed_ms: None,
        diff: None,
        content: None,
    };
    for (state, mark) in [
        (SessionToolState::Done, Mark::Done),
        (SessionToolState::Failed, Mark::Fail),
    ] {
        let mut feed = feed(vec![
            call(
                "finished",
                r#"{"command":"echo ready\nmore"}"#,
                SessionToolState::Running,
            ),
            call(
                "still-running",
                r#"{"command":"wait"}"#,
                SessionToolState::Running,
            ),
        ]);
        let mut result = call("finished", "", state);
        if let SessionItem::ToolCall { output, result, .. } = &mut result {
            *output = Some("ready".into());
            *result = Some(r#"{"ok":true}"#.into());
        }
        let target = DetailRef::Session { id: ID.into() };
        let DetailBody::Session(page) = feed
            .details
            .get_mut(&daemon::detail_key(&target))
            .unwrap()
            .merged
            .as_mut()
            .unwrap()
        else {
            panic!("session page")
        };
        page.messages.push(SessionMessage {
            author: "tool".into(),
            at: Some("2026-11-15T10:00:01Z".into()),
            items: vec![result],
            model: None,
            stop_reason: None,
            usage: None,
        });
        let page = shown(&feed);
        let tools: Vec<_> = page
            .messages
            .iter()
            .flat_map(|message| &message.items)
            .filter_map(|item| match item {
                MessageItem::Tool(tool) => Some(tool),
                _ => None,
            })
            .collect();
        assert_eq!(
            tools.len(),
            2,
            "the result must settle the call, not add a second row"
        );
        assert_eq!(tools[0].mark, mark);
        assert_eq!(tools[0].failed, state == SessionToolState::Failed);
        assert_eq!(tools[0].summary, "echo ready");
        assert_eq!(
            tools[1].mark,
            Mark::Active,
            "a different id must stay running"
        );
    }
}
