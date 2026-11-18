//! Sample chat sessions: each is a row of the assistant topic, a thread and a pi host whose
//! transcript the thread is projected from. Engine runs are not chats here: they are the live
//! runs the agents topic serves.

use corral_protocol::{assistant::*, rows::*};

use super::pi::{Host, Role};
use crate::model::daemon::*;

const PROFILE: &str = "omp-sol";

/// The sample's chats: the quick thread, two named sessions and one ended.
pub(super) fn populate(feed: &mut Feed) {
    for (id, title, focus, ended) in [
        ("A-7", "Quick thread", None, false),
        ("A-15", "T-14 timeline dates check", Some("T-14"), false),
        ("A-17", "Pairing page follow-ups", Some("T-9"), false),
        ("A-11", "Engine status table questions", None, true),
    ] {
        assistant(feed, id, title, focus, Some(PROFILE.into()), id == "A-7");
        if ended && let Some(row) = feed.assistants.get_mut(id) {
            row.state = AssistantState::Ended;
            row.live_in = AssistantLocation::None;
        }
    }
}

pub(super) fn entry(id: &str, at: &str, kind: EntryKind, text: &str) -> ThreadEntry {
    ThreadEntry {
        id: id.into(),
        rev: 1,
        at: at.into(),
        typed_at: None,
        kind,
        device: Some("preview-device".into()),
        via: Some("preview:local-only".into()),
        text: Some(text.into()),
        state: Some(MessageState::Delivered),
        model: None,
        cost_usd: None,
        tools: vec![],
        part: None,
        completion: None,
        cost_basis: None,
        links: vec![],
        card: None,
        attachments: vec![],
    }
}

/// A chat `id`, quick or named, with an empty thread and a host that has said nothing yet.
pub(super) fn assistant(
    feed: &mut Feed,
    id: &str,
    title: &str,
    focus: Option<&str>,
    profile: Option<String>,
    quick: bool,
) {
    feed.assistants.insert(
        id.into(),
        AssistantRow {
            id: id.into(),
            kind: if quick {
                AssistantKind::Quick
            } else {
                AssistantKind::Session
            },
            title: format!("Preview · {title}"),
            state: AssistantState::Ready,
            live_in: AssistantLocation::Daemon,
            profile,
            connection: None,
            focus: focus.map(str::to_owned),
            pending: 0,
            turn: None,
            cost_usd: Some(1.12),
            cost_basis: Some(CostBasis::Estimated),
            wait: None,
            question_id: None,
            rev: 1,
        },
    );
    feed.hosts.insert(id.into(), Host::new(Role::Chat));
    feed.threads.insert(id.into(), ThreadSnapshot::default());
    if quick {
        feed.threads
            .insert(QUICK_THREAD.into(), ThreadSnapshot::default());
    }
}
