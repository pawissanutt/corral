//! Home: what waits on the owner, then what is working, tasks and sessions mixed. Needs you
//! is exactly the Blocked tasks, the Review tasks the engine marked `ready`, and the Blocked
//! sessions that are not the run of a task already listed. A task and a session share one row
//! anatomy; the id's prefix and the role word tell them apart.

use corral_types::Status;

use crate::model::agents::{SessionLine, session_line, session_lines};
use crate::model::daemon::{Feed, Task};
use crate::model::list::Section;
use crate::model::next::{NextStep, next_step, session_next_step};
use crate::model::pipeline::phrase;
use crate::model::projects::Scope;
use crate::model::status::{Look, SessionState, session_look, status_look, status_word};
use crate::model::{Id, Live};

pub const TITLE: &str = "Home";
pub const NEEDS_YOU: &str = "Needs you";
pub const WORKING: &str = "Working";
/// What Home says when nothing waits and nothing works.
pub const EMPTY: &str = "Nothing waits on you and nothing is working.";

/// Home's own state: the row or card the keys stand on, a task's id or a session's.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct State {
    pub selected: Option<Id>,
}

/// The page a Home row opens: the task's when the open-task list holds the id, else the
/// session's.
pub fn live_of(feed: &Feed, id: &Id) -> Live {
    if feed.board.task(id).is_some() {
        Live::Task(id.clone())
    } else {
        Live::Session(id.clone())
    }
}

/// The record whose More menu a Home row carries.
pub fn record_of(feed: &Feed, id: &Id) -> crate::model::work::Record {
    match live_of(feed, id) {
        Live::Session(id) => crate::model::work::Record::Session(id),
        Live::Task(id) | Live::Plan(id) | Live::Review(id) => crate::model::work::Record::Task(id),
    }
}

/// Whether the task waits on the owner: Blocked, or in Review and ready to land.
pub fn task_needs_you(task: &Task) -> bool {
    match task.status() {
        Status::Blocked => true,
        Status::Review => task.row.ready,
        _ => false,
    }
}

/// Whether Corral or a worker is at the task: Running, or in Review while the gate or the
/// reviewer runs.
fn task_working(task: &Task) -> bool {
    match task.status() {
        Status::Running => true,
        Status::Review => !task.row.ready,
        _ => false,
    }
}

fn tasks<'a>(feed: &'a Feed, scope: &'a Scope) -> impl Iterator<Item = &'a Task> {
    feed.board
        .tasks()
        .iter()
        .filter(move |task| scope.matches(task.project()))
}

/// The sessions Home lists beside the tasks: a run is listed by its task, so only chats and
/// builds stand for themselves.
fn sessions(feed: &Feed, scope: &Scope, state: SessionState) -> Vec<SessionLine> {
    session_lines(feed, &scope.filter())
        .into_iter()
        .filter(|session| session.task.is_none() && session.state == state)
        .collect()
}

/// What waits on the owner in scope, tasks first: the rows of Needs you.
pub fn needs_you(feed: &Feed, scope: &Scope) -> Vec<Id> {
    tasks(feed, scope)
        .filter(|task| task_needs_you(task))
        .map(|task| task.id.clone())
        .chain(
            sessions(feed, scope, SessionState::Blocked)
                .into_iter()
                .map(|session| session.id),
        )
        .collect()
}

/// How many things wait on the owner in the whole workspace: the count on Home's place.
pub fn needs_you_count(feed: &Feed) -> usize {
    needs_you(feed, &Scope::All).len()
}

/// Home's sections: Needs you, then Working. A section with nothing in it is left out.
pub fn sections(feed: &Feed, scope: &Scope) -> Vec<Section> {
    let working: Vec<Id> = tasks(feed, scope)
        .filter(|task| task_working(task))
        .map(|task| task.id.clone())
        .chain(
            sessions(feed, scope, SessionState::Working)
                .into_iter()
                .map(|session| session.id),
        )
        .collect();
    [
        ("needs-you", NEEDS_YOU, needs_you(feed, scope)),
        ("working", WORKING, working),
    ]
    .into_iter()
    .filter(|(_, _, rows)| !rows.is_empty())
    .map(|(key, label, rows)| Section {
        key: key.to_owned(),
        label: label.to_owned(),
        look: None,
        count: rows.len(),
        note: None,
        fold: None,
        rows,
    })
    .collect()
}

/// One row of Home: a task or a session in the same anatomy.
#[derive(Debug, Clone, PartialEq)]
pub struct HomeRow {
    pub id: Id,
    /// The page the row opens.
    pub live: Live,
    pub look: Look,
    /// The status or state word: `Blocked`, `Working`.
    pub word: &'static str,
    /// The task's title, or a session's role and name.
    pub title: String,
    /// What is happening or asked, as one phrase.
    pub now: String,
    pub next: Option<NextStep>,
}

impl HomeRow {
    /// What a screen reader hears for the row.
    pub fn spoken(&self) -> String {
        let mut spoken = format!("{} {}, {}", self.id, self.title, self.word);
        if !self.now.is_empty() {
            spoken.push_str(", ");
            spoken.push_str(&self.now);
        }
        spoken
    }
}

/// The row of a task or a session by its id; none once the daemon lists neither.
pub fn row(feed: &Feed, id: &Id) -> Option<HomeRow> {
    if let Some(task) = feed.board.task(id) {
        return Some(HomeRow {
            id: id.clone(),
            live: Live::Task(id.clone()),
            look: status_look(task.status()),
            word: status_word(task.status()),
            title: task.title().to_owned(),
            now: phrase(task, feed),
            next: next_step(task),
        });
    }
    let session = session_line(feed, id)?;
    let title = if session.name.is_empty() {
        session.role.clone()
    } else {
        format!("{} · {}", session.role, session.name)
    };
    Some(HomeRow {
        id: id.clone(),
        live: Live::Session(id.clone()),
        look: session_look(session.state),
        word: session.state.word(),
        title,
        now: session.doing.clone().unwrap_or_default(),
        next: session_next_step(&session),
    })
}

#[cfg(test)]
mod tests {
    use corral_protocol::rows::AssistantKind;
    use corral_protocol::rows::AssistantState;

    use super::*;
    use crate::model::agents::roster_tests::assistant;
    use crate::model::daemon::{bare_task, task_row};

    fn task(id: &str, status: Status, ready: bool) -> Task {
        let mut row = task_row(bare_task(id, id, status, jiff::Timestamp::UNIX_EPOCH));
        row.ready = ready;
        Task::new(row)
    }

    /// A build waiting on an answer sat in the Agents roster while the home screen showed an
    /// idle loop: what waits on the owner is tasks and sessions alike, and a review still
    /// being verified is not the owner's move.
    #[test]
    fn needs_you_is_blocked_tasks_ready_reviews_and_blocked_sessions() {
        let mut feed = Feed::default();
        feed.board.apply_batch(
            1,
            vec![
                task("T-1", Status::Blocked, false),
                task("T-2", Status::Review, true),
                task("T-3", Status::Review, false),
                task("T-4", Status::Running, false),
                task("T-5", Status::Ready, false),
            ],
            &[],
        );
        let mut waiting = assistant("B-k3x9", AssistantKind::Build);
        waiting.state = AssistantState::Waiting;
        feed.assistants.insert("B-k3x9".into(), waiting);
        let mut working = assistant("B-p8d1", AssistantKind::Build);
        working.state = AssistantState::Working;
        feed.assistants.insert("B-p8d1".into(), working);
        let ids = |rows: &[Id]| rows.iter().map(ToString::to_string).collect::<Vec<_>>();
        assert_eq!(
            ids(&needs_you(&feed, &Scope::All)),
            ["T-1", "T-2", "B-k3x9"]
        );
        assert_eq!(needs_you_count(&feed), 3);
        let sections = sections(&feed, &Scope::All);
        assert_eq!(sections.len(), 2);
        assert_eq!(ids(&sections[1].rows), ["T-3", "T-4", "B-p8d1"]);
        let build = row(&feed, &Id::from("B-k3x9")).expect("the build's row");
        assert_eq!(build.word, "Blocked");
        assert_eq!(build.live, Live::Session(Id::from("B-k3x9")));
        assert_eq!(build.next.map(|step| step.label).as_deref(), Some("Answer"));
    }
}
