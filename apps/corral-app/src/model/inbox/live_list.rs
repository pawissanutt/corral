//! The served Inbox list, changes since the last visit and the selected row's pane.

use corral_protocol::rows::{InboxKind, InboxRow, InboxState, PlanRow, PlanState};

use super::live::{
    age, clock, from_this_device, is_open, is_question, ordered, selected, sent_status, target,
};
use super::live_sheet::{handover, sheet};
use super::sheet::button;
use super::{
    Action, ButtonKind, Chip, EmptyState, FeedRow, Handover, InboxList, InboxPreview, ItemPreview,
    NeedYouItem, Notice, PlanPreview, QuestionState, State, phone,
};
use crate::model::act::ActStatus;
use crate::model::board::ChangeKind;
use crate::model::daemon::{Feed, Group};
use crate::model::effect::Env;
use crate::model::hue::Hue;
use crate::model::nav::Destination;
use crate::model::text::span;
use crate::model::{Id, Live};

/// Whether the first inbox snapshot is still on its way.
fn loading(feed: &Feed) -> bool {
    let topic = feed.topics.get("inbox");
    feed.inbox.is_empty() && topic.is_none_or(|topic| topic.loading && topic.error.is_none())
}

fn lead(row: &InboxRow) -> String {
    let from = row.from.trim();
    match row.kind {
        InboxKind::WorkerQuestion => "Worker asks".into(),
        InboxKind::PmAsk => "PM asks".into(),
        InboxKind::AssistantAsk if !from.is_empty() => format!("Assistant {from} asks"),
        InboxKind::AssistantAsk => "Assistant asks".into(),
        InboxKind::Prompt if !from.is_empty() => format!("Assistant {from} waits"),
        InboxKind::Prompt => "Assistant waits".into(),
        InboxKind::PlanAcceptance => "Plan awaits acceptance".into(),
        InboxKind::Review => "Review needs you".into(),
        InboxKind::Stuck => "Stuck".into(),
        InboxKind::Escalated => "Escalated".into(),
        InboxKind::OwnerDecision => "Owner decision".into(),
        InboxKind::PmAction => "PM's move".into(),
        InboxKind::Other => "Needs you".into(),
    }
}

fn plan_of<'a>(feed: &'a Feed, row: &InboxRow) -> Option<&'a PlanRow> {
    feed.plans.get(row.plan.as_deref().unwrap_or(&row.id))
}

fn record(feed: &Feed, row: &InboxRow) -> String {
    match (row.kind, plan_of(feed, row)) {
        (InboxKind::PlanAcceptance, Some(plan)) => match &plan.revision {
            Some(revision) => format!("{} {revision}", plan.id),
            None => plan.id.clone(),
        },
        _ => row.id.clone(),
    }
}

/// A row's question state from the row alone and this device's sent answer.
fn row_state(state: &State, feed: &Feed, row: &InboxRow) -> QuestionState {
    match &row.state {
        InboxState::Answered { via, .. } if from_this_device(feed, None, via) => {
            QuestionState::Mine
        }
        InboxState::Answered { .. } => QuestionState::Elsewhere,
        InboxState::Cancelled | InboxState::Expired => QuestionState::Closed,
        InboxState::Open | InboxState::Other => {
            let sent = state
                .desk
                .draft(&row.id)
                .and_then(|draft| draft.answer.as_ref());
            match sent_status(feed, sent) {
                Some(ActStatus::Unknown) => QuestionState::Uncertain,
                Some(status) if !status.settled() => QuestionState::Pending,
                Some(ActStatus::Done) => QuestionState::Mine,
                _ => QuestionState::Open,
            }
        }
    }
}

fn plan_state(state: PlanState) -> &'static str {
    match state {
        PlanState::Drafting => "drafting",
        PlanState::Reviewing => "in plan review",
        PlanState::PlanFailed => "plan failed",
        PlanState::Awaiting => "awaits acceptance",
        PlanState::Executing => "executing",
        PlanState::Approved => "approved",
        PlanState::Complete => "complete",
        PlanState::Cancelled => "cancelled",
        PlanState::Other => "state unknown",
    }
}

fn meta(state: &State, feed: &Feed, row: &InboxRow, status: Option<QuestionState>) -> String {
    match (&row.state, status) {
        (InboxState::Answered { by, via, at }, Some(QuestionState::Mine) | None) => {
            if from_this_device(feed, None, via) {
                format!("answered by you {}", clock(at, feed.now))
            } else {
                format!(
                    "answered by {} {}",
                    super::provenance(feed, by, None, via),
                    clock(at, feed.now)
                )
            }
        }
        (InboxState::Answered { by, via, at }, _) => {
            format!(
                "answered by {} {}",
                super::provenance(feed, by, None, via),
                clock(at, feed.now)
            )
        }
        (InboxState::Cancelled, _) => "cancelled".into(),
        (InboxState::Expired, _) => "expired".into(),
        (_, Some(QuestionState::Pending)) => "your answer is on its way to corrald".into(),
        (_, Some(QuestionState::Uncertain)) => "your answer's result is unknown".into(),
        (_, Some(QuestionState::Mine)) => "answered by you".into(),
        _ => open_meta(state, feed, row),
    }
}

fn open_meta(_state: &State, feed: &Feed, row: &InboxRow) -> String {
    let mut parts: Vec<String> = Vec::new();
    match row.kind {
        InboxKind::PlanAcceptance => match plan_of(feed, row) {
            Some(plan) => {
                parts.push(plan_state(plan.state).into());
                parts.push(format!("{} members", plan.members_total));
                if !plan.blockers.is_empty() {
                    parts.push(format!("{} blockers", plan.blockers.len()));
                }
            }
            None => parts.push("awaits acceptance".into()),
        },
        InboxKind::Prompt => parts.push("answers in its thread".into()),
        _ => {
            if let Some(task) = &row.task {
                parts.push(if row.blocking {
                    format!("{task} blocked")
                } else {
                    task.clone()
                });
            } else if row.blocking {
                parts.push("blocking".into());
            }
            if let Some(project) = &row.project {
                parts.push(project.clone());
            }
        }
    }
    if let Some(at) = &row.needed_by {
        parts.push(format!("needed by {}", clock(at, feed.now)));
    }
    if !row.answerable && is_question(row) {
        parts.push("answer in the pane".into());
    }
    if parts.is_empty() && !row.from.is_empty() {
        parts.push(format!("from {}", row.from));
    }
    parts.join(" · ")
}

fn item(state: &State, feed: &Feed, row: &InboxRow, chosen: Option<&str>, env: Env) -> NeedYouItem {
    let lead = lead(row);
    let id = record(feed, row);
    let age = age(row.asked_at.as_deref(), feed);
    let status = is_question(row).then(|| row_state(state, feed, row));
    let open = is_open(row) && status.is_none_or(|status| status == QuestionState::Open);
    let key = Id::new(&row.id);
    let mut heading = if phone(env) {
        let mut heading = vec![span(lead.clone())];
        if !age.is_empty() {
            heading.push(span(format!(" · {age}")));
        }
        if row.blocking && is_open(row) {
            heading.push(span(" · blocking"));
        }
        heading
    } else {
        vec![span(lead.clone()), span(" · "), span(id.clone()).mono()]
    };
    if !row.answerable && is_question(row) && phone(env) {
        heading.push(span(" · in pane").dim());
    }
    NeedYouItem {
        attention: super::attention_line(row),
        test_id: format!("inbox-{}", row.id),
        lead,
        id,
        heading,
        title: row.title.clone(),
        meta: meta(state, feed, row, status),
        age,
        state: status,
        hue: if open { Hue::You } else { Hue::Neutral },
        selected: !phone(env) && chosen == Some(row.id.as_str()),
        sample: None,
        action: if phone(env) {
            Action::OpenRow(key)
        } else {
            Action::Row(key)
        },
    }
}

fn change_kind(class: &str) -> ChangeKind {
    let class = class.to_ascii_lowercase();
    if ["question", "answer", "ask", "message", "followup"]
        .iter()
        .any(|word| class.contains(word))
    {
        ChangeKind::Message
    } else if ["start", "dispatch", "launch"]
        .iter()
        .any(|word| class.contains(word))
    {
        ChangeKind::Start
    } else {
        ChangeKind::Note
    }
}

fn empty_body(feed: &Feed, env: Env) -> String {
    if !feed.board.complete {
        return "Nothing in the inbox is waiting on you.".into();
    }
    let verifying = feed.board.count(Group::Verifying);
    let producing = feed.board.count(Group::Producing);
    // The Queued group also holds ideas and backlog nobody queued, as Now's "Upcoming" says.
    let upcoming = feed.board.count(Group::Queued);
    if phone(env) {
        format!(
            "Corral is verifying {verifying} · worker runs in progress {producing} · {upcoming} upcoming."
        )
    } else {
        format!(
            "Corral is verifying {verifying}, worker runs in progress {producing}, {upcoming} are upcoming."
        )
    }
}

fn refresh_error(feed: &Feed, retained: bool) -> Option<Notice> {
    let error = feed.topics.get("inbox")?.error.as_ref()?;
    let mut body = vec![
        span("Couldn't refresh the inbox.").bold(),
        span(" corrald returned "),
        span(error.code.clone()).mono(),
        span(format!(": {}.", error.message.trim_end_matches('.'))),
    ];
    if retained {
        body.push(span(match &feed.synced {
            Some(at) => format!(" Showing the last state received at {at}."),
            None => " Showing the last state received.".into(),
        }));
    }
    Some(Notice {
        stamp: None,
        hue: Hue::Fail,
        body,
        retry: None,
    })
}

/// The served list: needs-you rows in working order, then what changed since the last visit.
pub fn list(state: &State, feed: &Feed, env: Env) -> InboxList {
    if loading(feed) {
        return InboxList::Loading {
            skeleton_count: if phone(env) { 2 } else { 5 },
            blocks_per_item: if phone(env) { 3 } else { 2 },
            caption: super::LOADING_LIST,
        };
    }
    let rows = ordered(feed);
    let chosen = selected(&state.desk, &rows).map(|row| row.id.as_str());
    let items: Vec<NeedYouItem> = rows
        .iter()
        .map(|row| item(state, feed, row, chosen, env))
        .collect();
    let error = refresh_error(feed, !items.is_empty());
    let empty = (items.is_empty() && error.is_none()).then(|| EmptyState {
        title: "Nothing needs you.",
        body: empty_body(feed, env),
        action: Some(button(
            "Open Now",
            Action::Go(Destination::Now),
            ButtonKind::Ghost,
        )),
    });
    let seen = feed
        .seen
        .as_ref()
        .or(feed.session.as_ref().map(|session| &session.seen));
    let through = seen.map_or(0, |seen| seen.through);
    let visited = seen
        .and_then(|seen| seen.at.as_deref())
        .map(|at| clock(at, feed.now));
    let limit = if phone(env) { 6 } else { 8 };
    let feed_rows = feed
        .changes
        .values()
        .rev()
        .take(limit)
        .map(|line| {
            let task = line.task.as_deref().filter(|task| !task.is_empty());
            FeedRow {
                at: clock(&line.at, feed.now),
                id: task.unwrap_or_default().to_owned(),
                kind: change_kind(&line.class),
                text: line.text.clone(),
                test_id: match task {
                    Some(task) => format!("task-{task}"),
                    None => format!("change-{}", line.seq),
                },
                unseen: !phone(env) && line.seq > through,
                action: match task {
                    Some(task) => Action::Live(Live::Task(Id::new(task))),
                    None => Action::Go(Destination::Changes),
                },
            }
        })
        .collect();
    let unseen = crate::model::work::unseen_count(feed);
    InboxList::Ready {
        label: "Inbox",
        error,
        needs_title: "Needs you",
        needs_count: super::attention_counts(feed).inbox as usize,
        items,
        empty,
        since_title: match (&visited, phone(env)) {
            (Some(at), true) => format!("Since {at}"),
            _ => "Since your last visit".into(),
        },
        last_visit: visited,
        changes_count: unseen.count,
        feed: feed_rows,
        all_changes: button(
            match (phone(env), unseen.count, unseen.total) {
                (true, _, _) => "All".to_owned(),
                (false, 0, _) => "All changes".to_owned(),
                (false, n, true) => format!("All {n} changes"),
                (false, n, false) => format!("All changes · {n} loaded unseen"),
            },
            Action::Go(Destination::Changes),
            ButtonKind::Quiet,
        ),
    }
}

fn plan_preview(feed: &Feed, row: &InboxRow) -> PlanPreview {
    let plan = plan_of(feed, row);
    let id = plan.map_or_else(
        || row.plan.clone().unwrap_or_else(|| row.id.clone()),
        |plan| plan.id.clone(),
    );
    let mut chips = vec![Chip {
        label: "needs you",
        hue: Hue::You,
    }];
    if let Some(plan) = plan {
        chips.push(Chip {
            label: plan_state(plan.state),
            hue: Hue::Neutral,
        });
    }
    let mut consequence = Vec::new();
    match plan {
        Some(plan) => {
            consequence.push(span(format!(
                "Publishes {} member task{}",
                plan.members_total,
                if plan.members_total == 1 { "" } else { "s" }
            )));
            if let Some(delivery) = &plan.delivery {
                consequence.extend([span(" to "), span(delivery.clone()).bold()]);
            }
            consequence.push(span("."));
            if !plan.blockers.is_empty() {
                consequence.push(span(format!(" Blocked by: {}.", plan.blockers.join(", "))));
            }
        }
        None => consequence.push(span(
            "The plan's projection has not arrived; open the plan review for its members.",
        )),
    }
    PlanPreview {
        attention: super::attention_line(row),
        chips,
        record: record(feed, row),
        title: plan.map_or_else(|| row.title.clone(), |plan| plan.title.clone()),
        goal: None,
        consequence_title: "What accepting does",
        consequence,
        open: button(
            "Open plan review",
            Action::Live(Live::Plan(Id::new(&id))),
            ButtonKind::Primary,
        ),
    }
}

fn item_preview(state: &State, feed: &Feed, row: &InboxRow) -> ItemPreview {
    let mut facts = vec![lead(row)];
    let detail = open_meta(state, feed, row);
    if !detail.is_empty() {
        facts.push(detail);
    }
    if let Some(at) = &row.asked_at {
        facts.push(format!("since {}", clock(at, feed.now)));
    }
    let open = target(row).map(|live| {
        let label = match &live {
            Live::Review(task) => format!("Open review of {task}"),
            Live::Task(task) => format!("Open {task}"),
            Live::Plan(plan) => format!("Open plan {plan}"),
            Live::Session(session) => format!("Open session {session}"),
            Live::Thread(thread) => format!("Open thread {thread}"),
            Live::Question(key) => format!("Open {key}"),
        };
        button(label, Action::Live(live), ButtonKind::Primary)
    });
    ItemPreview {
        attention: super::attention_line(row),
        chips: vec![Chip {
            label: if is_open(row) { "needs you" } else { "settled" },
            hue: if is_open(row) { Hue::You } else { Hue::Neutral },
        }],
        record: row.id.clone(),
        title: row.title.clone(),
        facts,
        open,
    }
}

/// The selected row's pane on desktop and terminal.
pub fn preview(state: &State, feed: &Feed, env: Env) -> InboxPreview {
    if loading(feed) {
        return InboxPreview::Loading {
            skeleton_count: 3,
            caption: super::LOADING_ITEM,
        };
    }
    let rows = ordered(feed);
    let Some(row) = selected(&state.desk, &rows) else {
        return match refresh_error(feed, false) {
            Some(notice) => InboxPreview::Failed(notice),
            None => InboxPreview::Empty(EmptyState {
                title: "Inbox is clear",
                body: "Answered questions and accepted plans stay in Changes and Timeline. Nothing here is waiting on you.".into(),
                action: None,
            }),
        };
    };
    if is_question(row) {
        let key = Id::new(&row.id);
        return match handover(feed, &key) {
            Handover::Ready => InboxPreview::Question(sheet(state, feed, &key, env, false, false)),
            Handover::Loading { caption } => InboxPreview::Loading {
                skeleton_count: 3,
                caption,
            },
            Handover::Failed(notice) => InboxPreview::Failed(notice),
            Handover::Gone(empty) => InboxPreview::Empty(empty),
        };
    }
    match row.kind {
        InboxKind::Prompt if !row.from.is_empty() => InboxPreview::Prompt {
            thread: Id::new(&row.from),
            key: Id::new(&row.id),
        },
        InboxKind::PlanAcceptance => InboxPreview::Plan(plan_preview(feed, row)),
        _ => InboxPreview::Item(item_preview(state, feed, row)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unseen_uses_owner_relevant_count_not_the_event_sequence_gap() {
        let mut feed = Feed {
            changes_head: 90_000,
            changes_unseen: Some((0, 2)),
            ..Feed::default()
        };
        feed.topics.insert("inbox".into(), Default::default());
        let env = crate::model::health::daemon_env(&feed, crate::model::nav::Form::Terminal);
        for expected in [2, 0] {
            let view = list(&State::default(), &feed, env);
            let InboxList::Ready { changes_count, .. } = view else {
                panic!("inbox topic is loaded");
            };
            assert_eq!(changes_count, expected);
            assert_eq!(changes_count, crate::model::work::unseen_count(&feed).count);
            feed.changes_unseen = None;
        }
    }
}
