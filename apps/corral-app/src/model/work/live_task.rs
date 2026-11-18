//! The selected daemon task in the inspector and on the phone's task page: its served row (the
//! task, its live run, its gate and review checks, whether it is ready) with the task detail
//! corrald serves (runs, checks, artifacts, events), the detail's loading, stale and error
//! states, a Blocked task's reason, detail and next steps, every engine operation, and this
//! device's last operation on it with its outcome.

use corral_protocol::detail::{DetailBody, DetailRef, TaskDetail};
use corral_types::{Outcome, Role, Run, SessionRef, Status};

use crate::model::act::{self, ActStatus};
use crate::model::board::{Actor, Mark, STAGES, daemon_actor, stage_word};
use crate::model::daemon::{self, Feed, Link};
use crate::model::hue::Hue;
use crate::model::nav::Form;
use crate::model::projects::{NowGroup, ProjectFilter};
use crate::model::text::{Rich, span};
use crate::model::{Id, Live, Selection};

use super::{
    Button, ButtonKind, Dependency, EMPTY_BOARD, Inspector, NO_BOARD, NowTab, Press, Record, Stage,
    State, Tail, TailLine, daemon_selection_filtered, group_key, group_label, session_for, stamp,
    stamp_at, when_at,
};

/// The detail reference of a task.
pub fn task_ref(id: &str) -> DetailRef {
    DetailRef::Task { id: id.to_owned() }
}

/// The task detail corrald last served for `id`, when one has arrived.
pub fn loaded_task<'a>(feed: &'a Feed, id: &str) -> Option<&'a TaskDetail> {
    match feed.detail(&task_ref(id))?.body()? {
        DetailBody::Task(detail) => Some(detail),
        _ => None,
    }
}

/// A one-line state of a detail beside what it shows: loading, stale or failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailNote {
    pub text: String,
    pub hue: Hue,
}

/// The detail's state for a task, or `None` while it is current.
pub fn detail_note(feed: &Feed, id: &str) -> Option<DetailNote> {
    let cache = feed.detail(&task_ref(id));
    let has_body = cache.and_then(|cache| cache.body()).is_some();
    let as_of = cache
        .and_then(|cache| cache.pages.last())
        .map(|page| stamp(&page.as_of, feed))
        .filter(|stamp| !stamp.is_empty());
    let note = |text: String, hue| Some(DetailNote { text, hue });
    if let Some(error) = cache.and_then(|cache| cache.error.as_ref()) {
        let why = if error.code == "not_found" {
            format!("corrald has no detail for {id}.")
        } else if error.message.is_empty() {
            format!("Task detail unavailable ({}).", error.code)
        } else {
            format!("Task detail unavailable: {}", error.message)
        };
        return note(
            match (has_body, as_of) {
                (true, Some(at)) => format!("{why} Showing the detail as of {at}."),
                _ => why,
            },
            Hue::Fail,
        );
    }
    if !has_body {
        return if feed.link == Link::Down {
            note(
                "Task detail unavailable while corrald is unreachable.".into(),
                Hue::Neutral,
            )
        } else {
            note("Loading task detail…".into(), Hue::Neutral)
        };
    }
    match cache {
        Some(cache) if cache.stale || feed.link == Link::Down => note(
            as_of.map_or_else(
                || "Task detail may be out of date.".to_owned(),
                |at| format!("Task detail as of {at} · may be out of date"),
            ),
            Hue::Held,
        ),
        _ => None,
    }
}

/// A labelled value of the live story.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fact {
    Text(Rich),
    /// Task ids that open the task.
    Tasks(Vec<Dependency>),
    /// The owner's move, as one button.
    Answer(Button),
}

/// This device's last operation on the task, with its outcome as far as the device knows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentLine {
    pub text: String,
    pub hue: Hue,
}

/// Why a Blocked task stopped and what gets it moving: the engine's reason, its detail, the
/// reply box when the recommended step is a reply, and one button per other served next step,
/// the first one primary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockedCard {
    pub reason: String,
    pub detail: Option<String>,
    pub reply: Option<ReplyBox>,
    pub next: Vec<Button>,
}

/// The recommended reply step in place: what the owner types goes out as that step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyBox {
    pub task: Id,
    /// The step's place in the served `next` list.
    pub index: usize,
    /// The field's spoken name.
    pub label: String,
    pub placeholder: &'static str,
    /// The send button: the step as the refusal sheet would name it ("Reply", "Reply anyway").
    pub send: String,
}

impl ReplyBox {
    /// What pressing send runs with `text`.
    pub fn press(&self, text: String) -> Press {
        Press::Answer {
            task: self.task.clone(),
            index: self.index,
            text,
        }
    }
}

/// The selected daemon task's story.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveStory {
    pub id: String,
    pub title: String,
    /// The heading of the group the task is in: who moves next, or its closed status.
    pub who: &'static str,
    pub who_hue: Hue,
    /// `· plan T-885`, opening the parent task's plan.
    pub plan: Option<(String, Press)>,
    /// The changed-since-visit chip, when the changes feed holds an unseen event for it.
    pub changed: Option<String>,
    /// Words under the title; the engine serves none beyond the body.
    pub summary: Rich,
    /// The task body's Markdown, exactly as the engine stores it.
    pub body: Option<String>,
    /// Run, gate, review, land, with each stage's sha and age where the row records them.
    pub stages: [Stage; 4],
    /// A Blocked task's reason, detail and next steps.
    pub blocked: Option<BlockedCard>,
    /// Labelled facts in display order; facts corrald left out are absent.
    pub facts: Vec<(&'static str, Fact)>,
    /// Open session, Ask about this, Review candidate, and every engine operation.
    pub actions: Vec<Button>,
    pub note: Option<DetailNote>,
    pub sent: Option<SentLine>,
}

/// The selected daemon task's artifacts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveEvidence {
    pub title: String,
    /// Each artifact's title and its reference with its kind.
    pub rows: Vec<(String, String)>,
    pub empty: Option<&'static str>,
    pub note: Option<DetailNote>,
    /// The finder over every task's artifacts, narrowed to this one.
    pub find: Button,
}

/// The selected daemon task's run, session hand-off and terminal capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveSession {
    pub description: &'static str,
    pub open: Option<Button>,
    /// The run's identity, role, profile and timing.
    pub run: Rich,
    pub tail: Option<Tail>,
    pub empty: Option<&'static str>,
    pub note: Option<DetailNote>,
}

/// Said by the Evidence tab when the task has no artifacts.
pub const NO_ARTIFACTS: &str = "No artifacts recorded for this task.";
/// Said by the Session tab when the task has no run.
pub const NO_RUN: &str = "No run for this task yet.";
/// Said where the terminal would be when no capture of the run's session is loaded.
pub const NO_TAIL: &str = "No terminal capture loaded; open the session to see its terminal.";

/// The inspector in the Daemon state, for the selected task on the board.
pub fn daemon_inspector_filtered(
    state: &State,
    feed: &Feed,
    selected: &Selection,
    form: Form,
    filter: &ProjectFilter,
    grouping: NowGroup,
) -> Inspector {
    if let Some(line) = feed.device.board_notice() {
        return Inspector::Quiet(line);
    }
    if !feed.board.complete {
        if super::served_facts::board_problem(feed).is_some() {
            return Inspector::Quiet(super::served::BOARD_UNAVAILABLE);
        }
        return match feed.link {
            Link::Down => Inspector::Quiet(NO_BOARD),
            Link::Connecting | Link::Up => Inspector::Loading,
        };
    }
    if let Some(id) = super::off_board(selected, feed) {
        return super::off_board_inspector(state, feed, id, state.now_tab, form);
    }
    let Some(task) = daemon_selection_filtered(state, selected, feed, form, filter, grouping)
    else {
        return Inspector::Quiet(EMPTY_BOARD);
    };
    task_inspector(state, feed, task, state.now_tab, form)
}

/// One task's inspector tab, independent of which task the list selects (the phone page).
pub fn task_inspector(
    state: &State,
    feed: &Feed,
    task: &daemon::Task,
    tab: NowTab,
    form: Form,
) -> Inspector {
    match tab {
        NowTab::Story => Inspector::LiveStory(Box::new(live_story(state, feed, task, form))),
        NowTab::Evidence => Inspector::LiveEvidence(live_evidence(feed, &task.id)),
        NowTab::Session => Inspector::LiveSession(live_session(feed, task)),
    }
}

/// A stage's detail: its candidate sha and how long ago it reached its state; an active stage
/// takes the hue of whoever acts on it.
fn stage(step: &daemon::Step, index: usize, now: u64, actor: Actor) -> Stage {
    let active = step.mark == Mark::Active;
    let detail = [
        step.sha7.clone().unwrap_or_default(),
        super::when::age(step.at, now),
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
        hue: match (active, index, actor) {
            (false, _, _) => Hue::Neutral,
            (true, 0, Actor::You) => Hue::You,
            (true, 0, _) => Hue::Worker,
            (true, _, _) => Hue::Corral,
        },
    }
}

/// The run the task's facts describe: the live one on its row, else the latest the detail
/// lists.
fn current_run<'a>(feed: &'a Feed, task: &'a daemon::Task) -> Option<&'a Run> {
    task.row.run.as_ref().or_else(|| {
        loaded_task(feed, &task.id)?
            .runs
            .iter()
            .max_by_key(|run| run.started)
    })
}

/// `R2 · worker · omp-sol · gpt-5 high · started 18:40 · ended 19:02`.
fn run_text(run: Option<&Run>, feed: &Feed) -> Rich {
    let Some(run) = run else {
        return Vec::new();
    };
    let mut text = vec![span(run.id.clone()).mono()];
    let model = [run.model.as_deref(), run.effort.as_deref()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    for part in [
        Some(run.role.as_str().to_owned()),
        Some(run.profile.clone()),
        (!model.is_empty()).then_some(model),
    ]
    .into_iter()
    .flatten()
    {
        text.push(span(format!(" · {part}")));
    }
    text.push(span(format!(" · started {}", when_at(run.started, feed))).dim());
    if let Some(ended) = run.ended {
        text.push(span(format!(" · ended {}", when_at(ended, feed))).dim());
    }
    text
}

/// The status of this device's last operation on the task.
fn sent_line(state: &State, feed: &Feed, task: &str) -> Option<SentLine> {
    let sent = state.sent_for(task)?;
    let (words, hue) = match act::status(feed, &sent.key) {
        None => ("Waiting to send".to_owned(), Hue::Neutral),
        Some(status) => {
            let hue = match &status {
                ActStatus::Done => Hue::Ok,
                ActStatus::Failed(_) | ActStatus::Refused(_) => Hue::Fail,
                ActStatus::Pending | ActStatus::Running => Hue::Neutral,
            };
            (status.line(), hue)
        }
    };
    Some(SentLine {
        text: format!("{} · {words}", sent.title),
        hue,
    })
}

/// Every engine operation on the task, all enabled: the engine decides.
fn verbs(id: &Id, feed: &Feed) -> Vec<Button> {
    super::record_verbs(&Record::Task(id.clone()), feed)
        .into_iter()
        .map(|verb| Button::new(verb.label, verb.press, ButtonKind::Ghost))
        .collect()
}

/// Whether the task has a candidate to review: it is in Review, a gate ran on its head, or a
/// worker run finished.
fn has_candidate(feed: &Feed, task: &daemon::Task) -> bool {
    task.status() == Status::Review
        || task.row.gate.is_some()
        || loaded_task(feed, &task.id).is_some_and(|detail| {
            detail
                .runs
                .iter()
                .any(|run| run.role == Role::Worker && run.outcome == Some(Outcome::Done))
        })
}

/// Open session (when the task has one), Ask about this and Review candidate (when there is a
/// candidate), in the mockup's order and wording; the phone's shorter labels.
pub fn task_buttons(feed: &Feed, task: &daemon::Task, phone: bool) -> Vec<Button> {
    let mut buttons = Vec::new();
    if let Some(session) = session_for(feed, &task.id) {
        buttons.push(Button::new(
            if phone { "Session" } else { "Open session" },
            Press::Open(Live::Session(session)),
            if phone {
                ButtonKind::Primary
            } else {
                ButtonKind::Ghost
            },
        ));
    }
    buttons.push(Button::new(
        "Ask about this",
        Press::Ask {
            about: Live::Task(task.id.clone()),
            rev: None,
        },
        ButtonKind::Ghost,
    ));
    if has_candidate(feed, task) {
        buttons.push(Button::new(
            if phone { "Review" } else { "Review candidate" },
            Press::Open(Live::Review(task.id.clone())),
            if phone {
                ButtonKind::Ghost
            } else {
                ButtonKind::Quiet
            },
        ));
    }
    buttons
}

/// The reply box's field (`field-reply`), which Now's `R` puts the keyboard focus in.
pub const REPLY_FIELD: &str = "field-reply";

/// The selected task's reply box, when its Blocked card recommends a reply.
pub fn reply_box(task: &daemon::Task) -> Option<ReplyBox> {
    blocked_card(task)?.reply
}

/// The Blocked card of a task: its reason, detail and one button per next step, labelled as
/// the operation it runs ("Dispatch anyway" for a forced one). A recommended reply is the reply
/// box instead of a button.
fn blocked_card(task: &daemon::Task) -> Option<BlockedCard> {
    let blocked = task.blocked()?;
    let reply = blocked
        .next
        .first()
        .filter(|next| matches!(next.op, corral_types::Op::Reply { .. }))
        .map(|next| ReplyBox {
            task: task.id.clone(),
            index: 0,
            label: format!("Reply to {}", task.id),
            placeholder: "What the worker should hear",
            send: act::label(&next.op, next.force),
        });
    Some(BlockedCard {
        reason: blocked.reason.clone(),
        detail: blocked.detail.clone(),
        next: blocked
            .next
            .iter()
            .enumerate()
            .skip(usize::from(reply.is_some()))
            .map(|(index, next)| {
                Button::new(
                    act::label(&next.op, next.force),
                    Press::Next {
                        task: task.id.clone(),
                        index,
                    },
                    if index == 0 {
                        ButtonKind::Primary
                    } else {
                        ButtonKind::Ghost
                    },
                )
            })
            .collect(),
        reply,
    })
}

/// Who moves next, for the story's heading; a closed task is named by its status.
fn standing(task: &daemon::Task) -> (&'static str, Hue) {
    match task.status() {
        Status::Done => ("Done", Hue::Ok),
        Status::Dropped => ("Dropped", Hue::Neutral),
        _ => {
            let key = group_key(task.group);
            (group_label(key), key.hue())
        }
    }
}

/// A dependency's status in a word and a mark: done, dropped, still to do, or moving.
fn dependency_state(status: Status) -> (Mark, &'static str) {
    let mark = match status {
        Status::Done => Mark::Done,
        Status::Dropped => Mark::Na,
        Status::Running | Status::Blocked | Status::Review => Mark::Active,
        Status::Idea | Status::Backlog | Status::Ready => Mark::Next,
    };
    (mark, status.as_str())
}

/// What the task waits on, each with its status as corrald serves it (Done and Dropped ones
/// included); an id corrald does not know is said plainly and opens nothing.
fn needs(feed: &Feed, task: &daemon::Task) -> Fact {
    let ids = &task.task().depends_on;
    if ids.is_empty() {
        return Fact::Text(vec![span("nothing")]);
    }
    let served = task.row.dependencies.as_ref();
    Fact::Tasks(
        ids.iter()
            .map(|id| {
                let status = match served {
                    Some(served) => served
                        .iter()
                        .find(|dependency| dependency.id == *id)
                        .and_then(|dependency| dependency.status),
                    None => feed.board.task(id).map(daemon::Task::status),
                };
                let known = status.is_some() || served.is_none();
                let (mark, word) = match status {
                    Some(status) => {
                        let (mark, word) = dependency_state(status);
                        (Some(mark), word)
                    }
                    None if known => (None, "not open"),
                    None => (None, "unknown task"),
                };
                Dependency {
                    id: id.clone(),
                    mark,
                    status: word,
                    action: known.then(|| Press::Open(Live::Task(Id::new(id)))),
                }
            })
            .collect(),
    )
}

/// `P1 · bug · S · omp-sol`, leaving out what the task does not set.
fn shape(task: &corral_types::Task) -> String {
    [
        Some(task.priority.as_str()),
        task.kind.as_deref(),
        task.size.as_deref(),
        task.profile.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ")
}

/// `· plan T-885`, opening the plan.
fn plan_link(plan: &str) -> (String, Press) {
    (
        format!("· plan {plan}"),
        Press::Open(Live::Plan(plan.into())),
    )
}

/// The changed-since-visit chip, when the changes feed holds an unseen event for the task.
fn changed_chip(feed: &Feed, id: &str) -> Option<String> {
    let since = feed
        .seen
        .as_ref()
        .and_then(|seen| seen.at.as_deref())
        .map(|at| stamp(at, feed));
    super::unseen_tasks(feed).contains(id).then(|| match since {
        Some(since) if !since.is_empty() => format!("▲ changed since {since}"),
        _ => "▲ changed since your last visit".to_owned(),
    })
}

/// Build a task's story, from its served row and, once it arrives, its detail.
pub fn live_story(state: &State, feed: &Feed, task: &daemon::Task, form: Form) -> LiveStory {
    let (who, who_hue) = standing(task);
    let blocked = blocked_card(task);
    let mut facts: Vec<(&'static str, Fact)> = Vec::new();

    let mut doing = vec![span(task.now())];
    let age = super::when::age(task.since(), feed.now);
    if !age.is_empty() {
        doing.push(span(format!(" · for {age}")).dim());
    }
    facts.push(("Now", Fact::Text(doing)));

    if task.group == daemon::Group::NeedsYou && task.status() == Status::Review {
        facts.push((
            "You",
            Fact::Answer(Button::new(
                "Review and land",
                Press::Open(Live::Review(task.id.clone())),
                ButtonKind::Primary,
            )),
        ));
    } else if task.group != daemon::Group::NeedsYou {
        facts.push(("You", Fact::Text(vec![span("nothing needed").hue(Hue::Ok)])));
    }
    facts.push(("Needs", needs(feed, task)));
    let run = run_text(current_run(feed, task), feed);
    if !run.is_empty() {
        facts.push(("Run", Fact::Text(run)));
    }
    if let Some(project) = task.project() {
        facts.push(("Project", Fact::Text(vec![span(project.to_owned())])));
    }
    facts.push(("Shape", Fact::Text(vec![span(shape(task.task()))])));
    if task.task().round > 0 {
        facts.push((
            "Round",
            Fact::Text(vec![span(task.task().round.to_string())]),
        ));
    }
    facts.push((
        "Status",
        Fact::Text(vec![span(task.status().as_str().to_owned())]),
    ));

    let mut actions = if form == Form::Phone {
        Vec::new()
    } else {
        task_buttons(feed, task, false)
    };
    actions.extend(verbs(&task.id, feed));
    let actor = daemon_actor(task.group);
    LiveStory {
        id: task.id.to_string(),
        title: task.title().to_owned(),
        who,
        who_hue,
        plan: task.parent().map(plan_link),
        changed: changed_chip(feed, &task.id),
        summary: Vec::new(),
        body: Some(task.task().body.clone()).filter(|body| !body.trim().is_empty()),
        stages: std::array::from_fn(|index| stage(&task.rail[index], index, feed.now, actor)),
        blocked,
        facts,
        actions,
        note: detail_note(feed, &task.id),
        sent: sent_line(state, feed, &task.id),
    }
}

/// Build a task's artifact list from its detail.
pub fn live_evidence(feed: &Feed, id: &str) -> LiveEvidence {
    let detail = loaded_task(feed, id);
    let rows: Vec<(String, String)> = detail
        .map(|detail| {
            detail
                .artifacts
                .iter()
                .map(|artifact| {
                    let source = match &artifact.kind {
                        Some(kind) => format!("{} · {kind}", artifact.reference()),
                        None => artifact.reference(),
                    };
                    (artifact.title.clone(), source)
                })
                .collect()
        })
        .unwrap_or_default();
    LiveEvidence {
        title: format!("Evidence for {id}"),
        empty: (detail.is_some() && rows.is_empty()).then_some(NO_ARTIFACTS),
        rows,
        note: detail_note(feed, id),
        find: Button::new(
            crate::model::artifacts::FIND_IN_ALL,
            Press::FindArtifacts(Id::from(id)),
            ButtonKind::Ghost,
        ),
    }
}

/// The task's terminal as last captured, when a capture of its run's session is loaded: the
/// Agents terminal reads the same capture.
pub fn task_tail(feed: &Feed, task: &daemon::Task) -> Option<Tail> {
    let session = session_for(feed, &task.id)?;
    let cache = feed.detail(&DetailRef::Terminal {
        id: session.to_string(),
    })?;
    let Some(DetailBody::Terminal(capture)) = cache.body() else {
        return None;
    };
    let run = feed.agents.get(&session).map(|row| &row.run);
    let pane = match run.and_then(|run| run.session.as_ref()) {
        Some(SessionRef::Herdr { pane, .. }) => pane.clone(),
        _ => "−".into(),
    };
    let at = capture
        .captured_at
        .parse::<jiff::Timestamp>()
        .ok()
        .map(|at| stamp_at(at, feed))
        .filter(|at| !at.is_empty());
    Some(Tail {
        id: task.id.to_string(),
        pane,
        runtime: run.map(|run| run.harness.clone()).unwrap_or_default(),
        label: match (capture.live, at) {
            (true, _) => "live".into(),
            (false, Some(at)) => format!("captured {at}"),
            (false, None) => "captured".into(),
        },
        snapshot: !capture.live,
        illustrative: None,
        lines: capture.lines.iter().map(TailLine::new).collect(),
        cursor: false,
        action: Some(Press::Open(Live::Session(session))),
    })
}

/// Build the selected daemon task's Session tab.
pub fn live_session(feed: &Feed, task: &daemon::Task) -> LiveSession {
    let detail = loaded_task(feed, &task.id);
    let tail = task_tail(feed, task).filter(|tail| !tail.lines.is_empty());
    let run = run_text(current_run(feed, task), feed);
    let id = &task.id;
    LiveSession {
        description: "Structured transcript in its session; below, its terminal as last captured.",
        open: session_for(feed, id).map(|session| {
            Button::new(
                format!("Open {id} session"),
                Press::Open(Live::Session(session)),
                ButtonKind::Primary,
            )
        }),
        empty: if detail.is_some() && run.is_empty() {
            Some(NO_RUN)
        } else if !run.is_empty() && tail.is_none() {
            Some(NO_TAIL)
        } else {
            None
        },
        run,
        tail,
        note: detail_note(feed, id),
    }
}
