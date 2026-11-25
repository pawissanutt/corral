//! What waits on the owner as the dashboard's cards. A closed card is two lines and its one
//! next step; the open one holds the decision. For a task that is its Blocked reason, the log
//! excerpt the engine served, the reply box when the recommended step is a reply, and one
//! button per other next step; for a review ready to land, the pipeline's phrase and Approve.
//! A Blocked task whose Pi run asks an owner question, as the run host's state briefs it,
//! offers Answer, which opens the run's page with that card open, in place of the reply box;
//! a run of another harness, or one whose state is not read, keeps the engine's steps.
//! A session's open card is its own dock and composer (the screen mounts them).

use corral_types::{SessionRef, SessionStatus, Status};

use crate::model::agents::{briefed_question, session_line};
use crate::model::daemon::{Feed, Task};
use crate::model::next::{NextStep, next_step, session_next_step};
use crate::model::pipeline::phrase;
use crate::model::session::Asked;
use crate::model::status::{Look, session_look, status_look, status_word};
use crate::model::work::{self, Button, ButtonKind, Press, ReplyBox, age};
use crate::model::{Id, Live};

/// The run session Home watches while it shows task `id`: the task's current run, when the task
/// is Blocked and a Pi host runs that run and waits, so the host's state says what it asks.
pub fn watched_run(feed: &Feed, id: &Id) -> Option<Id> {
    let task = feed
        .board
        .task(id)
        .filter(|task| task.status() == Status::Blocked)?;
    let session = Id::from(format!("{}_{}", task.id, task.row.run.as_ref()?.id));
    let row = feed.agents.get(&session)?;
    (matches!(row.run.session, Some(SessionRef::Pi { .. }))
        && matches!(row.status, SessionStatus::Blocked { .. }))
    .then_some(session)
}

/// Answer on a Blocked task whose Pi run asks an owner question: the run's page with that card
/// open, the card named by its id as the host's state briefs it, never by the excerpt's words.
/// None until that state is read and for a run of another harness.
pub fn question_step(feed: &Feed, task: &Task) -> Option<NextStep> {
    let session = watched_run(feed, &task.id)?;
    let card = briefed_question(feed, session.as_str())??.to_owned();
    Some(NextStep {
        label: "Answer".into(),
        press: Press::Question {
            session,
            asked: Asked::Card(card),
        },
        verb: None,
        yours: true,
    })
}

/// One card's head and its closed form.
#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub id: Id,
    pub live: Live,
    pub look: Look,
    pub word: &'static str,
    pub title: String,
    /// A priority other than P2, or a session's role.
    pub tag: Option<String>,
    /// How long it has waited: `2h05m`.
    pub age: String,
    /// Why it waits, as one phrase: the pipeline's, or what the session asks.
    pub why: String,
    /// `site · omp-sol`: the project and the profile, where known.
    pub facts: String,
    pub next: Option<NextStep>,
    pub session: bool,
}

/// The card of a task or a session that waits on the owner; none once the daemon lists
/// neither.
pub fn card(feed: &Feed, id: &Id) -> Option<Card> {
    if let Some(task) = feed.board.task(id) {
        let profile = task
            .row
            .run
            .as_ref()
            .map(|run| run.profile.clone())
            .or_else(|| task.task().profile.clone());
        let priority = task.task().priority.as_str();
        return Some(Card {
            id: id.clone(),
            live: Live::Task(id.clone()),
            look: status_look(task.status()),
            word: status_word(task.status()),
            title: task.title().to_owned(),
            tag: (priority != "P2").then(|| priority.to_owned()),
            age: age(task.since(), feed.now),
            why: phrase(task, feed),
            facts: join([task.project().map(str::to_owned), profile]),
            next: question_step(feed, task).or_else(|| next_step(task)),
            session: false,
        });
    }
    let session = session_line(feed, id)?;
    Some(Card {
        id: id.clone(),
        live: Live::Session(id.clone()),
        look: session_look(session.state),
        word: session.state.word(),
        title: if session.name.is_empty() {
            session.role.clone()
        } else {
            session.name.clone()
        },
        tag: Some(session.role.clone()),
        age: String::new(),
        why: session.doing.clone().unwrap_or_default(),
        facts: join([session.project.clone(), session.agent.clone()]),
        next: session_next_step(&session),
        session: true,
    })
}

fn join<const N: usize>(parts: [Option<String>; N]) -> String {
    parts
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The decision an open task card holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    /// `Blocked: the gate failed`, `Ready to land onto main`.
    pub what: String,
    /// The engine's detail (a log excerpt), or the pipeline's phrase.
    pub detail: Option<String>,
    /// The reply box, when the engine's first next step is a reply.
    pub reply: Option<ReplyBox>,
    /// One button per other served next step, the first one primary (`btn-next-<n>`).
    pub steps: Vec<Button>,
}

/// The decision of the task `id` when it waits on the owner; none for a session, whose card
/// is its own dock and composer, and for a task that waits on nobody. A Pi run's question
/// replaces the reply box: a reply would not answer its card.
pub fn decision(feed: &Feed, id: &Id) -> Option<Decision> {
    let task = feed.board.task(id)?;
    match task.status() {
        Status::Blocked => {
            let blocked = task.blocked()?;
            let reply = work::reply_box(task);
            let question = question_step(feed, task);
            let engine = blocked
                .next
                .iter()
                .enumerate()
                .skip(usize::from(reply.is_some()))
                .map(|(index, next)| {
                    let (label, press) = work::blocked_step(task, index, next);
                    Button::new(
                        label,
                        press,
                        if index == 0 && question.is_none() {
                            ButtonKind::Primary
                        } else {
                            ButtonKind::Ghost
                        },
                    )
                });
            Some(Decision {
                what: format!("Blocked: {}", blocked.reason),
                detail: blocked.detail.clone(),
                steps: question
                    .as_ref()
                    .map(|step| {
                        Button::new(step.label.clone(), step.press.clone(), ButtonKind::Primary)
                    })
                    .into_iter()
                    .chain(engine)
                    .collect(),
                reply: reply.filter(|_| question.is_none()),
            })
        }
        Status::Review if task.row.ready => {
            let onto = task
                .row
                .run
                .as_ref()
                .map(|run| run.base.clone())
                .or_else(|| task.task().base.clone())
                .filter(|base| !base.is_empty());
            Some(Decision {
                what: onto.map_or_else(
                    || "Ready to land".to_owned(),
                    |onto| format!("Ready to land onto {onto}"),
                ),
                detail: Some(phrase(task, feed)).filter(|phrase| !phrase.is_empty()),
                reply: None,
                // The one next step is Approve; the card draws it as every next step is drawn.
                steps: Vec::new(),
            })
        }
        _ => None,
    }
}
