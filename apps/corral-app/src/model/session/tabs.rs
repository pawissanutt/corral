//! The tabs a session has, from its role and what its host declares it serves
//! (`SessionState.can`); and the frame every tab and panel body shares before its content is
//! drawn (`PartBody`: its summary line, why the host cannot serve it now, and where its read
//! stands).

use corral_protocol::detail::{DetailRef, SessionItem};

use crate::model::daemon::Feed;
use crate::model::hue::Hue;

use super::read_state::ReadState;
use super::{Action, Can, Cx, Host, SessionRole, SubView, Tab, fleet};

#[derive(Debug, Clone, PartialEq)]
pub struct TabItem {
    pub tab: Tab,
    /// `tab-<slug>`.
    pub id: String,
    /// `Chat`, or a sub-view's `Chat › Usage`, `Agents › a2`.
    pub label: String,
    pub selected: bool,
    /// Shown muted with this reason: the host declared the part but cannot serve it now.
    pub unavailable: Option<String>,
    pub action: Action,
}

/// What a tab's or panel's body shows before (and around) its content: its summary line, why
/// the host cannot serve it now, and where its read stands, in the one read-state vocabulary.
#[derive(Debug, Clone, PartialEq)]
pub struct PartBody {
    /// `12 files changed`, `Plan 3/7`, `No usage recorded yet.`
    pub summary: String,
    pub hue: Hue,
    /// The host declared the part but cannot serve it now, or the host is not running.
    pub unavailable: Option<String>,
    pub read: ReadState,
    /// `r reread`; after a failed read, Retry.
    pub reread: Option<Action>,
}

/// Why a part the host serves cannot be read from it now.
fn host_down(cx: &Cx<'_>) -> Option<String> {
    match cx.host {
        Host::NotRunning | Host::Ended => Some("the host is not running".into()),
        Host::Unreachable => Some("corrald cannot reach the host".into()),
        Host::Running | Host::Unknown => None,
    }
}

/// The body of a part read from `target`; `Err` says why the part does not apply. A part the
/// host serves names its `capability`; one read from the engine's records names none.
pub(super) fn part(
    cx: &Cx<'_>,
    target: Result<DetailRef, String>,
    capability: Option<&str>,
    summary: String,
    hue: Hue,
) -> PartBody {
    let target = match target {
        Ok(target) => target,
        Err(why) => {
            return PartBody {
                summary,
                hue,
                unavailable: None,
                read: ReadState::DoesNotApply(why),
                reread: None,
            };
        }
    };
    let unavailable = capability.and_then(|id| match cx.can(id) {
        Can::Unavailable(why) => Some(why),
        _ => host_down(cx),
    });
    let cache = cx.feed.detail(&target);
    let read = ReadState::of(cache, cx.feed);
    let reread = cache
        .filter(|cache| !cache.loading && unavailable.is_none())
        .map(|_| Action::Refresh(target));
    PartBody {
        summary,
        hue,
        unavailable,
        read,
        reread,
    }
}

/// The read a part's body needs now: once, while nothing was read and the part can be served.
pub(super) fn load(
    cx: &Cx<'_>,
    target: Result<DetailRef, String>,
    capability: Option<&str>,
) -> Option<Action> {
    let target = target.ok()?;
    let servable = match capability {
        None => true,
        Some(id) => matches!(cx.can(id), Can::Yes | Can::Unknown) && cx.host == Host::Running,
    };
    (servable && cx.feed.detail(&target).is_none()).then_some(Action::Refresh(target))
}

/// The tabs a role has, in order. A task-bound role leads with Task and ends with Proof; a build
/// ends with Board unless it is bare (no board connection).
pub fn tabs_for(role: &SessionRole, task_bound: bool, bare: bool) -> Vec<Tab> {
    match role {
        SessionRole::Child => vec![Tab::Chat],
        SessionRole::Build => {
            let mut tabs = vec![Tab::Chat, Tab::Agents, Tab::Changes];
            if !bare {
                tabs.push(Tab::Board);
            }
            tabs
        }
        _ if task_bound => vec![Tab::Task, Tab::Chat, Tab::Agents, Tab::Changes, Tab::Proof],
        _ => vec![Tab::Chat, Tab::Agents, Tab::Changes],
    }
}

/// The host capability a tab's body is served by; Chat, Task and Proof read what every session
/// has (its transcript, its task's engine records).
fn capability(tab: Tab) -> Option<&'static str> {
    match tab {
        Tab::Agents => Some("children"),
        Tab::Changes => Some("changes"),
        Tab::Board => Some("board"),
        Tab::Chat | Tab::Task | Tab::Proof => None,
    }
}

/// The rows a reader would count as new: served messages and the live turn's messages and tool
/// calls. Streaming into a row does not change it.
pub fn rows(feed: &Feed, scope: &str) -> usize {
    let served = super::transcript::page(feed.detail(&DetailRef::Session {
        id: scope.to_owned(),
    }))
    .map_or(0, |page| {
        page.messages
            .iter()
            .filter(|message| {
                message
                    .items
                    .iter()
                    .any(|item| !matches!(item, SessionItem::Continuation { .. }))
            })
            .count()
    });
    let live = feed
        .live
        .get(scope)
        .map_or(0, |live| live.messages.len() + live.tools.len());
    served + live
}

fn label(tab: Tab, sub: Option<&SubView>) -> String {
    match (tab, sub) {
        (Tab::Chat, Some(SubView::Usage)) => "Chat › Usage".into(),
        (Tab::Agents, Some(SubView::Agent(child))) => format!("Agents › {child}"),
        (Tab::Agents, Some(SubView::Job(job))) => {
            format!("Agents › job {}", fleet::short_id(job))
        }
        _ => tab.label().into(),
    }
}

/// The tabs the page shows: the role's set, without a tab whose part the host does not serve
/// (when it reported what it serves).
pub fn items(cx: &Cx<'_>) -> Vec<TabItem> {
    let bare = !matches!(cx.can("board"), Can::Yes | Can::Unavailable(_));
    let tabs = tabs_for(cx.role, cx.role.task_bound() && cx.task.is_some(), bare);
    tabs.into_iter()
        .filter_map(|tab| {
            let unavailable = match capability(tab).map(|id| cx.can(id)) {
                Some(Can::Absent) => return None,
                Some(Can::Unknown) if tab != Tab::Agents => return None,
                Some(Can::Unavailable(why)) => Some(why),
                _ => None,
            };
            let selected = cx.ui.tab == tab;
            Some(TabItem {
                tab,
                id: format!("tab-{}", tab.slug()),
                label: label(tab, cx.ui.sub.as_ref().filter(|_| selected)),
                selected,
                unavailable,
                action: Action::Tab {
                    session: cx.id.clone(),
                    tab,
                },
            })
        })
        .collect()
}
