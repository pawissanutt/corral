use corral_protocol::detail::{
    DetailBody, DetailRef, HostPresence, SessionItem, SessionMessage, SessionPage, SessionToolState,
};
use corral_protocol::subscription::{SessionAgentMessage, SessionFleet};

use super::super::Ui;
use super::replay::transcript;
use super::{
    ItemFetch, LiveMessage, LiveSession, LiveTool, LiveToolState, MessageItem, More, Reading,
    Resume, Transcript,
};
use crate::model::Id;
use crate::model::daemon::{self, DetailCache};
use crate::model::hue::Hue;

const ID: &str = "T-1/run/R1";

fn feed(items: Vec<SessionItem>) -> daemon::Feed {
    feed_for(ID, items)
}

fn feed_for(id: &str, items: Vec<SessionItem>) -> daemon::Feed {
    let mut feed = daemon::Feed::default();
    let target = DetailRef::Session { id: id.into() };
    feed.details.insert(
        daemon::detail_key(&target),
        DetailCache {
            target,
            pages: Vec::new(),
            transcript_seams: Default::default(),
            loading: false,
            stale: false,
            error: None,
            merged: Some(DetailBody::Session(SessionPage {
                id: id.into(),
                role: "worker".into(),
                task: None,
                run: None,
                machine: None,
                messages: vec![SessionMessage {
                    id: String::new(),
                    author: "assistant".into(),
                    at: Some("2026-11-15T10:00:00Z".into()),
                    items,
                    model: None,
                    stop_reason: None,
                    usage: None,
                    input: None,
                    attachments: Vec::new(),
                }],
                older: None,
                replyable: false,
                reply_refusal: None,
                host: HostPresence::NotRunning,
            })),
        },
    );
    feed
}

fn shown(feed: &daemon::Feed) -> Transcript {
    let target = DetailRef::Session { id: ID.into() };
    transcript(&Id::from(ID), &Ui::default(), feed.detail(&target), feed)
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
                after: 0,
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
    use super::ToolState;

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
    for (state, expected) in [
        (SessionToolState::Done, ToolState::Ok),
        (SessionToolState::Failed, ToolState::Failed),
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
            id: String::new(),
            author: "tool".into(),
            at: Some("2026-11-15T10:00:01Z".into()),
            items: vec![result],
            model: None,
            stop_reason: None,
            usage: None,
            input: None,
            attachments: Vec::new(),
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
        assert_eq!(tools[0].state, expected);
        assert_eq!(
            tools[0].outcome_hue == Hue::Fail,
            state == SessionToolState::Failed
        );
        assert_eq!(tools[0].summary, "echo ready");
        assert_eq!(
            tools[1].state,
            ToolState::Running,
            "a different id must stay running"
        );
    }
}

#[test]
fn a_recorded_reply_retires_its_live_text() {
    // The feed's recorded assistant message was written at 10:00:00, after the turn started.
    let mut feed = feed(vec![SessionItem::Text {
        text: "Done: the tests pass.".into(),
    }]);
    feed.live.insert(
        Id::from(ID),
        LiveSession {
            open: false,
            turn: Some(3),
            started_at: Some("2026-11-15T09:59:58Z".into()),
            messages: vec![LiveMessage {
                ordinal: 1,
                text: "Done: the tests pass.".into(),
                thinking: String::new(),
                settled_at: None,
            }],
            ..Default::default()
        },
    );
    let page = shown(&feed);
    assert!(
        page.live.is_empty(),
        "the served reply of the turn must retire its live text, not stand beside it"
    );
}

#[test]
fn a_live_reply_that_follows_two_tool_calls_is_shown_after_them() {
    // The agent edited, committed, then said so; the turn ended before the page held any of it.
    let call = |id: &str, tool: &str, args: &str| LiveTool {
        call: id.into(),
        tool: tool.into(),
        args: Some(args.into()),
        args_truncated: false,
        partial: None,
        state: LiveToolState::Done,
        elapsed_ms: Some(300),
        settled_at: Some(1),
        after: 0,
    };
    let mut feed = feed(Vec::new());
    feed.live.insert(
        Id::from(ID),
        LiveSession {
            open: false,
            turn: Some(4),
            tools: vec![
                call("e1", "edit", r#"{"path":"line.txt"}"#),
                call(
                    "b1",
                    "bash",
                    r#"{"command":"git add line.txt && git commit -m label"}"#,
                ),
            ],
            messages: vec![LiveMessage {
                ordinal: 2,
                text: "The parity label is committed.".into(),
                thinking: String::new(),
                settled_at: Some(2),
            }],
            ..Default::default()
        },
    );
    let page = shown(&feed);
    let [live] = page.live.as_slice() else {
        panic!("the live turn is one message");
    };
    let kinds: Vec<&str> = live
        .items
        .iter()
        .map(|item| match item {
            MessageItem::Tool(_) => "tool",
            MessageItem::Prose(_) => "text",
            _ => "other",
        })
        .collect();
    assert_eq!(kinds, ["tool", "tool", "text"]);
}

/// The words being written were keyed by where they stood in the live turn, so their view was
/// made again each time a call before them was recorded.
#[test]
fn live_words_keep_their_key_when_a_call_before_them_is_recorded() {
    let turn = |tools: Vec<LiveTool>| LiveSession {
        open: true,
        turn: Some(4),
        tools,
        messages: vec![LiveMessage {
            ordinal: 2,
            text: "Committing the label".into(),
            thinking: String::new(),
            settled_at: None,
        }],
        ..Default::default()
    };
    let words = |tools| {
        let mut feed = feed(Vec::new());
        feed.live.insert(Id::from(ID), turn(tools));
        let page = shown(&feed);
        let live = &page.live[0];
        assert_eq!(live.keys.len(), live.items.len());
        let at = live
            .items
            .iter()
            .position(|item| matches!(item, MessageItem::Prose(_)))
            .unwrap();
        (at, live.keys[at].clone())
    };
    let (after_call, key) = words(vec![LiveTool {
        call: "e1".into(),
        tool: "edit".into(),
        args: None,
        args_truncated: false,
        partial: None,
        state: LiveToolState::Running,
        elapsed_ms: None,
        settled_at: None,
        after: 0,
    }]);
    assert_eq!((after_call, words(Vec::new())), (1, (0, key)));
}

/// Appends a message to the served page.
fn arrive(feed: &mut daemon::Feed, target: &DetailRef, author: &str, at: &str, text: &str) {
    let Some(DetailBody::Session(page)) = feed
        .details
        .get_mut(&daemon::detail_key(target))
        .and_then(|cache| cache.merged.as_mut())
    else {
        panic!("a served session page");
    };
    page.messages.push(SessionMessage {
        id: String::new(),
        author: author.into(),
        at: Some(at.into()),
        items: vec![SessionItem::Text { text: text.into() }],
        model: None,
        stop_reason: None,
        usage: None,
        input: None,
        attachments: Vec::new(),
    });
}

/// A record split across pages keeps its message and anchor when its earlier items arrive from
/// the page before, so a view watching the first message's anchor neither held the reader's row
/// nor told those items from new replies. A reply merged into the same message below must not
/// look like rows above, and earlier synthetic child mail must not hide recorded prepends.
#[test]
fn a_split_record_gaining_its_earlier_items_changes_the_first_row() {
    let id = Id::from("B-test/child/worker");
    let target = DetailRef::Session { id: id.to_string() };
    let text = |words: &str| SessionItem::Text { text: words.into() };
    let mut feed = feed_for(id.as_str(), vec![text("The record's end.")]);
    feed.live.insert(
        Id::from("B-test"),
        LiveSession {
            fleet: Some(SessionFleet {
                children: Vec::new(),
                messages: vec![SessionAgentMessage {
                    seq: 1,
                    at: super::replay::millis(Some("2026-11-15T09:59:00Z")).unwrap(),
                    from: "parent".into(),
                    to: "worker".into(),
                    text: "A message sent before the recorded reply.".into(),
                    delivery: "delivered".into(),
                }],
                messages_total: 1,
                max_parallel: 1,
            }),
            ..Default::default()
        },
    );
    let child = |feed: &daemon::Feed| transcript(&id, &Ui::default(), feed.detail(&target), feed);
    let before = child(&feed);
    assert_eq!(before.messages[0].id, format!("{id}/msg/1"));
    arrive(
        &mut feed,
        &target,
        "assistant",
        "2026-11-15T10:00:09Z",
        "A later reply.",
    );
    let below = child(&feed);
    assert_eq!(below.messages.len(), 2, "the reply joins the same message");
    assert_eq!(below.first, before.first, "rows below are not rows above");
    let Some(DetailBody::Session(page)) = feed
        .details
        .get_mut(&daemon::detail_key(&target))
        .and_then(|cache| cache.merged.as_mut())
    else {
        panic!("a served session page");
    };
    page.messages[0]
        .items
        .insert(0, text("The record's start, from the page before."));
    let above = child(&feed);
    assert_eq!(above.messages[0].anchor, before.messages[0].anchor);
    assert_eq!(above.messages[1].anchor, before.messages[1].anchor);
    assert_ne!(
        above.first, below.first,
        "the record's earlier items arrived above the shown rows"
    );
}

#[test]
fn a_reader_who_scrolled_up_finds_the_same_row_after_leaving_and_reopening_the_session() {
    let target = DetailRef::Session { id: ID.into() };
    let mut feed = feed(vec![SessionItem::Text {
        text: "The first reply.".into(),
    }]);
    arrive(&mut feed, &target, "user", "2026-11-15T10:00:05Z", "Go on.");
    let mut ui = Ui::default();
    let first = transcript(&Id::from(ID), &ui, feed.detail(&target), &feed);
    assert_eq!(
        first.following().resume,
        Resume::End,
        "a session never read opens at its end"
    );
    // The reader scrolled up to the first reply and left; more arrived while they were away.
    let key = first.messages[0].anchor.clone();
    ui.reading = Reading::At {
        key: key.clone(),
        offset: 12,
        rows: first.rows(),
    };
    arrive(
        &mut feed,
        &target,
        "assistant",
        "2026-11-15T10:00:09Z",
        "The second reply.",
    );
    let reopened = transcript(&Id::from(ID), &ui, feed.detail(&target), &feed);
    let following = reopened.following();
    assert_eq!(
        following.resume,
        Resume::Row {
            key,
            offset: 12,
            rows: first.rows(),
        },
        "the reopened view starts at the row the reader left"
    );
    assert!(
        following.rows > first.rows(),
        "what arrived while they were away is counted from where they left"
    );
}
