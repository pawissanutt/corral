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
