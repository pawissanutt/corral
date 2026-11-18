//! Sample assistant sessions and their threads, on the unchanged chat wire. Engine runs are not
//! sessions here: they are the live runs the agents topic serves.

use corral_protocol::{assistant::*, detail::*, rows::*};

use super::{put, stamp};
use crate::model::daemon::*;

const PROFILE: &str = "omp-sol";

pub(super) fn populate(feed: &mut Feed) {
    for (id, title, focus, ended) in [
        ("A-7", "Quick thread", None, false),
        ("A-15", "T-14 timeline dates check", Some("T-14"), false),
        ("A-17", "Pairing page follow-ups", Some("T-9"), false),
        ("A-11", "Engine status table questions", None, true),
    ] {
        assistant(feed, id, title, focus, Some(PROFILE.into()));
        if ended && let Some(row) = feed.assistants.get_mut(id) {
            row.state = AssistantState::Ended;
            row.live_in = AssistantLocation::None;
        }
    }
    let asked = stamp(feed.now.saturating_sub(4 * 60));
    let mut owner = entry(
        "preview-owner-question",
        &asked,
        EntryKind::Message,
        "why is T-9 blocked, and what does T-12 wait on?",
    );
    owner.typed_at = Some(asked);
    let answered = stamp(feed.now.saturating_sub(3 * 60));
    let mut answer = entry(
        "preview-answer",
        &answered,
        EntryKind::Answer,
        "T-9 used both automatic change rounds: the reviewer still finds the paired marker cleared before the stored credential is read. Request changes with a note, retry, or land it anyway. T-12 depends on T-14, which is still running. Preview · this is an illustrative recorded conversation.",
    );
    answer.links = [("task", "T-9"), ("task", "T-12"), ("task", "T-14")]
        .into_iter()
        .map(|(kind, id)| ContextRef {
            kind: kind.into(),
            id: id.into(),
            rev: None,
        })
        .collect();
    let mut entries = vec![owner, answer];
    for (task, words) in [
        ("T-19", "changes should group under their parent task"),
        ("T-20", "keep a green gate when a rebase leaves the tree unchanged"),
    ] {
        let mut filed = entry(&format!("preview-filed:{task}"), &answered, EntryKind::Idea, words);
        filed.card = Some(EntryCard::Idea {
            task: task.into(),
            title: super::record(feed, task)
                .map(|record| record.task.title)
                .unwrap_or_default(),
            labels: vec!["preview".into()],
            criteria: vec!["Covered by one test at the lowest layer".into()],
            duplicates: vec![],
            words: words.into(),
        });
        entries.push(filed);
    }
    for scope in ["quick", "A-7"] {
        if let Some(thread) = feed.threads.get_mut(scope) {
            for entry in &entries {
                thread.entries.insert(entry.id.clone().into(), entry.clone());
            }
        }
    }
}

pub(super) fn message(author: &str, at: &str, items: Vec<SessionItem>) -> SessionMessage {
    SessionMessage {
        author: author.into(),
        at: Some(at.into()),
        items,
        model: Some("preview · sol".into()),
        stop_reason: None,
        usage: None,
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

pub(super) fn assistant(
    feed: &mut Feed,
    id: &str,
    title: &str,
    focus: Option<&str>,
    profile: Option<String>,
) {
    let at = stamp(feed.now.saturating_sub(9 * 60));
    put(
        feed,
        DetailRef::Session { id: id.into() },
        DetailBody::Session(SessionPage {
            id: id.into(),
            role: "assistant".into(),
            task: focus.map(str::to_owned),
            run: None,
            machine: Some("preview-workstation".into()),
            messages: vec![message(
                "assistant",
                &at,
                vec![SessionItem::Text {
                    text: "Preview · ready for your next message".into(),
                }],
            )],
            older: None,
            replyable: true,
            reply_refusal: None,
        }),
    );
    feed.assistants.insert(
        id.into(),
        AssistantRow {
            id: id.into(),
            kind: if id == "A-7" {
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
            rev: 1,
        },
    );
    feed.threads.insert(id.into(), ThreadSnapshot::default());
    if id == "A-7" {
        feed.threads
            .insert("quick".into(), ThreadSnapshot::default());
    }
}
