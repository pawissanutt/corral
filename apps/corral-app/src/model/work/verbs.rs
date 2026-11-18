//! One Actions menu for every task, shared by row menus, page heads, the keyboard menu and the
//! palette. It lists every engine operation in a fixed order and never predicts a refusal:
//! every item is enabled and the engine decides. A run's Session menu leads with the operations
//! that act on its session, then opens its task's Actions menu.
use super::Press;
use crate::model::daemon::Feed;
use crate::model::{Destination, Id, Live, Selection};

/// What an action list is about: a task (a plan and a review candidate are tasks), a run's
/// session (`T-42_R1`), or the workspace, whose only action creates a task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Record {
    Task(Id),
    Session(Id),
    Workspace,
}

impl Record {
    pub fn from_live(live: &Live) -> Self {
        match live {
            Live::Task(id) | Live::Plan(id) | Live::Review(id) => Self::Task(id.clone()),
            Live::Session(id) if session_task(id).is_some() => Self::Session(id.clone()),
            Live::Session(_) | Live::Thread(_) => Self::Workspace,
        }
    }
    pub fn live(&self) -> Option<Live> {
        match self {
            Self::Task(id) => Some(Live::Task(id.clone())),
            Self::Session(id) => Some(Live::Session(id.clone())),
            Self::Workspace => None,
        }
    }
    pub fn id(&self) -> &str {
        match self {
            Self::Task(id) | Self::Session(id) => id.as_str(),
            Self::Workspace => "workspace",
        }
    }
}

/// The task of a run's session id, `T-42_R1`.
pub fn session_task(session: &str) -> Option<&str> {
    session
        .split_once('_')
        .map(|(task, _)| task)
        .filter(|task| task.starts_with("T-"))
}

pub fn palette_record(
    destination: Destination,
    selected: &Selection,
    top: Option<&Live>,
) -> Option<Record> {
    if let Some(top) = top {
        return Some(Record::from_live(top));
    }
    Some(
        match destination {
            Destination::Now => selected.task.clone().map(Record::Task),
            Destination::Plans => selected.plan.clone().map(Record::Task),
            Destination::Review => selected.review.clone().map(Record::Task),
            Destination::Agents => selected
                .session
                .clone()
                .filter(|session| session_task(session).is_some())
                .map(Record::Session),
            _ => None,
        }
        .unwrap_or(Record::Workspace),
    )
}

/// Every engine operation the owner starts on a task, plus the workspace's New task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Edit,
    Comment,
    Move,
    Dispatch,
    Reply,
    Done,
    Gate,
    Review,
    Verdict,
    Changes,
    Approve,
    Retry,
    Stop,
    Drop,
    Reopen,
    NewTask,
}

impl Verb {
    /// A task's Actions menu, in the plan's fixed order.
    pub const TASK: [Verb; 15] = [
        Verb::Edit,
        Verb::Comment,
        Verb::Move,
        Verb::Dispatch,
        Verb::Reply,
        Verb::Done,
        Verb::Gate,
        Verb::Review,
        Verb::Verdict,
        Verb::Changes,
        Verb::Approve,
        Verb::Retry,
        Verb::Stop,
        Verb::Drop,
        Verb::Reopen,
    ];

    pub const fn slug(self) -> &'static str {
        match self {
            Self::Edit => "edit",
            Self::Comment => "comment",
            Self::Move => "move",
            Self::Dispatch => "dispatch",
            Self::Reply => "reply",
            Self::Done => "done",
            Self::Gate => "gate",
            Self::Review => "review",
            Self::Verdict => "verdict",
            Self::Changes => "changes",
            Self::Approve => "approve",
            Self::Retry => "retry",
            Self::Stop => "stop",
            Self::Drop => "drop",
            Self::Reopen => "reopen",
            Self::NewTask => "new-task",
        }
    }

    /// The menu's words; an ellipsis marks one that asks for input first.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Edit => "Edit…",
            Self::Comment => "Comment…",
            Self::Move => "Move to…",
            Self::Dispatch => "Dispatch…",
            Self::Reply => "Reply…",
            Self::Done => "Mark done…",
            Self::Gate => "Run gate",
            Self::Review => "Start review…",
            Self::Verdict => "Record verdict…",
            Self::Changes => "Request changes…",
            Self::Approve => "Approve and land…",
            Self::Retry => "Retry…",
            Self::Stop => "Stop",
            Self::Drop => "Drop",
            Self::Reopen => "Reopen",
            Self::NewTask => "New task…",
        }
    }

    /// The composer's send button: the label without its ellipsis.
    pub fn submit_label(self) -> &'static str {
        self.label().trim_end_matches('…')
    }

    pub fn title(self, record: &Record) -> String {
        match record {
            Record::Task(id) | Record::Session(id) => format!("{} {id}", self.submit_label()),
            Record::Workspace => self.submit_label().into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerbField {
    pub label: &'static str,
    pub placeholder: &'static str,
    pub required: bool,
    pub value: String,
    /// A closed set of values drawn as one button each; empty for free text.
    pub choices: Vec<Choice>,
}

/// One value a choice field offers: what is sent, and what its button says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub value: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerbAction {
    pub slug: &'static str,
    pub label: String,
    /// Always true for engine operations: the engine decides, the app never predicts.
    pub enabled: bool,
    pub reason: Option<String>,
    pub press: Press,
}

/// No surface owns a second list of operations or its own enabled conditions. A run's Session
/// menu reads the feed for where its pane can be focused.
pub fn record_verbs(record: &Record, feed: &Feed) -> Vec<VerbAction> {
    let table: &[Verb] = match record {
        Record::Task(_) => &Verb::TASK,
        Record::Workspace => &[Verb::NewTask],
        Record::Session(session) => return crate::model::agents::session_verbs(feed, session),
    };
    table
        .iter()
        .map(|&verb| VerbAction {
            slug: verb.slug(),
            label: verb.label().into(),
            enabled: true,
            reason: None,
            press: Press::Verb {
                record: record.clone(),
                verb,
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_task_lists_all_fifteen_operations_in_order_and_all_enabled() {
        let verbs = record_verbs(&Record::Task(Id::new("T-1")), &Feed::default());
        assert_eq!(
            verbs.iter().map(|verb| verb.slug).collect::<Vec<_>>(),
            [
                "edit", "comment", "move", "dispatch", "reply", "done", "gate", "review",
                "verdict", "changes", "approve", "retry", "stop", "drop", "reopen"
            ]
        );
        assert!(
            verbs
                .iter()
                .all(|verb| verb.enabled && verb.reason.is_none())
        );
    }

    #[test]
    fn a_run_session_has_its_session_menu_and_an_assistant_none() {
        assert_eq!(
            Record::from_live(&Live::Session(Id::new("T-42_R2"))),
            Record::Session(Id::new("T-42_R2"))
        );
        assert_eq!(
            Record::from_live(&Live::Session(Id::new("A-1"))),
            Record::Workspace
        );
    }
}
