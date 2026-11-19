//! The Board tab of a build: the corral tasks it filed and a proposed plan waiting for the
//! owner, read from its host (`BuildBoard`, refreshed with `board_refresh`). Each task shows its
//! stage in the terminal UI's words, what it waits for, its dependencies, the host's detail and
//! its last change; a row opens the task's own page in the app, never a copy here. The proposed
//! plan reads in full (its reasons and each task with its brief; Show brief reads the part of a
//! brief the board reply left out, `plan_brief.rs`); its File / File and start / Revise answers
//! are the dock's board-plan card, which Review plan brings forward. The tab's badge counts tasks
//! not landed and is marked when one is blocked or failed. The build's record opens in a pager
//! over the board (`record.rs`).

use std::collections::BTreeSet;

use corral_protocol::detail::{
    BoardPlan, BoardTask, BuildBoard, CardKind, DetailBody, DetailGet, DetailRef,
};
use corral_protocol::session_host::{SessionAction, SessionActionRequest};

use crate::model::daemon::Feed;
use crate::model::effect::Effect;
use crate::model::hue::Hue;
use crate::model::{Id, Live};

use super::actions::{ActionGroup, row};
use super::dock::{DockAction, Pending};
use super::keys::{KeyAction, KeyBinding, bind};
use super::tabs::{Badge, PartBody, count, load, part};
use super::{Action, Can, Cx, Focus, Host, SessionRole, State, Tab};

#[path = "record.rs"]
mod record;

#[path = "plan_brief.rs"]
mod plan_brief;

pub use plan_brief::BriefRead;
pub use record::{RecordLine, RecordPager, RecordSection, RecordView};

const CAPABILITY: &str = "board";
/// The refresh's name in the notice that reports a refusal.
const REFRESH: &str = "Board refresh";
/// What Plan board tasks from this conversation puts in the draft, as the terminal UI does.
const PLAN_DRAFT: &str =
    "Split the remaining work into board tasks and propose them with board_plan.";

/// The Board tab's state on this device, per session.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BoardUi {
    /// The task the keys stand on, by id.
    pub selected: Option<String>,
    /// The proposed tasks whose briefs are open, by key.
    pub opened: BTreeSet<String>,
    /// The build record shows in the pager over the board.
    pub record: bool,
    /// Briefs whose read met a changed proposal and had the board read again, by key: each is
    /// read again on its own only once, so a proposal that keeps changing cannot loop.
    pub conflicted: BTreeSet<String>,
}

impl BoardUi {
    pub const EMPTY: Self = Self {
        selected: None,
        opened: BTreeSet::new(),
        record: false,
        conflicted: BTreeSet::new(),
    };
}

/// What the Board tab does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoardAction {
    /// Move the keys one task down (1) or up (-1).
    Step { session: Id, delta: i8 },
    /// Ask the host to read the board again (`board_refresh`); `show` also opens the tab.
    Refresh { session: Id, show: bool },
    /// Open or fold a proposed task's brief; reopening one whose read failed reads it again.
    Fold { session: Id, key: String },
    /// Read the rest of a proposed task's brief again (Retry).
    Brief { session: Id, key: String },
    /// The proposal changed while a brief was read: read the board and the brief again, once.
    Conflict { session: Id, key: String },
    /// Open the build record in the pager (on the Board tab), or close it.
    Record { session: Id, open: bool },
    /// Plan board tasks from this conversation…: the request goes into the draft, not sent.
    PlanRequest(Id),
}

/// A control of the tab: it runs its action, or stays drawn muted with why it cannot.
#[derive(Debug, Clone, PartialEq)]
pub struct BoardControl {
    /// `btn-board-<slug>`.
    pub id: &'static str,
    pub label: &'static str,
    /// The key the terminal UI gives it.
    pub key: &'static str,
    pub action: Action,
    pub unavailable: Option<String>,
    /// Sent and not answered yet.
    pub busy: bool,
}

/// One task this build filed.
#[derive(Debug, Clone, PartialEq)]
pub struct BoardRow {
    /// `task-<id>`.
    pub id: String,
    pub task: Id,
    pub title: String,
    /// `running`, `blocked`, `landed`.
    pub word: &'static str,
    /// `○ ✓ ▲ ✗`; none while the spinner stands for it.
    pub glyph: Option<&'static str>,
    pub spinner: bool,
    pub hue: Hue,
    /// What it does or waits on now: `waits T-41`, `needs a decision`.
    pub now: String,
    /// `depends on T-41, T-42`.
    pub depends: Option<String>,
    /// The host's detail, when `now` does not already say it.
    pub detail: Option<String>,
    /// `12m ago`.
    pub updated: Option<String>,
    pub selected: bool,
    /// Opens the task's page (`Live::Task`).
    pub open: Action,
}

/// The plan the build proposes, waiting for the owner.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanCard {
    pub title: String,
    /// `for corral · 4 tasks · 3 dependencies`.
    pub facts: String,
    pub why: String,
    pub tasks: Vec<PlanTask>,
    /// Docks the plan's choice (File, File and start, Revise).
    pub review: BoardControl,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlanTask {
    /// `plan-task-<key>`.
    pub id: String,
    pub key: String,
    pub title: String,
    /// `feature · M · deep`.
    pub facts: Option<String>,
    /// `after T1, T2`.
    pub depends: Option<String>,
    /// The brief's first line while folded.
    pub preview: Option<String>,
    /// The brief while open: the whole one once read, else what the board reply carried.
    pub body: Option<String>,
    /// How much the host kept back: before the owner asks for the rest, or while it cannot be read.
    pub more: Option<String>,
    /// The read of the rest: under way, or why it failed with a Retry.
    pub read: Option<BriefRead>,
    pub open: bool,
    pub toggle: Option<Action>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BoardTab {
    pub part: PartBody,
    /// `From this session · 2 running · 1 in gate or review · 3/5 landed · 1 blocked`.
    pub counts: Option<(String, Hue)>,
    /// The host serves the part but has no board to read now, with its reason.
    pub unavailable: Option<String>,
    pub plan: Option<PlanCard>,
    pub rows: Vec<BoardRow>,
    /// `Nothing from this session is on the board.`
    pub empty: Option<&'static str>,
    pub refresh: BoardControl,
    pub request: BoardControl,
    pub record: Action,
    /// The build record, over the board while open.
    pub pager: Option<RecordPager>,
}

pub fn target(cx: &Cx<'_>) -> DetailRef {
    DetailRef::BuildBoard {
        id: cx.id.to_string(),
    }
}

/// The board last read for session `id`.
fn loaded<'f>(feed: &'f Feed, id: &str) -> Option<&'f BuildBoard> {
    match feed
        .detail(&DetailRef::BuildBoard { id: id.to_owned() })?
        .body()?
    {
        DetailBody::BuildBoard(board) => Some(board),
        _ => None,
    }
}

/// A task's stage in the terminal UI's words, glyph and tone; `tasks` say which of its
/// dependencies landed.
struct Stage {
    word: &'static str,
    glyph: Option<&'static str>,
    spinner: bool,
    hue: Hue,
    now: String,
}

fn stage(task: &BoardTask, tasks: &[BoardTask]) -> Stage {
    let detail = task
        .detail
        .as_deref()
        .map(str::trim)
        .filter(|detail| !detail.is_empty());
    let said = |fallback: &str| detail.map_or_else(|| fallback.to_owned(), str::to_owned);
    let quiet = |word, now: String| Stage {
        word,
        glyph: Some("○"),
        spinner: false,
        hue: Hue::Neutral,
        now,
    };
    let working = |word, hue, now: String| Stage {
        word,
        glyph: None,
        spinner: true,
        hue,
        now,
    };
    match task.phase.as_str() {
        "backlog" => quiet("backlog", "filed, not started".into()),
        "waiting" => {
            // A dependency this session did not file is unknown here, so it still counts as
            // waited for.
            let waits: Vec<&str> = task
                .depends
                .iter()
                .filter(|id| {
                    tasks
                        .iter()
                        .find(|other| other.id == **id)
                        .is_none_or(|other| other.phase != "landed")
                })
                .map(String::as_str)
                .collect();
            quiet(
                "waiting",
                if waits.is_empty() {
                    said("waits for its dependencies")
                } else {
                    format!("waits {}", waits.join(", "))
                },
            )
        }
        "queued" => quiet("queued", "waits for a slot".into()),
        "running" => working("running", Hue::Worker, said("an agent works on it")),
        "gating" => working("gate", Hue::Corral, said("the gate checks the candidate")),
        "reviewing" => working("review", Hue::Corral, said("independent review")),
        "landed" => Stage {
            word: "landed",
            glyph: Some("✓"),
            spinner: false,
            hue: Hue::Ok,
            now: "on the target branch".into(),
        },
        "blocked" => Stage {
            word: "blocked",
            glyph: Some("▲"),
            spinner: false,
            hue: Hue::You,
            now: said("blocked without a stated reason"),
        },
        "failed" => Stage {
            word: "failed",
            glyph: Some("✗"),
            spinner: false,
            hue: Hue::Fail,
            now: said("failed without a stated reason"),
        },
        _ => quiet(
            "unknown",
            said("the host names a stage this app does not know"),
        ),
    }
}

fn phases(board: &BuildBoard, wanted: &[&str]) -> usize {
    board
        .tasks
        .iter()
        .filter(|task| wanted.contains(&task.phase.as_str()))
        .count()
}

/// `From this session · 2 running · 1 in gate or review · 3/5 landed · 1 blocked`.
fn counts(board: &BuildBoard) -> (String, Hue) {
    let blocked = phases(board, &["blocked"]);
    let failed = phases(board, &["failed"]);
    let mut words = format!(
        "From this session · {} running · {} in gate or review · {}/{} landed",
        phases(board, &["running"]),
        phases(board, &["gating", "reviewing"]),
        phases(board, &["landed"]),
        board.tasks.len()
    );
    if blocked > 0 {
        words.push_str(&format!(" · {blocked} blocked"));
    }
    if failed > 0 {
        words.push_str(&format!(" · {failed} failed"));
    }
    let hue = if blocked + failed > 0 {
        Hue::Held
    } else {
        Hue::Neutral
    };
    (words, hue)
}

/// `3 open · 1 blocked · plan waiting`, as the host's board brief counts them.
pub fn summary(cx: &Cx<'_>) -> (String, Hue) {
    let Some(board) = cx.state.and_then(|state| state.board) else {
        return match loaded(cx.feed, cx.id.as_str()) {
            Some(board) => counts(board),
            None => ("board not counted yet".into(), Hue::Neutral),
        };
    };
    let mut parts = vec![format!("{} open", board.open)];
    if board.blocked > 0 {
        parts.push(format!("{} blocked", board.blocked));
    }
    if board.failed > 0 {
        parts.push(format!("{} failed", board.failed));
    }
    if board.proposed {
        parts.push("plan waiting".into());
    }
    let hue = if board.blocked + board.failed > 0 || board.proposed {
        Hue::Held
    } else {
        Hue::Neutral
    };
    (parts.join(" · "), hue)
}

/// The tab's mark: tasks not landed, `!` when one is blocked or failed, `◆` while a plan waits
/// and nothing is open. The host's brief counts them; before it does, the board last read.
pub fn badge(cx: &Cx<'_>) -> Option<Badge> {
    let (open, alarmed, proposed) = match cx.state.and_then(|state| state.board) {
        Some(board) => (board.open, board.blocked + board.failed > 0, board.proposed),
        None => {
            let board = loaded(cx.feed, cx.id.as_str())?;
            let open = board.tasks.len() - phases(board, &["landed"]);
            (
                u32::try_from(open).unwrap_or(u32::MAX),
                phases(board, &["blocked", "failed"]) > 0,
                board.proposed.is_some(),
            )
        }
    };
    if alarmed {
        Some(Badge {
            text: format!("{open} !"),
            hue: Hue::Held,
        })
    } else if proposed && open == 0 {
        Some(Badge {
            text: "◆".into(),
            hue: Hue::Held,
        })
    } else {
        count(open)
    }
}

/// The board-plan card the host raised, by id.
fn plan_card(cx: &Cx<'_>) -> Option<String> {
    cx.state?
        .cards
        .iter()
        .find(|brief| brief.kind == CardKind::BoardPlan)
        .map(|brief| brief.id.clone())
}

/// Why the host cannot run a board action now.
fn blocked(cx: &Cx<'_>) -> Option<String> {
    if cx.host != Host::Running {
        return Some("the host is not running; resume it first".into());
    }
    match cx.can(CAPABILITY) {
        Can::Unavailable(why) => Some(why),
        Can::Absent => Some("this build has no board connection".into()),
        Can::Yes | Can::Unknown => None,
    }
}

/// A board refresh this page sent is not answered yet.
fn refreshing(cx: &Cx<'_>) -> bool {
    cx.ui.dock.pending.iter().any(|(key, pending)| {
        matches!(pending, Pending::Action { what } if *what == REFRESH)
            && cx
                .feed
                .operations
                .get(key)
                .is_none_or(|operation| operation.loading)
    })
}

fn refresh(cx: &Cx<'_>, show: bool) -> BoardControl {
    BoardControl {
        id: "btn-board-refresh",
        label: "Refresh",
        key: "r",
        action: Action::Board(BoardAction::Refresh {
            session: cx.id.clone(),
            show,
        }),
        unavailable: blocked(cx),
        busy: refreshing(cx),
    }
}

fn request(cx: &Cx<'_>) -> BoardControl {
    BoardControl {
        id: "btn-board-plan-request",
        label: "Plan board tasks from this conversation…",
        key: "n",
        action: Action::Board(BoardAction::PlanRequest(cx.id.clone())),
        unavailable: (cx.host != Host::Running)
            .then(|| "the host is not running; resume it first".into()),
        busy: false,
    }
}

fn review(cx: &Cx<'_>) -> BoardControl {
    let card = plan_card(cx);
    BoardControl {
        id: "btn-board-review-plan",
        label: "Review plan",
        key: "p",
        action: Action::Dock(DockAction::Bring {
            session: cx.id.clone(),
            card: card.clone().unwrap_or_default(),
        }),
        unavailable: card
            .is_none()
            .then(|| "the host has not asked for your answer yet".into()),
        busy: false,
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// A fresh read of `target`, past any copy this device holds.
fn read(target: DetailRef) -> Effect {
    Effect::Detail(DetailGet {
        target,
        etag: None,
        page: None,
    })
}

fn plan(cx: &Cx<'_>, plan: &BoardPlan) -> PlanCard {
    let edges: usize = plan.tasks.iter().map(|task| task.depends.len()).sum();
    let mut facts = Vec::new();
    if let Some(project) = &plan.project {
        facts.push(format!("for {project}"));
    }
    facts.push(plural(plan.tasks.len(), "task", "tasks"));
    if edges > 0 {
        facts.push(plural(edges, "dependency", "dependencies"));
    }
    let tasks = plan
        .tasks
        .iter()
        .map(|task| {
            let open = cx.ui.board.opened.contains(&task.key);
            let body = task.body.trim();
            let shown = plan_brief::shown(cx, task, open);
            let kind: Vec<&str> = [&task.kind, &task.size, &task.profile]
                .into_iter()
                .filter_map(|fact| fact.as_deref())
                .collect();
            PlanTask {
                id: format!("plan-task-{}", task.key),
                key: task.key.clone(),
                title: task.title.clone(),
                facts: (!kind.is_empty()).then(|| kind.join(" · ")),
                depends: (!task.depends.is_empty())
                    .then(|| format!("after {}", task.depends.join(", "))),
                preview: (!open)
                    .then(|| body.lines().map(str::trim).find(|line| !line.is_empty()))
                    .flatten()
                    .map(str::to_owned),
                body: shown.body,
                more: shown.more,
                read: shown.read,
                open,
                toggle: (!body.is_empty()).then(|| {
                    Action::Board(BoardAction::Fold {
                        session: cx.id.clone(),
                        key: task.key.clone(),
                    })
                }),
            }
        })
        .collect();
    PlanCard {
        title: plan.title.clone(),
        facts: facts.join(" · "),
        why: plan.why.trim().to_owned(),
        tasks,
        review: review(cx),
    }
}

fn rows(cx: &Cx<'_>, board: &BuildBoard) -> Vec<BoardRow> {
    board
        .tasks
        .iter()
        .map(|task| {
            let stage = stage(task, &board.tasks);
            let detail = task
                .detail
                .as_deref()
                .map(str::trim)
                .filter(|detail| !detail.is_empty() && *detail != stage.now)
                .map(str::to_owned);
            let updated = crate::model::agents::unix(&task.updated_at)
                .filter(|_| cx.feed.now > 0)
                .map(|at| format!("{} ago", crate::model::work::age(Some(at), cx.feed.now)));
            BoardRow {
                id: format!("task-{}", task.id),
                task: Id::from(task.id.as_str()),
                title: task.title.clone(),
                word: stage.word,
                glyph: stage.glyph,
                spinner: stage.spinner,
                hue: stage.hue,
                now: stage.now,
                depends: (!task.depends.is_empty())
                    .then(|| format!("depends on {}", task.depends.join(", "))),
                detail,
                updated,
                selected: cx.ui.board.selected.as_deref() == Some(task.id.as_str()),
                open: Action::OpenLive(Live::Task(Id::from(task.id.as_str()))),
            }
        })
        .collect()
}

pub fn board_tab(cx: &Cx<'_>) -> BoardTab {
    let (summary, hue) = summary(cx);
    let board = loaded(cx.feed, cx.id.as_str());
    let pager = cx.ui.board.record.then(|| {
        record::pager(
            cx,
            Action::Board(BoardAction::Record {
                session: cx.id.clone(),
                open: false,
            }),
        )
    });
    BoardTab {
        part: part(cx, Ok(target(cx)), Some(CAPABILITY), summary, hue),
        counts: board.filter(|board| !board.tasks.is_empty()).map(counts),
        unavailable: board.and_then(|board| board.unavailable.clone()),
        plan: board
            .and_then(|board| board.proposed.as_ref())
            .map(|proposed| plan(cx, proposed)),
        rows: board.map_or_else(Vec::new, |board| rows(cx, board)),
        empty: board
            .filter(|board| {
                board.tasks.is_empty() && board.proposed.is_none() && board.unavailable.is_none()
            })
            .map(|_| "Nothing from this session is on the board."),
        refresh: refresh(cx, false),
        request: request(cx),
        record: Action::Board(BoardAction::Record {
            session: cx.id.clone(),
            open: true,
        }),
        pager,
    }
}

/// What the tab reads while it shows: the board, the rest of each open clipped brief, and the
/// record while its pager is open.
pub fn fetches(cx: &Cx<'_>) -> Vec<Action> {
    let briefs = loaded(cx.feed, cx.id.as_str())
        .and_then(|board| board.proposed.as_ref())
        .map_or_else(Vec::new, |plan| plan_brief::fetches(cx, &plan.tasks));
    load(cx, Ok(target(cx)), Some(CAPABILITY))
        .into_iter()
        .chain(briefs)
        .chain(
            cx.ui
                .board
                .record
                .then(|| record::fetches(cx, true))
                .flatten(),
        )
        .collect()
}

/// The build record's read on any tab, once the build's exit is under way: the exit's result
/// card lists its gate attempts and review rounds. The open pager reads it itself.
pub fn record_fetches(cx: &Cx<'_>) -> Option<Action> {
    let pager = cx.ui.tab == Tab::Board && cx.ui.board.record;
    (!pager).then(|| record::fetches(cx, false)).flatten()
}

pub(super) fn act(state: &mut State, action: BoardAction, feed: &Feed) -> Vec<Effect> {
    match action {
        BoardAction::Step { session, delta } => {
            let ids: Vec<String> = loaded(feed, session.as_str())
                .map(|board| board.tasks.iter().map(|task| task.id.clone()).collect())
                .unwrap_or_default();
            let ui = &mut state.ui_mut(&session).board;
            let at = ui
                .selected
                .as_ref()
                .and_then(|selected| ids.iter().position(|id| id == selected));
            let next = match at {
                None => 0,
                Some(at) if delta < 0 => at.saturating_sub(1),
                Some(at) => (at + 1).min(ids.len().saturating_sub(1)),
            };
            ui.selected = ids.get(next).cloned();
            Vec::new()
        }
        BoardAction::Refresh { session, show } => {
            let mut effects = if show {
                state.act(
                    Action::Tab {
                        session: session.clone(),
                        tab: Tab::Board,
                    },
                    feed,
                )
            } else {
                Vec::new()
            };
            let key = state.ids.next_id();
            state
                .ui_mut(&session)
                .dock
                .pending
                .insert(key.clone(), Pending::Action { what: REFRESH });
            effects.push(Effect::SessionAction {
                key,
                request: SessionActionRequest {
                    id: session.to_string(),
                    action: SessionAction::BoardRefresh,
                },
            });
            effects
        }
        BoardAction::Fold { session, key } => {
            let brief = plan_brief::target(session.as_str(), &key);
            let failed = feed
                .detail(&brief)
                .is_some_and(|cache| cache.error.is_some() && !cache.loading);
            let board = &mut state.ui_mut(&session).board;
            if board.opened.remove(&key) {
                return Vec::new();
            }
            board.opened.insert(key.clone());
            if !failed {
                return Vec::new();
            }
            board.conflicted.remove(&key);
            vec![read(brief)]
        }
        BoardAction::Brief { session, key } => {
            state.ui_mut(&session).board.conflicted.remove(&key);
            vec![read(plan_brief::target(session.as_str(), &key))]
        }
        BoardAction::Conflict { session, key } => {
            state.ui_mut(&session).board.conflicted.insert(key.clone());
            vec![
                read(DetailRef::BuildBoard {
                    id: session.to_string(),
                }),
                read(plan_brief::target(session.as_str(), &key)),
            ]
        }
        BoardAction::Record { session, open } => {
            let effects = if open {
                state.act(
                    Action::Tab {
                        session: session.clone(),
                        tab: Tab::Board,
                    },
                    feed,
                )
            } else {
                Vec::new()
            };
            state.ui_mut(&session).board.record = open;
            effects
        }
        BoardAction::PlanRequest(session) => {
            let now = feed.now;
            let ui = state.ui_mut(&session);
            ui.draft = if ui.draft.trim().is_empty() {
                PLAN_DRAFT.to_owned()
            } else {
                format!("{}\n\n{PLAN_DRAFT}", ui.draft.trim_end())
            };
            ui.focus = Focus::Composer;
            ui.notices.push(
                Hue::Neutral,
                "The request is in your message; send it when it reads right",
                now,
            );
            Vec::new()
        }
    }
}

/// The build's rows in the session's action catalog: Refresh board (`/board`), Review the
/// proposed board plan, Plan board tasks from this conversation…, Build record.
pub fn actions(cx: &Cx<'_>) -> Option<ActionGroup> {
    if *cx.role != SessionRole::Build
        || !matches!(cx.can(CAPABILITY), Can::Yes | Can::Unavailable(_))
    {
        return None;
    }
    let control = |slug: &str, label: &str, control: BoardControl| {
        let mut found = row(slug, label, Some(control.key), control.action);
        found.unavailable = control.unavailable;
        found
    };
    let mut refresh = control("board-refresh", "Refresh board", refresh(cx, true));
    refresh.command = Some("/board");
    let mut rows = vec![refresh];
    let proposed = cx
        .state
        .and_then(|state| state.board)
        .is_some_and(|board| board.proposed)
        || plan_card(cx).is_some();
    if proposed {
        rows.push(control(
            "board-review-plan",
            "Review the proposed board plan",
            review(cx),
        ));
    }
    rows.push(control(
        "board-plan-request",
        "Plan board tasks from this conversation…",
        request(cx),
    ));
    rows.push(row(
        "build-record",
        "Build record",
        None,
        Action::Board(BoardAction::Record {
            session: cx.id.clone(),
            open: true,
        }),
    ));
    Some(ActionGroup {
        title: "Build",
        rows,
    })
}

/// The tab's own keys, taken only while the board has the keys.
pub fn keys() -> Vec<KeyBinding> {
    vec![
        bind("j", "j", "next task", KeyAction::Part),
        bind("k", "k", "previous task", KeyAction::Part),
        bind("down", "↓", "next task", KeyAction::Part),
        bind("up", "↑", "previous task", KeyAction::Part),
        bind("enter", "⏎", "open task", KeyAction::Part),
        bind("r", "r", "refresh board", KeyAction::Part),
        bind("p", "p", "review plan", KeyAction::Part),
        bind("n", "n", "plan board tasks", KeyAction::Part),
        bind("q", "q", "close the record", KeyAction::Part),
    ]
}

pub fn press(key: &str, cx: &Cx<'_>) -> Option<Action> {
    if cx.ui.tab != Tab::Board || cx.ui.focus == Focus::Composer {
        return None;
    }
    let session = cx.id.clone();
    let runs = |control: BoardControl| control.unavailable.is_none().then_some(control.action);
    if cx.ui.board.record {
        return (key == "q").then_some(Action::Board(BoardAction::Record {
            session,
            open: false,
        }));
    }
    match key {
        "j" | "down" => Some(Action::Board(BoardAction::Step { session, delta: 1 })),
        "k" | "up" => Some(Action::Board(BoardAction::Step { session, delta: -1 })),
        "enter" => {
            let board = loaded(cx.feed, cx.id.as_str())?;
            let task = board
                .tasks
                .iter()
                .find(|task| cx.ui.board.selected.as_deref() == Some(task.id.as_str()))
                .or_else(|| board.tasks.first())?;
            Some(Action::OpenLive(Live::Task(Id::from(task.id.as_str()))))
        }
        "r" => {
            let control = refresh(cx, false);
            if control.busy {
                return None;
            }
            runs(control)
        }
        "p" => runs(review(cx)),
        "n" => runs(request(cx)),
        _ => None,
    }
}
