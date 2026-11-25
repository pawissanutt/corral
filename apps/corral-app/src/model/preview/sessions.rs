//! Sample chat and build sessions: each is a row of the assistant topic and a pi host whose
//! transcript its page is read from; a build's host also serves its parts (`parts.rs`). Engine
//! runs are not here: they are the live runs the agents topic serves.

use corral_protocol::rows::*;
use corral_protocol::session_state::Counts;

use super::pi::{Host, Role};
use super::stamp;
use crate::model::daemon::*;

const PROFILE: &str = "omp-sol";

/// The sample build session that runs, with its children, jobs, changes, plan and board.
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
    build(feed, BUILD, "parity-guide", Some("corral"), false);
    super::parts_sample::seed(feed, BUILD);
    build(feed, STOPPED_BUILD, "docs-sweep", Some("site"), true);
    super::parts_sample::attach(feed, STOPPED_BUILD);
    build(feed, "B-code", "checkout-notes", Some("corral"), true);
    if let Some(facts) = feed
        .assistants
        .get_mut("B-code")
        .and_then(|row| row.build.as_mut())
    {
        facts.mode = corral_protocol::build::BuildMode::Code;
        facts.worktree.clone_from(&facts.repo_root);
        facts.branch = Some("main".into());
        facts.base = None;
        facts.here = true;
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

/// A chat `id`, quick or named, with a host that has said nothing yet.
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
}

/// A build `id` named `name` in `project`, in a new worktree and branch `build/<name>`, on the
/// configured build model; a stopped one's host has gone and its row is cold.
pub(super) fn build(feed: &mut Feed, id: &str, name: &str, project: Option<&str>, stopped: bool) {
    let mut session = row(feed, id, AssistantKind::Build, name, None);
    session.title = name.into();
    session.cost_usd = None;
    session.cost_basis = None;
    session.model = Some(super::parts_sample::PARENT.into());
    session.effort = Some("medium".into());
    let root = feed
        .workspace
        .clone()
        .unwrap_or_else(|| "/preview/corral".into());
    session.build = Some(BuildFacts {
        mode: corral_protocol::build::BuildMode::Build,
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
