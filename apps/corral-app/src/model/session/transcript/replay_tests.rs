use corral_protocol::detail::{
    DetailBody, DetailRef, HostPresence, SessionItem, SessionMessage, SessionPage, SessionToolState,
};
use corral_protocol::subscription::{SessionAgentMessage, SessionFleet, TranscriptOp};

use super::super::Ui;
use super::replay::transcript;
use super::{
    ItemFetch, LiveSession, LiveTool, LiveToolState, MessageItem, More, Reading, Resume, Transcript,
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
fn live_tool_result_decorates_the_held_call_without_adding_a_row() {
    let mut feed = feed(vec![SessionItem::ToolCall {
        call_id: "c1".into(),
        name: "bash".into(),
        arguments: "{\"command\": \"ls\"}".into(),
        summary: "ls".into(),
        state: SessionToolState::Running,
        paired: true,
        call: None,
        output: None,
        result: None,
        elapsed_ms: None,
        diff: None,
        content: None,
    }]);
    feed.capabilities
        .insert(corral_protocol::app::SESSION_TRANSCRIPT_V1.into());
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
    let [MessageItem::Tool(tool)] = page.messages[0].items.as_slice() else {
        panic!("one tool row")
    };
    assert_eq!(tool.state, super::ToolState::Ok);
    assert!(tool.time.is_some());
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
fn a_streamed_tool_result_settles_only_its_call_without_a_row() {
    use super::ToolState;

    let call = |id: &str, name: &str, arguments: &str, state| SessionItem::ToolCall {
        call_id: id.into(),
        name: name.into(),
        arguments: arguments.into(),
        summary: String::new(),
        state,
        paired: false,
        call: None,
        output: None,
        result: None,
        elapsed_ms: None,
        diff: None,
        content: None,
    };
    for (state, expected, exit_code) in [
        (SessionToolState::Done, ToolState::Ok, 0),
        (SessionToolState::Failed, ToolState::Failed, 1),
    ] {
        let id = "finished";
        let mut feed = feed(vec![
            call(
                id,
                "bash",
                r#"{"command":"echo ready"}"#,
                SessionToolState::Running,
            ),
            call(
                "still-running",
                "job_wait",
                r#"{"job":"j1-abc"}"#,
                SessionToolState::Running,
            ),
        ]);
        let mut result = call(id, "bash", "", state);
        if let SessionItem::ToolCall { output, result, .. } = &mut result {
            *output = Some("ready".into());
            *result = Some(
                serde_json::json!({"details":{"job":"j1-abc","state":"exited","exit_code":exit_code}})
                    .to_string(),
            );
        }
        let target = DetailRef::Session { id: ID.into() };
        let cache = feed.details.get_mut(&daemon::detail_key(&target)).unwrap();
        let Some(DetailBody::Session(page)) = cache.merged.as_mut() else {
            panic!("session page")
        };
        page.messages[0].id = "calls".into();
        let result = SessionMessage {
            id: "result".into(),
            author: "toolResult".into(),
            at: Some("2026-11-15T10:00:01Z".into()),
            items: vec![result],
            ..page.messages[0].clone()
        };
        super::mirror::apply(
            cache,
            vec![TranscriptOp::Put {
                message: result,
                before: None,
                replaces: None,
                open: false,
                cut: false,
            }],
            false,
        );
        let page = shown(&feed);
        let [message] = page.messages.as_slice() else {
            panic!("the paired result should have no bubble of its own")
        };
        let [MessageItem::Tool(done), MessageItem::Tool(waiting)] = message.items.as_slice() else {
            panic!("one card per call")
        };
        assert_eq!(done.state, expected);
        assert_eq!(done.outcome_hue == Hue::Fail, exit_code != 0);
        assert_eq!(waiting.state, ToolState::Running);
        assert!(waiting.summary.contains("echo ready"));
    }
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

/// T-2119: a page that starts partway through an assistant record holds only its tail, each
/// block of it whole. Once `history_changed` hides the way to the page before, Copy reply on
/// that tail must still say it is partial; a record the window paged the end out of too. A
/// record wholly on the page copies whole, also when the cut record took over the anchor of the
/// open reply it replaced.
#[test]
fn a_split_record_copies_as_partial_after_history_changed_hides_older_pages() {
    let text = |words: &str| SessionItem::Text { text: words.into() };
    let mut feed = feed(vec![
        text("The tail's first block."),
        text("Its last block."),
    ]);
    let target = DetailRef::Session { id: ID.into() };
    let cache = feed
        .details
        .get_mut(&daemon::detail_key(&target))
        .expect("the fixture's cache");
    let Some(DetailBody::Session(page)) = cache.merged.as_mut() else {
        panic!("a served session page");
    };
    page.older = Some("before".into());
    page.messages[0].id = "m1".into();
    let mut whole = page.messages[0].clone();
    whole.id = "m3".into();
    whole.at = Some("2026-11-15T10:00:09Z".into());
    whole.items = vec![text("A reply wholly on this page.")];
    let mut asked = whole.clone();
    asked.id = "m2".into();
    asked.author = "user".into();
    asked.at = Some("2026-11-15T10:00:05Z".into());
    asked.items = vec![text("Go on.")];
    page.messages.extend([asked, whole]);
    cache.error = Some(daemon::ClientError {
        code: "history_changed".into(),
        message: "history changed".into(),
    });
    let partial = |feed: &daemon::Feed| {
        let shown = shown(feed);
        assert!(shown.older.is_none() && shown.older_cursor.is_none());
        let replies: Vec<_> = shown
            .messages
            .iter()
            .filter_map(|m| shown.reply_of(m))
            .map(|reply| reply.partial)
            .collect();
        replies
    };
    assert_eq!(partial(&feed), [true, false]);
    let seams = &mut feed
        .details
        .get_mut(&daemon::detail_key(&target))
        .expect("the fixture's cache")
        .transcript_seams;
    // The cut record replaced an open streamed reply, whose anchor it keeps.
    seams.mirror.identity.insert("m1".into(), "stream-1".into());
    seams.suffix = Some(("m3".into(), 1));
    assert_eq!(partial(&feed), [true, true]);
}
