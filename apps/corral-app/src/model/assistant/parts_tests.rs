use corral_protocol::assistant::{EntryKind, EntryPart, MessageState, ThreadEntry};
use corral_protocol::rows::{AssistantKind, AssistantLocation, AssistantRow, AssistantState};

use super::{Body, State, ThreadItem, live_chip, live_items};
use crate::model::daemon::{Feed, Link, ThreadSnapshot};

fn entry(id: String, kind: EntryKind, text: String) -> ThreadEntry {
    ThreadEntry {
        id,
        kind,
        text: Some(text),
        rev: 1,
        at: "2026-11-15T10:00:00Z".into(),
        typed_at: None,
        device: None,
        via: None,
        state: None,
        model: None,
        cost_usd: None,
        tools: Vec::new(),
        part: None,
        completion: None,
        cost_basis: None,
        links: Vec::new(),
        card: None,
        attachments: Vec::new(),
    }
}

fn answer(feed: &Feed) -> super::AssistantMessage {
    let messages: Vec<_> = live_items(&State::default(), feed, "A-1")
        .into_iter()
        .filter_map(|item| match item {
            ThreadItem::Assistant(message) => Some(message),
            _ => None,
        })
        .collect();
    assert_eq!(
        messages.len(),
        1,
        "transport parts must not become extra turns"
    );
    messages.into_iter().next().expect("one semantic turn")
}

fn text(message: &super::AssistantMessage) -> String {
    message
        .body
        .iter()
        .filter_map(|body| match body {
            Body::Markdown(words) => Some(words.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[test]
fn long_answer_parts_render_once_in_numeric_order() {
    // Twelve small suffixes exercise /10 before /2 lexical ordering without a large fixture.
    let parts: Vec<_> = std::iter::once("a".repeat(7 * 1024))
        .chain((1..12).map(|index| format!("[{index}]")))
        .collect();
    let canonical = parts.concat();
    for metadata in [false, true] {
        let mut thread = ThreadSnapshot::default();
        for (index, part) in parts.iter().enumerate() {
            let mut row = entry(
                format!("A-1/M1/answer/{index}"),
                EntryKind::Answer,
                part.clone(),
            );
            if metadata {
                row.part = Some(EntryPart {
                    id: "A-1/M1/answer/0".into(),
                    index: index as u32,
                    total: parts.len() as u32,
                    bytes: canonical.len() as u64,
                    complete: false,
                });
            }
            thread.entries.insert(row.id.as_str().into(), row);
        }
        let mut owner = entry("A-1/M1".into(), EntryKind::Message, "owner ".into());
        owner.state = Some(MessageState::Waiting {
            reason: corral_protocol::rows::WaitReason {
                code: "host_down".into(),
                text: "unavailable".into(),
                retry_at: None,
            },
        });
        thread
            .entries
            .insert(owner.id.as_str().into(), owner.clone());
        owner.id = "A-1/M1/text/1".into();
        owner.text = Some("continuation".into());
        thread.entries.insert(owner.id.as_str().into(), owner);
        let mut feed = Feed::default();
        feed.threads.insert("A-1".into(), thread);
        assert_eq!(text(&answer(&feed)), canonical);
        let rows = live_items(&State::default(), &feed, "A-1");
        assert_eq!(
            rows.iter()
                .filter(|item| matches!(item, ThreadItem::Waiting(_)))
                .count(),
            1
        );
        let owners: Vec<_> = rows
            .iter()
            .filter_map(|item| match item {
                ThreadItem::Owner(owner) => Some(owner.text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(owners, ["owner continuation"]);
    }
}

#[test]
fn cold_sessions_with_recorded_history_are_not_presented_as_never_started() {
    let mut feed = Feed {
        link: Link::Up,
        ..Feed::default()
    };
    feed.assistants.insert(
        "A-1".into(),
        AssistantRow {
            id: "A-1".into(),
            kind: AssistantKind::Session,
            title: "Session".into(),
            state: AssistantState::Cold,
            live_in: AssistantLocation::None,
            profile: None,
            connection: None,
            focus: None,
            pending: 0,
            turn: None,
            cost_usd: None,
            cost_basis: None,
            wait: None,
            rev: 1,
        },
    );
    let fresh = live_chip(&feed, "A-1");
    feed.assistants.get_mut("A-1").unwrap().turn = Some(1);
    let resumed = live_chip(&feed, "A-1");
    assert_ne!(fresh.label, resumed.label);
    assert_eq!(fresh.hue, resumed.hue);
    feed.assistants.get_mut("A-1").unwrap().turn = None;
    let owner = entry("A-1/M1".into(), EntryKind::Message, "history".into());
    let mut thread = ThreadSnapshot::default();
    thread.entries.insert(owner.id.as_str().into(), owner);
    feed.threads.insert("A-1".into(), thread);
    assert_eq!(live_chip(&feed, "A-1"), resumed);
}

#[test]
fn quick_thread_end_closes_its_current_generation_and_frees_the_next_one() {
    use super::{Action, live_thread_header};
    use crate::model::Effect;
    use corral_protocol::assistant::AssistantSessionOp;
    let quick = |id: &str, state| AssistantRow {
        id: id.into(),
        kind: AssistantKind::Quick,
        title: "Quick thread".into(),
        state,
        live_in: AssistantLocation::Daemon,
        profile: None,
        connection: None,
        focus: None,
        pending: 0,
        turn: None,
        cost_usd: None,
        cost_basis: None,
        wait: None,
        rev: 1,
    };
    let mut feed = Feed {
        transport_ready: true,
        projection_ready: true,
        link: Link::Up,
        session: Some(serde_json::from_value(serde_json::json!({
            "device":{"id":"dev","name":"Desk","platform":"native","app_version":"0","enrolled_at":"2026-11-15T09:00:00Z","revoked_at":null},
            "authority":{"reply":true,"act":true,"assistant":true,"assistant_act":true,"admin":true},
            "seen":{"through":0,"at":null},"server_time":"2026-11-15T09:00:00Z"
        })).unwrap()),
        ..Feed::default()
    };
    feed.assistants
        .insert("A-2".into(), quick("A-2", AssistantState::Ready));
    let mut state = State::default();
    assert!(
        live_thread_header(&state, &feed, super::QUICK)
            .end
            .is_some()
    );
    let effects = state.act_live(Action::ConfirmEnd(super::QUICK.into()), &feed);
    assert!(matches!(
        effects.as_slice(),
        [Effect::Session { op: AssistantSessionOp::End { session }, .. }] if session == "A-2"
    ));
    feed.assistants.get_mut("A-2").unwrap().state = AssistantState::Ended;
    assert!(
        live_thread_header(&state, &feed, super::QUICK)
            .end
            .is_none()
    );
    feed.assistants
        .insert("A-3".into(), quick("A-3", AssistantState::Cold));
    let end = live_thread_header(&state, &feed, super::QUICK).end.unwrap();
    assert!(
        end.enabled,
        "an earlier generation's End must not hold the new one"
    );
}
