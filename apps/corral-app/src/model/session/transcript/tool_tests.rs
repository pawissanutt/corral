use corral_protocol::detail::{
    DetailBody, DetailRef, DetailReply, SessionItem, SessionMessage, SessionPage, SessionToolState,
};

use super::super::Ui;
use super::group_tests::{ID, feed_with, said, shown};
use super::{
    BuildCard, ItemFetch, MessageItem, Reading, Resume, Section, Tool, ToolState, fetches,
};
use crate::model::Id;
use crate::model::daemon::{self, Feed};

fn tools(items: &[MessageItem]) -> Vec<&Tool> {
    items
        .iter()
        .filter_map(|item| match item {
            MessageItem::Tool(tool) => Some(tool),
            _ => None,
        })
        .collect()
}

#[test]
fn an_absent_exit_code_is_never_shown_as_zero() {
    // The owner's `!command` whose exit was not recorded, and a background job whose recorded
    // result carries no exit code.
    let owner = SessionItem::BashExecution {
        command: "cargo test".into(),
        output: "running 9 tests".into(),
        exit_code: None,
        cancelled: false,
        truncated: false,
        exclude_from_context: false,
        full_output_path: None,
    };
    let job = SessionItem::ToolCall {
        call_id: "b1".into(),
        name: "bash".into(),
        arguments: r#"{"command":"make all"}"#.into(),
        summary: String::new(),
        state: SessionToolState::Done,
        paired: true,
        call: None,
        output: Some("built".into()),
        result: Some(
            serde_json::json!({
                "role": "toolResult",
                "details": { "job": "j1-4", "state": "exited", "exit_code": null, "text": "built" }
            })
            .to_string(),
        ),
        elapsed_ms: None,
        diff: None,
        content: None,
    };
    let feed = feed_with(vec![
        said("bashExecution", "2026-11-15T10:00:00Z", vec![owner]),
        said("assistant", "2026-11-15T10:00:01Z", vec![job]),
    ]);
    let page = shown(&feed);
    let rows: Vec<&Tool> = page
        .messages
        .iter()
        .flat_map(|message| tools(&message.items))
        .collect();
    assert_eq!(rows.len(), 2);
    for row in &rows {
        assert!(
            !row.outcome.contains("exit 0"),
            "an unknown exit read as 0: {}",
            row.outcome
        );
    }
    assert_eq!(rows[0].state, ToolState::Unknown);
}

fn read(id: &str, output: Option<&str>) -> SessionItem {
    SessionItem::ToolCall {
        call_id: id.into(),
        name: "read".into(),
        arguments: if output.is_none() {
            r#"{"path":"src/first.rs"}"#.into()
        } else {
            String::new()
        },
        summary: String::new(),
        state: if output.is_some() {
            SessionToolState::Done
        } else {
            SessionToolState::Running
        },
        paired: false,
        call: output.is_none().then(|| r#"{"type":"toolCall"}"#.into()),
        output: output.map(str::to_owned),
        result: output
            .map(|text| serde_json::json!({"role": "toolResult", "text": text}).to_string()),
        elapsed_ms: None,
        diff: None,
        content: None,
    }
}

fn record(id: &str, author: &str, items: Vec<SessionItem>) -> SessionMessage {
    let mut message = said(author, "2026-11-15T10:00:00Z", items);
    message.id = id.into();
    message
}

fn reply(page: &SessionPage) -> DetailReply {
    DetailReply {
        etag: "snap".into(),
        as_of: String::new(),
        seq: 0,
        next: None,
        body: Some(DetailBody::Session(page.clone())),
    }
}

/// Serve a real newest-first cache chain, including the client-joined rows in `merged`.
fn tool_seam(split: bool) -> (Feed, Feed) {
    let target = DetailRef::Session { id: ID.into() };
    let text = || SessionItem::Text {
        text: "Between the call and its result.".into(),
    };
    let mut newer = vec![
        record(
            "result",
            "toolResult",
            vec![read("first", Some("first output"))],
        ),
        record(
            "next-result",
            "toolResult",
            vec![read("second", Some("second output"))],
        ),
    ];
    let mut older = vec![record("call", "assistant", vec![read("first", None)])];
    let mut joined = older.clone();
    if split {
        let continuation = |token: &str| SessionItem::Continuation {
            token: token.into(),
            bytes: 4096,
        };
        older[0].items.push(continuation("whole-call"));
        joined[0].items.push(continuation("whole-call"));
        joined[0].items.push(text());
        newer[0].items.push(continuation("whole-result"));
        newer.insert(0, record("call", "assistant", vec![text()]));
    } else {
        // The last quiet row on the earlier page must not swallow the newer quiet anchor.
        let row = record(
            "earlier-quiet",
            "toolResult",
            vec![read("before", Some("earlier output"))],
        );
        older.push(row.clone());
        joined.push(row);
    }
    joined.extend(newer.iter().skip(usize::from(split)).cloned());
    let mut before = feed_with(newer);
    let cache = before
        .details
        .get_mut(&daemon::detail_key(&target))
        .unwrap();
    let Some(DetailBody::Session(page)) = cache.merged.as_mut() else {
        panic!("a served session page");
    };
    page.older = Some("older".into());
    cache.pages.push(reply(page));
    let mut prepended = page.clone();
    prepended.older = None;
    prepended.messages = older;
    let mut after = before.clone();
    let cache = after.details.get_mut(&daemon::detail_key(&target)).unwrap();
    cache.pages.push(reply(&prepended));
    prepended.messages = joined;
    cache.merged = Some(DetailBody::Session(prepended));
    (before, after)
}

#[test]
fn a_cross_page_tool_result_keeps_its_anchor_even_in_a_folded_group() {
    let (before, after) = tool_seam(false);
    let target = DetailRef::Session { id: ID.into() };
    let before = shown(&before);
    let anchor = before.messages[0].anchor.clone();
    assert_eq!(anchor, "result");
    let mut ui = Ui {
        reading: Reading::At {
            key: anchor.clone(),
            offset: 7,
            rows: before.rows(),
        },
        ..Ui::default()
    };
    let show = |ui: &Ui| super::transcript(&Id::from(ID), ui, after.detail(&target), &after);
    let view = show(&ui);
    assert_eq!(
        view.following().resume,
        Resume::Row {
            key: anchor,
            offset: 7,
            rows: before.rows()
        }
    );
    let [earlier, result] = view.messages.as_slice() else {
        panic!("the earlier quiet row and the newer result group remain separate");
    };
    assert_eq!(earlier.anchor, "earlier-quiet");
    let [MessageItem::Group(group)] = result.items.as_slice() else {
        panic!("the two results on the newer page still group together");
    };
    assert_eq!(result.anchor, "result");
    assert_eq!(group.row.fold.key, "group:call:first");
    assert!(!group.open);
    ui.expanded.insert(group.row.fold.key.clone());
    let view = show(&ui);
    let [MessageItem::Group(group)] = view.messages[1].items.as_slice() else {
        panic!("the group opens at the same anchor");
    };
    let [first, second] = group.rows.as_slice() else {
        panic!("each result draws exactly once");
    };
    assert_eq!(first.fold.key, "call:first");
    assert_eq!(first.summary, "src/first.rs");
    assert_eq!(second.fold.key, "call:second");
}

#[test]
fn a_result_keeps_its_position_when_the_older_call_is_in_a_split_record() {
    let (_, mut after) = tool_seam(true);
    let id = Id::from(ID);
    let target = DetailRef::Session { id: ID.into() };
    let mut ui = Ui::default();
    ui.expanded.insert("call:first".into());
    assert_eq!(fetches(&after, &id, &ui), ["whole-call", "whole-result"]);
    after
        .items
        .insert("whole-call".into(), ItemFetch::Ready(read("first", None)));
    after.items.insert(
        "whole-result".into(),
        ItemFetch::Ready(read("first", Some("complete first output"))),
    );
    let view = super::transcript(&id, &ui, after.detail(&target), &after);
    let [call, result] = view.messages.as_slice() else {
        panic!("one assistant message and one quiet result message");
    };
    assert_eq!(call.anchor, "call");
    assert!(matches!(call.items.as_slice(), [MessageItem::Prose(_)]));
    assert_eq!(result.anchor, "result");
    let [MessageItem::Tool(first), MessageItem::Tool(second)] = result.items.as_slice() else {
        panic!("the opened result and its neighbor draw exactly once");
    };
    assert_eq!(second.fold.key, "call:second");
    assert_eq!(first.state, ToolState::Ok);
    assert_eq!(first.fold.key, "call:first");
    assert!(first.open);
    for (expected_label, words) in [
        ("arguments", "src/first.rs"),
        ("recorded call", "toolCall"),
        ("output", "complete first output"),
        ("recorded result", "complete first output"),
    ] {
        assert!(
            first
                .body
                .iter()
                .any(|section| matches!(section, Section::Text { label, text }
            if label == expected_label && text.contains(words))),
            "{expected_label} was lost: {:?}",
            first.body
        );
    }
}

#[test]
fn a_new_cut_at_a_split_tool_result_keeps_following_prose_in_its_original_bubble() {
    let target = DetailRef::Session { id: ID.into() };
    let cache = |older: Vec<SessionMessage>, newer: Vec<SessionMessage>| {
        let mut rows = older.clone();
        rows.extend(newer.clone());
        let mut feed = feed_with(rows);
        let cache = feed.details.get_mut(&daemon::detail_key(&target)).unwrap();
        let Some(DetailBody::Session(page)) = &cache.merged else {
            panic!("a served session page");
        };
        cache.pages = vec![
            reply(&SessionPage {
                messages: newer,
                older: Some("older".into()),
                ..page.clone()
            }),
            reply(&SessionPage {
                messages: older,
                ..page.clone()
            }),
        ];
        cache.clone()
    };
    let output = "first output";
    let call = read("first", None);
    let result = read("first", Some(output));
    let mut paired = call.clone();
    if let SessionItem::ToolCall {
        paired: settled,
        state,
        output: text,
        result: payload,
        ..
    } = &mut paired
    {
        *settled = true;
        *state = SessionToolState::Done;
        *text = Some(output.into());
        *payload = Some(serde_json::json!({"role": "toolResult", "text": output}).to_string());
    }
    let prose = |id: &str, text: &str| {
        record(
            id,
            "assistant",
            vec![SessionItem::Text { text: text.into() }],
        )
    };
    let earlier = prose("earlier", "An earlier reply.");
    let words = prose("words", "The read is done.");
    let held = cache(
        vec![earlier.clone()],
        vec![record("call", "assistant", vec![paired]), words.clone()],
    );
    let mut replacement = cache(
        vec![earlier, record("call", "assistant", vec![call])],
        vec![record("result", "toolResult", vec![result]), words],
    );
    let feed = Feed::default();
    let show = |cache| super::transcript(&Id::from(ID), &Ui::EMPTY, Some(cache), &feed);
    let before = show(&held);
    assert_eq!(before.messages[1].anchor, "call");
    assert!(matches!(
        before.messages[1].items.as_slice(),
        [MessageItem::Tool(_), MessageItem::Prose(_)]
    ));
    assert!(super::preserve_anchors(&feed, &held, &mut replacement));
    let after = show(&replacement);
    let [_, bubble] = after.messages.as_slice() else {
        panic!("the empty result record must not forward the new cut to the prose");
    };
    assert_eq!(bubble.anchor, "call");
    assert_eq!(bubble.keys, ["call|1", "words|1"]);
    assert!(matches!(
        bubble.items.as_slice(),
        [MessageItem::Tool(tool), MessageItem::Prose(prose)]
            if tool.fold.key == "call:first" && tool.state == ToolState::Ok
                && prose.markdown == "The read is done."
    ));
}

/// The `task` call of the parity walk, with the arguments the host's schema takes.
fn two_children(arguments: &str) -> SessionItem {
    SessionItem::ToolCall {
        call_id: "two-children".into(),
        name: "task".into(),
        arguments: arguments.into(),
        summary: String::new(),
        state: SessionToolState::Done,
        paired: true,
        call: None,
        output: Some("## fixture-reader: queued\n## fixture-checker: queued".into()),
        result: None,
        elapsed_ms: None,
        diff: None,
        content: None,
    }
}

const TWO_CHILDREN: &str = r#"{"agents":[{"id":"fixture-reader","kind":"slice","title":"Read the starting line","brief":"PARITY_CHILD_READER: Read line.txt and report the starting label without editing.","owns":[]},{"id":"fixture-checker","kind":"slice","title":"Check the project inputs","brief":"PARITY_CHILD_CHECKER: Read line.txt and report whether the gate has a nonempty input without editing.","owns":[]}]}"#;

/// The task card's rows, as (id, title).
fn task_rows(feed: &crate::model::daemon::Feed) -> Vec<(String, String)> {
    let page = shown(feed);
    let card = page
        .messages
        .iter()
        .flat_map(|message| &message.items)
        .find_map(|item| match item {
            MessageItem::Build(BuildCard::Task(card)) => Some(card),
            _ => None,
        })
        .expect("the task call is drawn as its card");
    card.rows
        .iter()
        .map(|row| (row.id.clone(), row.title.clone()))
        .collect()
}

#[test]
fn a_task_card_counts_and_lists_the_agents_its_call_passed() {
    let expected = vec![
        (
            "fixture-reader".to_owned(),
            "Read the starting line".to_owned(),
        ),
        (
            "fixture-checker".to_owned(),
            "Check the project inputs".to_owned(),
        ),
    ];
    let whole = feed_with(vec![said(
        "assistant",
        "2026-11-15T10:00:00Z",
        vec![two_children(TWO_CHILDREN)],
    )]);
    assert_eq!(task_rows(&whole), expected);

    // The page serves the call clipped, its complete item behind a continuation: the card asks
    // for it on sight and counts from it, never from the clipped preview.
    let mut clipped = feed_with(vec![said(
        "assistant",
        "2026-11-15T10:00:00Z",
        vec![
            two_children(&TWO_CHILDREN[..120]),
            SessionItem::Continuation {
                token: "t-task".into(),
                bytes: 4096,
            },
        ],
    )]);
    assert_eq!(
        fetches(&clipped, &Id::from(ID), &Default::default()),
        ["t-task"]
    );
    clipped.items.insert(
        "t-task".into(),
        ItemFetch::Ready(two_children(TWO_CHILDREN)),
    );
    assert_eq!(task_rows(&clipped), expected);
}
