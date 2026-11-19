use corral_protocol::detail::{SessionItem, SessionToolState};

use super::group_tests::{ID, feed_with, said, shown};
use super::{BuildCard, ItemFetch, MessageItem, Tool, ToolState, fetches};
use crate::model::Id;

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
    assert_eq!(fetches(&clipped, &Id::from(ID)), ["t-task"]);
    clipped.items.insert(
        "t-task".into(),
        ItemFetch::Ready(two_children(TWO_CHILDREN)),
    );
    assert_eq!(task_rows(&clipped), expected);
}
