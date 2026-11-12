//! Complete workspace, Changes and More content projections.

use crate::model::board::{Change, ChangeKind, LAST_VISIT, Mark, changes as board_changes};
use crate::model::effect::{Conn, Env};
use crate::model::hue::Hue;
use crate::model::nav::{Destination, Form, PhoneTab};
use crate::model::text::{Rich, span};

use super::{
    Action, Button, ButtonKind, CtxTab, GroupedList, HOLDS, Hold, Inspector, Live, LowTab, NowTab,
    PlanContext, State, grouped_list_with_question, inspector, live, plan_context, question_open,
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
    pub as_of: Option<&'static str>,
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
        as_of: env.conn.refuses().then_some("as of 22:08"),
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeedRow {
    /// Clock, task id, mark kind and exact event wording.
    pub change: Change,
    /// New since this device's last visit.
    pub unseen: bool,
    /// The historical rows after the seen line are dimmed.
    pub dim: bool,
    /// The 21:59 reply and three historical rows are illustrative.
    pub illustrative: bool,
    /// Recent rows open the task; the historical source rows are plain text.
    pub action: Option<Action>,
}

/// The newest-first snapshot feed, ending before the last-visit marker.
pub fn feed(_state: &State, _env: Env) -> Vec<FeedRow> {
    board_changes()
        .iter()
        .filter(|change| change.at >= LAST_VISIT)
        .map(|change| FeedRow {
            change: *change,
            unseen: true,
            dim: false,
            illustrative: change.at == "21:59",
            action: Some(Action::OpenTask(change.id)),
        })
        .collect()
}

/// A displayed owner-hold row, with the source's neutral mark and dim project suffix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoldRow {
    /// Original task, project, date and optional owner answer.
    pub hold: Hold,
    /// The hold row's neutral not-applicable mark.
    pub mark: Mark,
    /// Title followed by the dimmed project attribution.
    pub text: Rich,
}

/// Build the six parked rows displayed by the lower dock tab.
pub fn holds(_state: &State, _env: Env) -> Vec<HoldRow> {
    HOLDS
        .iter()
        .map(|hold| HoldRow {
            hold: *hold,
            mark: Mark::Na,
            text: vec![span(hold.title), span(format!(" · {}", hold.project)).dim()],
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
    Live(Live),
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

/// A Changes filter control; the source has no filtering handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeedFilter {
    /// Filter label, including desktop-only Runs.
    pub label: &'static str,
    /// All is selected in the source.
    pub selected: bool,
}

/// Complete Changes content before and after the per-device seen line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Changes {
    /// Page title.
    pub title: &'static str,
    /// Phone navigation back label, absent on desktop/terminal.
    pub back_label: Option<&'static str>,
    /// Pop the phone page, absent on desktop/terminal.
    pub back: Option<Action>,
    /// Desktop subtitle explaining the per-device cursor, absent on phone.
    pub subtitle: Option<&'static str>,
    /// Static source filters, with All selected.
    pub filters: &'static [FeedFilter],
    /// Recent, clickable rows before the seen marker.
    pub recent: Vec<FeedRow>,
    /// Seen-cursor line; desktop names the device, phone uses a shorter last-visit label.
    pub seen_line: &'static str,
    /// Three dim historical rows, omitted on phone.
    pub history: Vec<FeedRow>,
    /// Desktop-only illustrative attribution below the feed.
    pub illustrative: Option<&'static str>,
    /// Desktop-only Mark seen control, whose source handler shows a notice only.
    pub mark_seen: Option<Button>,
    /// Connection banner and stale flags.
    pub status: Status,
}

/// Build Changes as entered from the phone More tab, or the desktop destination.
/// Use [`changes_for_tab`] when the phone back label depends on a different current tab.
pub fn changes(state: &State, env: Env) -> Changes {
    changes_for_tab(state, env, PhoneTab::More)
}

/// Build Changes with the phone shell's current tab supplied for the back label.
pub fn changes_for_tab(state: &State, env: Env, tab: PhoneTab) -> Changes {
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
        .into_iter()
        .map(|change| FeedRow {
            change,
            unseen: false,
            dim: true,
            illustrative: true,
            action: None,
        })
        .collect()
    };
    Changes {
        title: "Changes",
        back_label: phone.then_some(if tab == PhoneTab::More { "More" } else { "Back" }),
        back: phone.then_some(Action::Back),
        subtitle: (!phone).then_some("since this device's last visit 18:40 · 12 events · seen-cursor is per device: your phone read these at 21:30, the ▲ marks here stay"),
        filters: if phone { &[
            FeedFilter { label: "All", selected: true }, FeedFilter { label: "Needs you", selected: false },
            FeedFilter { label: "Verdicts", selected: false },
        ] } else { &[
            FeedFilter { label: "All", selected: true }, FeedFilter { label: "Needs you", selected: false },
            FeedFilter { label: "Verdicts", selected: false }, FeedFilter { label: "Runs", selected: false },
        ] },
        recent: feed(state, env),
        seen_line: if phone { "last visit 18:40" } else { "your last visit · 18:40 · seen-cursor for pawis-desktop" },
        history,
        illustrative: (!phone).then_some("Events after 18:40 are real snapshot changes; the three before the line and 21:59 are illustrative."),
        mark_seen: (!phone).then(|| Button::new("Mark seen", Action::MarkSeen, ButtonKind::Ghost)),
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
