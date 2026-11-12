//! What Now shows besides the list in the Daemon data state: the stale flags, the selected task's
//! facts, the held tasks, the lower tab titles and the phone tab, all from the daemon's feed.

use crate::model::board::{Actor, Mark, STAGES, daemon_actor, money, stage_word};
use crate::model::daemon::{self, Link};
use crate::model::hue::Hue;
use crate::model::nav::Form;
use crate::model::text::{Rich, span};

use super::{
    EMPTY_BOARD, GroupKey, HoldRow, Inspector, LowTab, NO_BOARD, NowTab, PhoneNow, Stage, State,
    Status, daemon_list, daemon_selection, group_key, group_label,
};

/// Said by every panel whose content the daemon does not send.
pub const NOT_SERVED: &str = "corrald does not serve this yet.";
/// Said by the holds panel when the board holds no held task.
pub const NOTHING_HELD: &str = "Nothing is held.";

/// The daemon's facts about one task, laid out as the inspector's header and key/value rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskFacts {
    pub id: &'static str,
    pub title: String,
    /// The heading of the group the task is in: who acts next.
    pub who: &'static str,
    pub who_hue: Hue,
    /// Run, gate, review, land, with each stage's sha and age where the daemon knows them.
    pub stages: [Stage; 4],
    /// Labelled facts in display order; facts the daemon left out are absent.
    pub facts: Vec<(&'static str, Rich)>,
}

/// Stale flags in the Daemon state: a board received before the link went down stays, dimmed,
/// stamped with the last time the daemon was heard. The health strip carries the banner.
pub fn daemon_status(feed: &daemon::Feed) -> Status {
    let dimmed = feed.link == Link::Down && feed.board.complete;
    Status {
        dimmed,
        as_of: feed
            .synced
            .as_ref()
            .filter(|_| dimmed)
            .map(|clock| format!("as of {clock}")),
        banner: None,
    }
}

/// The inspector in the Daemon state: the selected task's facts on Story; the other tabs, which
/// the daemon does not serve, one quiet line.
pub fn daemon_inspector(state: &State, feed: &daemon::Feed, form: Form) -> Inspector {
    let board = &feed.board;
    if !board.complete {
        return match feed.link {
            Link::Down => Inspector::Quiet(NO_BOARD),
            Link::Connecting | Link::Up => Inspector::Loading,
        };
    }
    if state.now_tab != NowTab::Story {
        return Inspector::Quiet(NOT_SERVED);
    }
    daemon_selection(state, board, form)
        .and_then(|id| board.task(id))
        .map_or(Inspector::Quiet(EMPTY_BOARD), |task| {
            Inspector::Facts(task_facts(task, feed.now))
        })
}

/// A stage's detail: its candidate sha and how long ago it reached its state.
fn stage(task: &daemon::Task, index: usize, now: u64) -> Stage {
    let step = &task.rail[index];
    let active = step.mark == Mark::Active;
    let detail = [
        step.sha7.clone().unwrap_or_default(),
        daemon::age(step.at, now),
    ]
    .into_iter()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join(" · ");
    Stage {
        label: STAGES[index],
        mark: step.mark,
        word: stage_word(step.mark),
        detail,
        active,
        hue: match (active, index, daemon_actor(task.group)) {
            (false, _, _) => Hue::Neutral,
            (true, 0, Actor::You) => Hue::You,
            (true, 0, _) => Hue::Worker,
            (true, _, _) => Hue::Corral,
        },
    }
}

fn task_facts(task: &daemon::Task, now: u64) -> TaskFacts {
    let key = group_key(task.group);
    let mut facts: Vec<(&'static str, Rich)> = Vec::new();
    let mut doing = vec![span(task.now.clone())];
    let age = daemon::age(task.since, now);
    if !age.is_empty() {
        doing.push(span(format!(" · for {age}")).dim());
    }
    facts.push(("Now", doing));
    for (label, value) in [
        ("Plan", &task.plan),
        ("Project", &task.project),
        ("Goal", &task.goal),
    ] {
        if let Some(value) = value {
            facts.push((label, vec![span(value.clone())]));
        }
    }
    if let Some(agent) = &task.agent {
        let mut who = vec![span(agent.role.clone())];
        if let Some(profile) = &agent.profile {
            who.push(span(format!(" · {profile}")));
        }
        if let Some(session) = &agent.session {
            who.push(span(" · "));
            who.push(span(session.clone()).mono());
        }
        facts.push(("Agent", who));
    }
    if task.cost.is_some() {
        let amount = span(money(task.cost.map(|cost| cost as f32)));
        let word = if task.cost_estimated {
            " estimated"
        } else {
            " reported"
        };
        facts.push(("Cost", vec![amount, span(word).dim()]));
    }
    if let Some(reason) = &task.held_reason {
        facts.push(("Held", vec![span(reason.clone()).hue(Hue::Held)]));
    }
    if task.questions != 0 {
        facts.push((
            "Questions",
            vec![span(format!("{} open", task.questions)).hue(Hue::You)],
        ));
    }
    if !task.attention.is_empty() {
        facts.push(("Attention", vec![span(attention_kinds(&task.attention))]));
    }
    TaskFacts {
        id: task.id,
        title: task.title.clone(),
        who: group_label(key),
        who_hue: key.hue(),
        stages: std::array::from_fn(|index| stage(task, index, now)),
        facts,
    }
}

/// The kinds of attention keys (`T-935/escalation/2026-…Z` reads `escalation`), comma-separated,
/// each once in first-seen order; a key without that shape is shown whole.
fn attention_kinds(keys: &[String]) -> String {
    let mut kinds: Vec<&str> = Vec::new();
    for key in keys {
        let kind = key
            .split_once('/')
            .and_then(|(_, rest)| rest.split_once('/'))
            .map_or(key.as_str(), |(kind, _)| kind);
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    kinds.join(", ")
}

/// The held tasks of the daemon's board, in the hold table's columns: how long ago the task
/// entered its state, the id, the title with the held reason dimmed after it.
pub fn daemon_holds(feed: &daemon::Feed) -> Vec<HoldRow> {
    feed.board
        .tasks()
        .iter()
        .filter(|task| group_key(task.group) == GroupKey::Held)
        .map(|task| {
            let mut text = vec![span(task.title.clone())];
            if let Some(reason) = &task.held_reason {
                text.push(span(format!(" · {reason}")).dim());
            }
            HoldRow {
                id: task.id,
                since: daemon::age(task.since, feed.now),
                title: task.title.clone(),
                mark: Mark::Na,
                text,
            }
        })
        .collect()
}

/// A lower dock tab's title in the Daemon state: no fixture clock or count; Parked holds the
/// board's held tasks and counts them once a board has arrived.
pub fn daemon_lower_label(tab: LowTab, board: &daemon::Board) -> String {
    match tab {
        LowTab::Since => "Since your last visit".into(),
        LowTab::Parked if board.complete => {
            format!("Held {}", board.count(daemon::Group::Held))
        }
        LowTab::Parked => "Held".into(),
    }
}

/// The phone Now tab in the Daemon state: the daemon's list with no fixture summary chips.
pub fn daemon_phone_now(state: &State, feed: &daemon::Feed) -> PhoneNow {
    PhoneNow {
        title: "Now",
        chips: &[],
        summary_action: None,
        list: daemon_list(state, feed, Form::Phone),
        status: daemon_status(feed),
    }
}
