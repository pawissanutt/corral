//! Sample chat and build sessions: each is a row of the assistant topic and a pi host whose
//! transcript its page is read from; a chat also has its thread. Engine runs are not here: they
//! are the live runs the agents topic serves.

use corral_protocol::session_state::Counts;
use corral_protocol::{assistant::*, rows::*};

use super::pi::{Host, Role};
use super::stamp;
use crate::model::daemon::*;

const PROFILE: &str = "omp-sol";

/// The sample build session that runs, with a recorded turn and child agents.
pub(super) const BUILD: &str = "B-k3x9";

/// The sample build session whose host stopped: listed under Show All, resumable.
pub(super) const STOPPED_BUILD: &str = "B-k2a1";

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
    build(feed, BUILD, "auth-refactor", Some("corral"), false);
    if let Some(host) = feed.hosts.get_mut(BUILD) {
        let at = stamp(feed.now.saturating_sub(40 * 60));
        host.record_turn(
            &at,
            &["Split the auth module into a token store and a session API."],
            vec![corral_protocol::detail::SessionItem::Text {
                text: "Preview · sample build. Two slices, one worktree: the token store and the session API. Starting both now."
                    .into(),
            }],
        );
        // It asks the owner two things at once before the slices start.
        host.pending.push(super::build_ops::api_question(&at));
    }
    build(feed, STOPPED_BUILD, "docs-sweep", Some("site"), true);
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

fn row(
    feed: &Feed,
    id: &str,
    kind: AssistantKind,
    title: &str,
    profile: Option<String>,
) -> AssistantRow {
    AssistantRow {
        id: id.into(),
        kind,
        title: format!("Preview · {title}"),
        state: AssistantState::Ready,
        live_in: AssistantLocation::Daemon,
        profile,
        connection: None,
        focus: None,
        pending: 0,
        turn: None,
        cost_usd: Some(1.12),
        cost_basis: Some(CostBasis::Estimated),
        wait: None,
        question_id: None,
        rev: 1,
        model: Some("preview/sol".into()),
        effort: Some("high".into()),
        created: Some(stamp(feed.now.saturating_sub(3600))),
        updated: Some(stamp(feed.now)),
        build: None,
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
    let kind = if quick {
        AssistantKind::Quick
    } else {
        AssistantKind::Session
    };
    let mut chat = row(feed, id, kind, title, profile);
    chat.focus = focus.map(str::to_owned);
    feed.assistants.insert(id.into(), chat);
    feed.hosts.insert(id.into(), Host::new(Role::Chat));
    feed.threads.insert(id.into(), ThreadSnapshot::default());
    if quick {
        feed.threads
            .insert(QUICK_THREAD.into(), ThreadSnapshot::default());
    }
}

/// A build `id` named `name` in `project`, in a new worktree and branch `build/<name>`; a
/// stopped one's host has gone and its row is cold.
pub(super) fn build(feed: &mut Feed, id: &str, name: &str, project: Option<&str>, stopped: bool) {
    let mut session = row(feed, id, AssistantKind::Build, name, None);
    session.title = name.into();
    session.cost_usd = None;
    session.cost_basis = None;
    let root = feed
        .workspace
        .clone()
        .unwrap_or_else(|| "/preview/corral".into());
    session.build = Some(BuildFacts {
        name: name.into(),
        project: project.map(str::to_owned),
        worktree: format!("{root}/.worktrees/{name}"),
        repo_root: root,
        branch: Some(format!("build/{name}")),
        base: Some("main".into()),
        here: false,
        exit: None,
        children: Counts::default(),
        diff: None,
    });
    if stopped {
        session.state = AssistantState::Cold;
        session.live_in = AssistantLocation::None;
    }
    feed.assistants.insert(id.into(), session);
    let mut host = Host::new(Role::Build);
    host.gone = stopped;
    feed.hosts.insert(id.into(), host);
}
