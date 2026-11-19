//! The Proof tab of a task-bound session: the gate and review checks of the run's task, as the
//! engine records them (`DetailRef::Review`), read as Review reads them: the newest gate and
//! review in the readiness chain's words (`review::checks`), the earlier rounds with each
//! reviewer's notes, and the task's artifacts (`work::live_evidence`). Links lead out to the
//! Artifacts finder narrowed to the task and to the candidate in Review. A check is the engine's
//! result on one commit, never the owner's acceptance. A session with no task run has no Proof
//! tab at all.

use corral_protocol::detail::DetailRef;
use corral_types::{Check, CheckResult};

use crate::model::daemon::{self, Feed};
use crate::model::effect::Effect;
use crate::model::hue::Hue;
use crate::model::review::{ReadyLink, Rounds};
use crate::model::work::{detail_task, live_evidence, loaded_task};
use crate::model::{Id, Live};

use super::actions::{ActionGroup, row};
use super::keys::{KeyAction, KeyBinding, bind};
use super::read_state::ReadState;
use super::tabs::{Badge, PartBody, load, part};
use super::task::{TaskLink, bound};
use super::{Action, Cx, Focus, State, Tab};

/// The Proof tab's state on this device, per session.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProofUi {}

impl ProofUi {
    pub const EMPTY: Self = Self {};
}

/// What the Proof tab does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProofAction {
    /// The Artifacts finder, narrowed to the task's artifacts.
    Artifacts(Id),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProofTab {
    pub part: PartBody,
    /// `gate ✓`, `reviewed`: the newest checks in words.
    pub chips: Vec<(String, Hue)>,
    /// The newest gate and review, as Review's readiness chain says them.
    pub checks: Vec<ReadyLink>,
    /// Earlier rounds and the reviewer that stays across them.
    pub rounds: Option<Rounds>,
    /// What a check is, and is not.
    pub note: &'static str,
    /// Each artifact's title and its reference with its kind.
    pub artifacts: Vec<(String, String)>,
    pub artifacts_empty: Option<&'static str>,
    pub links: Vec<TaskLink>,
}

/// The run's task's checks; `Err` when the session has no task run.
pub fn target(cx: &Cx<'_>) -> Result<DetailRef, String> {
    match bound(cx) {
        Some(task) => Ok(DetailRef::Review {
            task: task.to_string(),
        }),
        None => Err("this session has no task run".into()),
    }
}

/// `Gate and review checks of T-12`.
pub fn summary(cx: &Cx<'_>) -> String {
    match cx.task {
        Some(task) => format!("Gate and review checks of {task}"),
        None => "no task".into(),
    }
}

/// The task as Review reads it: the board's row, else its record read by id (a closed task).
fn with_task<T>(feed: &Feed, id: &Id, read: impl FnOnce(&daemon::Task) -> T) -> Option<T> {
    match feed.board.task(id) {
        Some(task) => Some(read(task)),
        None => loaded_task(feed, id.as_str()).map(|detail| read(&detail_task(detail))),
    }
}

/// `gate ✓`, `gate ✗`, `no gate`, then `reviewed` or `changes asked` when a review was recorded.
fn chips(gate: Option<&Check>, review: Option<&Check>) -> Vec<(String, Hue)> {
    let mut chips = vec![match gate.map(|gate| gate.result) {
        Some(CheckResult::Pass) => ("gate ✓".to_owned(), Hue::Ok),
        Some(CheckResult::Fail) => ("gate ✗".to_owned(), Hue::Fail),
        None => ("no gate".to_owned(), Hue::Neutral),
    }];
    match review.map(|review| review.result) {
        Some(CheckResult::Pass) => chips.push(("reviewed".into(), Hue::Ok)),
        Some(CheckResult::Fail) => chips.push(("changes asked".into(), Hue::Fail)),
        None => {}
    }
    chips
}

fn links(task: &Id) -> Vec<TaskLink> {
    vec![
        TaskLink {
            id: "btn-proof-artifacts",
            label: "Artifacts",
            key: Some("a"),
            action: Action::Proof(ProofAction::Artifacts(task.clone())),
        },
        TaskLink {
            id: "btn-proof-review",
            label: "Open in Review",
            key: None,
            action: Action::OpenLive(Live::Review(task.clone())),
        },
    ]
}

pub fn proof_tab(cx: &Cx<'_>) -> ProofTab {
    let part = part(cx, target(cx), None, summary(cx), Hue::Neutral);
    let Some(task) = bound(cx) else {
        return ProofTab {
            part,
            chips: Vec::new(),
            checks: Vec::new(),
            rounds: None,
            note: "",
            artifacts: Vec::new(),
            artifacts_empty: None,
            links: Vec::new(),
        };
    };
    let (chips, checks, rounds) = with_task(cx.feed, task, |shown| {
        let (checks, rounds) = crate::model::review::checks(cx.feed, shown);
        (
            chips(shown.row.gate.as_ref(), shown.row.review.as_ref()),
            checks,
            rounds,
        )
    })
    .unwrap_or_default();
    let evidence = live_evidence(cx.feed, task.as_str());
    ProofTab {
        part,
        chips,
        checks,
        rounds,
        note: "A check is the engine's result on one commit, never your acceptance.",
        artifacts: evidence.rows,
        artifacts_empty: evidence.empty,
        links: links(task),
    }
}

/// The badge: `proof?` once a read of the checks failed, else the newest gate in words.
pub fn badge(cx: &Cx<'_>) -> Option<Badge> {
    let target = target(cx).ok()?;
    if matches!(
        ReadState::of(cx.feed.detail(&target), cx.feed),
        ReadState::Failed(_)
    ) {
        return Some(Badge {
            text: "proof?".into(),
            hue: Hue::Held,
        });
    }
    let task = bound(cx)?;
    let chips = with_task(cx.feed, task, |shown| {
        chips(shown.row.gate.as_ref(), shown.row.review.as_ref())
    })?;
    let hue = if chips.iter().any(|(_, hue)| *hue == Hue::Fail) {
        Hue::Fail
    } else if chips.len() > 1 && chips.iter().all(|(_, hue)| *hue == Hue::Ok) {
        Hue::Ok
    } else {
        Hue::Neutral
    };
    Some(Badge {
        text: chips
            .into_iter()
            .map(|(words, _)| words)
            .collect::<Vec<_>>()
            .join(" · "),
        hue,
    })
}

/// What the tab reads while it shows: the checks and the task's record (its artifacts).
pub fn fetches(cx: &Cx<'_>) -> Vec<Action> {
    let record = bound(cx).map(|task| {
        Ok(DetailRef::Task {
            id: task.to_string(),
        })
    });
    load(cx, target(cx), None)
        .into_iter()
        .chain(record.and_then(|record| load(cx, record, None)))
        .collect()
}

pub(super) fn act(_: &mut State, action: ProofAction, _: &Feed) -> Vec<Effect> {
    match action {
        ProofAction::Artifacts(task) => vec![Effect::FindArtifacts(task)],
    }
}

/// The task's artifacts and its candidate in Review, among the Task rows.
pub fn actions(cx: &Cx<'_>) -> Option<ActionGroup> {
    let task = bound(cx)?;
    Some(ActionGroup {
        title: "Task",
        rows: vec![
            row(
                "proof-artifacts",
                format!("{task} artifacts"),
                Some("a"),
                Action::Proof(ProofAction::Artifacts(task.clone())),
            ),
            row(
                "proof-review",
                format!("Open {task} in Review"),
                None,
                Action::OpenLive(Live::Review(task.clone())),
            ),
        ],
    })
}

/// The tab's own keys, taken only while the Proof tab has the keys.
pub fn keys() -> Vec<KeyBinding> {
    vec![
        bind("r", "r", "reread proof", KeyAction::Part),
        bind("a", "a", "task artifacts", KeyAction::Part),
    ]
}

pub fn press(key: &str, cx: &Cx<'_>) -> Option<Action> {
    if cx.ui.tab != Tab::Proof || cx.ui.focus == Focus::Composer {
        return None;
    }
    match key {
        "r" => Some(Action::Refresh(target(cx).ok()?)),
        "a" => bound(cx).map(|task| Action::Proof(ProofAction::Artifacts(task.clone()))),
        _ => None,
    }
}
