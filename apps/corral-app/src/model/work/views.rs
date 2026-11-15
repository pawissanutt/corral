//! Complete workspace, Changes and More content projections of the fixture preview, and the
//! owned feed, hold and Changes descriptors the Daemon state's builders share.

use crate::model::board::{Change, ChangeKind, LAST_VISIT, Mark, changes as board_changes};
use crate::model::effect::{Conn, Env};
use crate::model::hue::Hue;
use crate::model::nav::{Destination, Form};
use crate::model::text::{Rich, span};

use super::{
    Action, Button, ButtonKind, ChangeFilter, CtxTab, GroupedList, HOLDS, Inspector, LiveDock,
    LowTab, NowTab, PlanContext, Press, State, grouped_list_with_question, inspector, live,
    plan_context, question_open,
};

/// A connection banner with its entire inline emphasis and handled controls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Banner {
    /// Connection state hue.
    pub hue: Hue,
    /// Complete message; the renderer inserts no additional wording.
    pub text: Rich,
    /// Desktop/terminal controls; the phone banner has none.
    pub actions: Vec<Button>,
}

/// Staleness flags and text used by every work-domain screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    /// Dim and desaturate board content when the daemon is down or the device is offline.
    pub dimmed: bool,
    /// Snapshot timestamp to accompany stale views.
    pub as_of: Option<String>,
    /// Connection banner; healthy and relay connections have no banner.
    pub banner: Option<Banner>,
}

/// Resolve degraded, daemon-down and offline messages without consulting a UI service.
pub fn status(env: Env) -> Status {
    let phone = env.form == Form::Phone;
    let banner = match env.conn {
        Conn::Degraded => Some(Banner {
            hue: Hue::Held,
            text: vec![
                span("corrald degraded · sync stalled 4m.").bold(),
                span(
                    " The board can trail reality by up to 4 minutes; commands still go through and are revision-checked. PM is relaunching.",
                ),
            ],
            actions: if phone {
                Vec::new()
            } else {
                vec![Button::new(
                    "Details",
                    Action::Connection,
                    ButtonKind::Quiet,
                )]
            },
        }),
        Conn::Down => Some(Banner {
            hue: Hue::Fail,
            text: vec![
                span("corrald is down since 22:09.").bold(),
                span(
                    " Showing the board as of 22:08, marked stale. Nothing syncs, gates or lands until it is back.",
                ),
            ],
            actions: if phone {
                Vec::new()
            } else {
                vec![
                    Button::new("Restart corrald", Action::RestartDaemon, ButtonKind::Ghost),
                    Button::new("Diagnostics", Action::Connection, ButtonKind::Quiet),
                ]
            },
        }),
        Conn::Offline => Some(Banner {
            hue: Hue::Neutral,
            text: vec![
                span("Offline · last sync 22:08.").bold(),
                span(
                    " Reading the last known state. A command queues only if corrald can check it on reconnect; everything else refuses. ",
                ),
                span("2 queued").bold(),
                span(
                    " (answer T-868/R1/Q1 at rev 2, checkable on reconnect; one message to the quick thread) · ",
                ),
                span("1 refused").bold(),
                span(" (accept plan T-885 P2)."),
            ],
            actions: if phone {
                Vec::new()
            } else {
                vec![Button::new("Queue", Action::Connection, ButtonKind::Quiet)]
            },
        }),
        Conn::Healthy | Conn::Relay => None,
    };
    Status {
        dimmed: env.conn.refuses(),
        as_of: env.conn.refuses().then(|| "as of 22:08".to_owned()),
        banner,
    }
}

/// A model-owned tab label, selected state and reducer action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tab {
    /// Complete visible tab wording.
    pub label: &'static str,
    /// Whether this tab is selected.
    pub selected: bool,
    /// The domain action that selects it.
    pub action: Action,
}

/// One feed row in Now or Changes, with link and illustrative flags resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedRow {
    /// Local time, `18:40` today or `Nov 12` before.
    pub at: String,
    /// The task the event concerns; empty for an event about none.
    pub id: String,
    /// The mark drawn for the event.
    pub kind: ChangeKind,
    /// Exact event wording.
    pub text: String,
    /// Who did it and through what, as corrald attributes it; absent in the fixture.
    pub by: Option<String>,
    /// New since this device's last visit.
    pub unseen: bool,
    /// The historical rows after the seen line are dimmed.
    pub dim: bool,
    /// The 21:59 reply and three historical rows are illustrative.
    pub illustrative: bool,
    /// Rows about a task open it; the historical source rows are plain text.
    pub action: Option<Press>,
}

impl FeedRow {
    fn fixture(change: &Change, unseen: bool, dim: bool, illustrative: bool) -> FeedRow {
        FeedRow {
            at: change.at.to_owned(),
            id: change.id.to_owned(),
            kind: change.kind,
            text: change.text.to_owned(),
            by: None,
            unseen,
            dim,
            illustrative,
            action: (!dim).then(|| Press::Work(Action::OpenTask(change.id))),
        }
    }

    /// What a screen reader hears: time, task, event and attribution.
    pub fn label(&self) -> String {
        [self.at.as_str(), self.id.as_str(), self.text.as_str()]
            .into_iter()
            .chain(self.by.as_deref())
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Whether a fixture change passes `filter`, by the mark the feed draws for it.
fn admits_kind(filter: ChangeFilter, kind: ChangeKind) -> bool {
    match filter {
        ChangeFilter::All => true,
        ChangeFilter::NeedsYou => kind == ChangeKind::Message,
        ChangeFilter::Verdicts => matches!(kind, ChangeKind::Mark(_)),
        ChangeFilter::Runs => matches!(kind, ChangeKind::Start | ChangeKind::Note),
    }
}

/// The newest-first snapshot feed, ending before the last-visit marker.
pub fn feed(state: &State, _env: Env) -> Vec<FeedRow> {
    since_rows(ChangeFilter::All, state)
}

fn since_rows(filter: ChangeFilter, _state: &State) -> Vec<FeedRow> {
    board_changes()
        .iter()
        .filter(|change| change.at >= LAST_VISIT && admits_kind(filter, change.kind))
        .map(|change| FeedRow::fixture(change, true, false, change.at == "21:59"))
        .collect()
}

/// A displayed hold row, with the source's neutral mark and dim attribution suffix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoldRow {
    /// Mono task id.
    pub id: String,
    /// When the hold began: a date, or how long ago.
    pub since: String,
    /// Full task title, for screen readers.
    pub title: String,
    /// The hold row's neutral not-applicable mark.
    pub mark: Mark,
    /// Title followed by the dimmed attribution.
    pub text: Rich,
    /// Open the task, in the Daemon state.
    pub action: Option<Press>,
}

/// Build the six parked rows displayed by the lower dock tab.
pub fn holds(_state: &State, _env: Env) -> Vec<HoldRow> {
    HOLDS
        .iter()
        .map(|hold| HoldRow {
            id: hold.id.to_owned(),
            since: hold.since.to_owned(),
            title: hold.title.to_owned(),
            mark: Mark::Na,
            text: vec![span(hold.title), span(format!(" · {}", hold.project)).dim()],
            action: None,
        })
        .collect()
}

/// The selected lower-left dock content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lower {
    /// Changes since this device's last visit.
    Since(Vec<FeedRow>),
    /// Owner-held tasks.
    Parked(Vec<HoldRow>),
}

/// The selected lower-right dock content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Context {
    /// Selected terminal tail and optional running-agent tiles.
    Live(LiveDock),
    /// Plan and member ids handed to the Plans graph renderer.
    PlanGraph(PlanContext),
}

/// Now's desktop/terminal content, with all four dock panels resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct Workspace {
    /// Connection banner and stale board flags.
    pub status: Status,
    /// Who-acts-next list, or its loading placeholders.
    pub list: GroupedList,
    /// The Goals control alongside the fixed list tab.
    pub goals: Button,
    /// Story/Evidence/Session tab descriptors.
    pub inspector_tabs: Vec<Tab>,
    /// Selected inspector body.
    pub inspector: Inspector,
    /// Since your last visit/Parked tab descriptors.
    pub lower_tabs: Vec<Tab>,
    /// Selected lower-left body.
    pub lower: Lower,
    /// Live/Plan graph tab descriptors.
    pub context_tabs: Vec<Tab>,
    /// Selected lower-right body.
    pub context: Context,
    /// Labelled but unhandled mockup controls at each dock's end; no reducer is invented.
    pub inert_tools: &'static [&'static str],
}

/// Build all four Now dock panels from the environment's question projection.
pub fn workspace(state: &State, env: Env) -> Workspace {
    workspace_with_question(state, env, question_open(env))
}

/// Build all Now docks with an app-supplied inbox question-open projection.
/// This avoids importing the inbox domain for the source's shared `q.state` branch.
pub fn workspace_with_question(state: &State, env: Env, question_is_open: bool) -> Workspace {
    Workspace {
        status: status(env),
        list: grouped_list_with_question(state, env, question_is_open),
        goals: Button::new("Goals", Action::Goals, ButtonKind::Quiet),
        inspector_tabs: NowTab::ALL
            .iter()
            .map(|tab| Tab {
                label: tab.label(),
                selected: state.now_tab == *tab,
                action: Action::NowTab(*tab),
            })
            .collect(),
        inspector: inspector(state, env),
        lower_tabs: LowTab::ALL
            .iter()
            .map(|tab| Tab {
                label: tab.label(),
                selected: state.low_tab == *tab,
                action: Action::LowTab(*tab),
            })
            .collect(),
        lower: match state.low_tab {
            LowTab::Since => Lower::Since(feed(state, env)),
            LowTab::Parked => Lower::Parked(holds(state, env)),
        },
        context_tabs: CtxTab::ALL
            .iter()
            .map(|tab| Tab {
                label: tab.label(),
                selected: state.ctx_tab == *tab,
                action: Action::CtxTab(*tab),
            })
            .collect(),
        context: match state.ctx_tab {
            CtxTab::Live => Context::Live(live(state, env)),
            CtxTab::PlanGraph => Context::PlanGraph(plan_context(state, env)),
        },
        inert_tools: &["Split", "Zoom"],
    }
}

/// A Changes filter control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeedFilter {
    /// Filter label, including desktop-only Runs.
    pub label: &'static str,
    /// Whether the feed shows this filter's events now.
    pub selected: bool,
    /// The filter it selects.
    pub filter: ChangeFilter,
}

/// The filter controls of a form: the phone has no Runs, as in the mockup.
pub fn feed_filters(state: &State, form: Form) -> Vec<FeedFilter> {
    ChangeFilter::ALL
        .into_iter()
        .filter(|filter| form != Form::Phone || *filter != ChangeFilter::Runs)
        .map(|filter| FeedFilter {
            label: filter.label(),
            selected: state.filter == filter,
            filter,
        })
        .collect()
}

/// Complete Changes content before and after the per-device seen line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Changes {
    /// Page title.
    pub title: &'static str,
    /// Desktop subtitle explaining the per-device cursor, absent on phone.
    pub subtitle: Option<String>,
    /// The filter controls.
    pub filters: Vec<FeedFilter>,
    /// Recent rows before the seen marker.
    pub recent: Vec<FeedRow>,
    /// Seen-cursor line; desktop names the device, phone uses a shorter last-visit label.
    pub seen_line: String,
    /// Rows this device has seen, dimmed.
    pub history: Vec<FeedRow>,
    /// Desktop-only illustrative attribution below the feed, in the fixture.
    pub illustrative: Option<&'static str>,
    /// Desktop-only Mark seen control.
    pub mark_seen: Option<Button>,
    /// Fetch the page before the oldest row shown, while corrald has more.
    pub older: Option<Button>,
    /// One line instead of, or after, the rows: loading, empty, refused or failed.
    pub notice: Option<String>,
    /// Connection banner and stale flags.
    pub status: Status,
}

/// Build Changes; the shell owns the phone stack's back label.
pub fn changes(state: &State, env: Env) -> Changes {
    let phone = env.form == Form::Phone;
    let history = if phone {
        Vec::new()
    } else {
        [
            Change {
                at: "18:12",
                id: "T-885",
                kind: ChangeKind::Mark(Mark::Na),
                text: "review P2 approve · pi-sol",
            },
            Change {
                at: "17:40",
                id: "T-854",
                kind: ChangeKind::Mark(Mark::Na),
                text: "checkpoint 2 captured",
            },
            Change {
                at: "16:40",
                id: "T-852",
                kind: ChangeKind::Mark(Mark::Na),
                text: "held · external op started",
            },
        ]
        .iter()
        .filter(|change| admits_kind(state.filter, change.kind))
        .map(|change| FeedRow::fixture(change, false, true, true))
        .collect()
    };
    Changes {
        title: "Changes",
        subtitle: (!phone).then(|| "since this device's last visit 18:40 · 12 events · seen-cursor is per device: your phone read these at 21:30, the ▲ marks here stay".to_owned()),
        filters: feed_filters(state, env.form),
        recent: since_rows(state.filter, state),
        seen_line: if phone { "last visit 18:40" } else { "your last visit · 18:40 · seen-cursor for pawis-desktop" }.to_owned(),
        history,
        illustrative: (!phone).then_some("Events after 18:40 are real snapshot changes; the three before the line and 21:59 are illustrative."),
        mark_seen: (!phone).then(|| Button::new("Mark seen", Action::MarkSeen, ButtonKind::Ghost)),
        older: None,
        notice: None,
        status: status(env),
    }
}

/// One navigation row on phone More, including its subtitle and structured counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MoreRow {
    /// Destination identity, also identifying its common navigation icon.
    pub destination: Destination,
    /// Main row label.
    pub label: &'static str,
    /// Complete smaller second line.
    pub subtitle: &'static str,
    /// Principal count, if the subtitle has one.
    pub count: Option<u32>,
    /// Secondary stale-verdict/stall count, if present.
    pub secondary_count: Option<u32>,
    /// Provider window percentage, only for Usage.
    pub percent: Option<u32>,
    /// Push/navigate to this destination.
    pub action: Action,
}

/// The phone More page, with all five navigation rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct More {
    /// Large navigation title.
    pub title: &'static str,
    /// Destination rows in source order.
    pub rows: &'static [MoreRow],
    /// Connection banner and stale flags, supplied by the phone shell in the mockup.
    pub status: Status,
}

const fn bare(destination: Destination, label: &'static str) -> MoreRow {
    MoreRow {
        destination,
        label,
        subtitle: "",
        count: None,
        secondary_count: None,
        percent: None,
        action: Action::Go(destination),
    }
}

/// The More rows in the Daemon state: the destinations alone, with no fixture count or
/// subtitle.
const DAEMON_MORE: &[MoreRow] = &[
    bare(Destination::Changes, "Changes"),
    bare(Destination::Review, "Review"),
    bare(Destination::Timeline, "Timeline"),
    bare(Destination::Usage, "Usage"),
    bare(Destination::Settings, "Settings"),
];

/// The phone More list in the Daemon state.
pub fn daemon_more(feed: &crate::model::daemon::Feed) -> More {
    More {
        title: "More",
        rows: DAEMON_MORE,
        status: super::daemon_status(feed),
    }
}

/// Build the phone More list; count and subtitle wording never live in the UI.
pub fn more(_state: &State, mut env: Env) -> More {
    env.form = Form::Phone;
    More {
        title: "More",
        status: status(env),
        rows: &[
            MoreRow {
                destination: Destination::Changes,
                label: "Changes",
                subtitle: "12 since 18:40",
                count: Some(12),
                secondary_count: None,
                percent: None,
                action: Action::Go(Destination::Changes),
            },
            MoreRow {
                destination: Destination::Review,
                label: "Review",
                subtitle: "3 candidates · 1 stale verdict",
                count: Some(3),
                secondary_count: Some(1),
                percent: None,
                action: Action::Go(Destination::Review),
            },
            MoreRow {
                destination: Destination::Timeline,
                label: "Timeline",
                subtitle: "today · 1 stall",
                count: None,
                secondary_count: Some(1),
                percent: None,
                action: Action::Go(Destination::Timeline),
            },
            MoreRow {
                destination: Destination::Usage,
                label: "Usage",
                subtitle: "OpenAI 78% of window",
                count: None,
                secondary_count: None,
                percent: Some(78),
                action: Action::Go(Destination::Usage),
            },
            MoreRow {
                destination: Destination::Settings,
                label: "Settings",
                subtitle: "Devices, Machines, notifications",
                count: None,
                secondary_count: None,
                percent: None,
                action: Action::Go(Destination::Settings),
            },
        ],
    }
}
