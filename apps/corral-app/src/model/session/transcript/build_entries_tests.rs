use corral_protocol::detail::SessionItem;
use corral_protocol::rows::AssistantRow;

use super::group_tests::{ID, feed_with, said, shown};
use super::{BuildCard, MessageItem, ToolState, Transcript};
use crate::model::daemon::Feed;

/// A build entry as the daemon serves it: the custom message's text, and the whole message as
/// its data.
fn entry(custom: &str, content: &str, details: serde_json::Value) -> SessionItem {
    SessionItem::Card {
        title: custom.into(),
        text: content.into(),
        data: Some(
            serde_json::json!({
                "role": "custom",
                "customType": custom,
                "content": content,
                "display": true,
                "details": details,
                "timestamp": 1_763_190_000_000_u64,
            })
            .to_string(),
        ),
    }
}

/// A page of host entries, for a build whose row names its branch and changes.
fn page(items: Vec<SessionItem>) -> (Feed, Transcript) {
    let mut feed = feed_with(vec![said("custom", "2026-11-15T05:30:00Z", items)]);
    let row: AssistantRow = serde_json::from_value(serde_json::json!({
        "id": ID, "kind": "build", "title": "term", "state": "ready", "live_in": "pane",
        "pending": 0, "rev": 1,
        "build": { "name": "term", "worktree": "/work/.worktrees/term", "repo_root": "/work",
            "branch": "build/term", "here": false,
            "children": { "live": 0, "waiting": 0, "total": 0 },
            "diff": { "commits": 0, "files": 0, "added": 0, "removed": 0 } }
    }))
    .expect("a build row");
    feed.assistants.insert(ID.into(), row);
    let page = shown(&feed);
    (feed, page)
}

fn cards(page: &Transcript) -> Vec<&BuildCard> {
    page.messages
        .iter()
        .flat_map(|message| &message.items)
        .filter_map(|item| match item {
            MessageItem::Build(card) => Some(card),
            _ => None,
        })
        .collect()
}

const FINISH_TEXT: &str = "The owner opened the exit choice for this build branch. The owner chooses the exit; do not land or merge the branch yourself.";

#[test]
fn an_opened_exit_choice_is_the_build_finished_card_not_a_raw_host_card() {
    let (_, page) = page(vec![entry(
        "corral_build_finish",
        FINISH_TEXT,
        serde_json::json!({ "id": "finish-1763190000000-1" }),
    )]);
    let found = cards(&page);
    let [BuildCard::Finish(card)] = found.as_slice() else {
        panic!("the finish entry is drawn as the result card");
    };
    assert!(
        card.figures.is_some(),
        "the card says what the branch holds"
    );
    assert!(!card.superseded, "the only exit card follows the exit");
    let told = card
        .told
        .as_ref()
        .expect("the agent's words stay behind the card");
    assert!(
        !told.open && told.body.is_empty(),
        "the agent's words are not drawn folded"
    );
}

#[test]
fn an_exit_outcome_is_the_owners_choice_and_its_result_not_a_raw_host_card() {
    let (_, page) = page(vec![entry(
        "corral_build_exit",
        "The owner chose \"Keep iterating\" for this build branch. The build session continues; nothing was landed.",
        serde_json::json!({ "choice": "iterate", "label": "Keep iterating", "phase": "none" }),
    )]);
    let found = cards(&page);
    let [BuildCard::Exit(exit)] = found.as_slice() else {
        panic!("the exit entry is drawn as the owner's choice");
    };
    assert_eq!(exit.chose, "Keep iterating");
    assert_eq!(exit.outcome.state, ToolState::Waiting);
    assert!(!exit.told.open);
}

#[test]
fn a_board_filing_is_the_filed_tasks_not_a_raw_host_card() {
    let (_, page) = page(vec![entry(
        "corral_build_board",
        "The owner filed 2 board tasks and queued them; they start as slots free: T-7 Read (after none); T-8 Check (after T-7).",
        serde_json::json!({ "started": true, "filed": [
            { "id": "T-7", "title": "Read the starting line", "depends": [] },
            { "id": "T-8", "title": "Check the project inputs", "depends": ["T-7"] }
        ] }),
    )]);
    let found = cards(&page);
    let [BuildCard::Filed(filed)] = found.as_slice() else {
        panic!("the board entry is drawn as the filed tasks");
    };
    assert!(filed.filed.contains("T-7") && filed.filed.contains("T-8"));
    assert!(!filed.told.open);
}
