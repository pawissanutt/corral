//! One canonical action vocabulary shared by inspector, context menus and palette.
use super::{Press, loaded_task};
use crate::model::daemon::{Feed, Link};
use crate::model::{Destination, Id, Live, Selection};
use corral_protocol::{
    app::ActClass,
    cmd::Expect,
    detail::{DetailBody, DetailRef},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Record {
    Task(Id),
    Plan(Id),
    Review(Id),
    Question(Id),
    Workspace,
    Assistant(Id),
}

impl Record {
    pub fn from_live(live: &Live) -> Self {
        match live {
            Live::Task(id) => Self::Task(id.clone()),
            Live::Plan(id) => Self::Plan(id.clone()),
            Live::Review(id) => Self::Review(id.clone()),
            Live::Question(id) => Self::Question(id.clone()),
            Live::Thread(id) => Self::Assistant(id.clone()),
            // A build record carries no record actions; the workspace's apply beside it.
            Live::Session(_) | Live::Build(_) => Self::Workspace,
        }
    }
    pub fn live(&self) -> Option<Live> {
        Some(match self {
            Self::Task(id) => Live::Task(id.clone()),
            Self::Plan(id) => Live::Plan(id.clone()),
            Self::Review(id) => Live::Review(id.clone()),
            Self::Question(id) => Live::Question(id.clone()),
            Self::Assistant(id) => Live::Thread(id.clone()),
            Self::Workspace => return None,
        })
    }
    pub fn id(&self) -> &str {
        self.live_id().map_or("workspace", Id::as_str)
    }
    fn live_id(&self) -> Option<&Id> {
        match self {
            Self::Task(id)
            | Self::Plan(id)
            | Self::Review(id)
            | Self::Question(id)
            | Self::Assistant(id) => Some(id),
            Self::Workspace => None,
        }
    }
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
            Destination::Inbox => selected.question.clone().map(Record::Question),
            Destination::Plans => selected.plan.clone().map(Record::Plan),
            Destination::Review => selected.review.clone().map(Record::Review),
            Destination::Agents => selected.thread.clone().map(Record::Assistant),
            _ => None,
        }
        .unwrap_or(Record::Workspace),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Hold,
    Release,
    Retry,
    Dispatch,
    QueueAdd,
    QueueRemove,
    Park,
    Unpark,
    Comment,
    NewTask,
    Unstick,
    Revert,
    Discard,
    PlanCancel,
    Verdict,
    AddMember,
    MoveMember,
    Escalate,
    CancelQuestion,
    Deadline,
    Route,
    Focus,
    Move,
    ModeOff,
    PermissionsRevoke,
}
impl Verb {
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Hold => "hold",
            Self::Release => "release",
            Self::Retry => "retry",
            Self::Dispatch => "dispatch",
            Self::QueueAdd => "queue-add",
            Self::QueueRemove => "queue-remove",
            Self::Park => "park",
            Self::Unpark => "unpark",
            Self::Comment => "comment",
            Self::NewTask => "new-task",
            Self::Unstick => "unstick",
            Self::Revert => "revert",
            Self::Discard => "discard",
            Self::PlanCancel => "plan-cancel",
            Self::Verdict => "verdict",
            Self::AddMember => "add-member",
            Self::MoveMember => "move-member",
            Self::Escalate => "escalate",
            Self::CancelQuestion => "cancel-question",
            Self::Deadline => "deadline",
            Self::Route => "route",
            Self::Focus => "focus",
            Self::Move => "move",
            Self::ModeOff => "mode-off",
            Self::PermissionsRevoke => "permissions-revoke",
        }
    }
    pub const fn label(self) -> &'static str {
        match self {
            Self::Hold => "Hold",
            Self::Release => "Release",
            Self::Retry => "Retry",
            Self::Dispatch => "Dispatch",
            Self::QueueAdd => "Add to queue",
            Self::QueueRemove => "Remove from queue",
            Self::Park => "Park",
            Self::Unpark => "Unpark",
            Self::Comment => "Comment on",
            Self::NewTask => "New task",
            Self::Unstick => "Unstick",
            Self::Revert => "Revert",
            Self::Discard => "Discard",
            Self::PlanCancel => "Cancel plan",
            Self::Verdict => "Record verdict for",
            Self::AddMember => "Add member to",
            Self::MoveMember => "Move member from",
            Self::Escalate => "Escalate",
            Self::CancelQuestion => "Cancel question",
            Self::Deadline => "Set deadline for",
            Self::Route => "Route",
            Self::Focus => "Focus",
            Self::Move => "Move",
            Self::ModeOff => "Turn automation off",
            Self::PermissionsRevoke => "Revoke permissions for",
        }
    }
    /// Destructive verbs ask for this device's confirmation even where the command's wire class
    /// alone would send straight away.
    pub const fn confirms(self) -> bool {
        matches!(self, Self::Discard)
    }
    pub fn title(self, record: &Record) -> String {
        if matches!(self, Self::NewTask | Self::ModeOff) {
            self.label().into()
        } else {
            format!("{} {}", self.label(), record.id())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerbField {
    pub label: &'static str,
    pub placeholder: &'static str,
    pub required: bool,
    pub value: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerbAction {
    pub slug: &'static str,
    pub label: String,
    pub class: ActClass,
    pub enabled: bool,
    pub reason: Option<String>,
    pub fields: Vec<VerbField>,
    pub press: Press,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Frozen {
    pub record: Record,
    pub revision: Option<String>,
    pub question_rev: u32,
    pub sha: String,
    pub run: Option<String>,
    pub park_revision: Option<u64>,
}
impl Frozen {
    pub fn capture(feed: &Feed, record: &Record) -> Result<Self, String> {
        if feed.link != Link::Up {
            return Err("Needs a live connection. Nothing was sent.".into());
        }
        let mut frozen = Self {
            record: record.clone(),
            revision: None,
            question_rev: 0,
            sha: String::new(),
            run: None,
            park_revision: None,
        };
        if let Some(live) = record.live()
            && let Some(target) = live.detail()
            && feed
                .detail(&target)
                .is_none_or(|cache| cache.stale || cache.error.is_some() || cache.body().is_none())
        {
            return Err("Load the current record detail before acting.".into());
        }
        match record {
            Record::Task(id) | Record::Review(id) | Record::Plan(id) => {
                let task = loaded_task(feed, id)
                    .ok_or("Load the task's canonical owner revision before acting.")?;
                if feed
                    .detail(&super::task_ref(id))
                    .is_none_or(|cache| cache.stale || cache.error.is_some())
                {
                    return Err("Task detail is stale; refresh it before acting.".into());
                }
                frozen.revision = Some(task.owner_revision.clone());
                frozen.run = task.run.as_ref().map(|run| run.id.clone());
                frozen.park_revision = task.park_revision;
                if matches!(record, Record::Review(_)) {
                    let review = feed
                        .detail(&DetailRef::Review {
                            task: id.to_string(),
                        })
                        .and_then(|cache| cache.body());
                    let Some(DetailBody::Review(review)) = review else {
                        return Err("The candidate detail is unavailable.".into());
                    };
                    frozen.sha = review
                        .candidate_sha
                        .clone()
                        .ok_or("No candidate SHA is available.")?;
                }
            }
            Record::Question(id) => {
                let Some(DetailBody::Question(question)) = feed
                    .detail(&DetailRef::Question {
                        key: id.to_string(),
                    })
                    .and_then(|cache| cache.body())
                else {
                    return Err("The question detail is unavailable.".into());
                };
                if question.state != corral_protocol::rows::InboxState::Open {
                    return Err("This question is no longer open.".into());
                }
                if let Some(row) = feed.inbox.get(id)
                    && (row.state != corral_protocol::rows::InboxState::Open
                        || row.question_rev.is_some_and(|rev| rev != question.rev))
                {
                    return Err("The question changed; refresh it before acting.".into());
                }
                frozen.question_rev = question.rev;
            }
            Record::Assistant(_) | Record::Workspace => {}
        }
        Ok(frozen)
    }
    pub fn expect(&self) -> Option<Expect> {
        self.revision.as_ref().map(|revision| Expect::TaskRevision {
            task: self.record.id().into(),
            revision: revision.clone(),
        })
    }
}

fn table(record: &Record) -> &'static [Verb] {
    use Verb::*;
    match record {
        Record::Task(_) => &[
            Dispatch,
            QueueAdd,
            QueueRemove,
            Park,
            Unpark,
            Comment,
            Hold,
            Release,
            Retry,
            Unstick,
            Revert,
            Discard,
            Route,
            Focus,
            Move,
        ],
        Record::Plan(_) => &[PlanCancel, AddMember, MoveMember],
        Record::Review(_) => &[Verdict],
        Record::Question(_) => &[Escalate, CancelQuestion, Deadline],
        Record::Workspace => &[NewTask, ModeOff],
        Record::Assistant(_) => &[PermissionsRevoke],
    }
}

/// No UI surface owns a second list of commands or different enabled conditions.
pub fn record_verbs(feed: &Feed, record: &Record) -> Vec<VerbAction> {
    let captured = Frozen::capture(feed, record);
    let prototype = Frozen {
        record: record.clone(),
        revision: None,
        question_rev: 0,
        sha: String::new(),
        run: None,
        park_revision: Some(0),
    };
    table(record)
        .iter()
        .copied()
        .map(|verb| {
            let fields = super::verb_forms::fields(verb);
            let mut reason = captured
                .as_ref()
                .err()
                .cloned()
                .or_else(|| unavailable(feed, record, verb));
            let values: Vec<String> = fields.iter().map(|field| field.value.clone()).collect();
            let class = super::verb_build::command(&prototype, verb, &values)
                .map_or(ActClass::Refused, |cmd| cmd.app_class());
            if reason.is_none() {
                reason = authority_reason(feed, class);
            }
            VerbAction {
                slug: verb.slug(),
                label: verb.title(record),
                class,
                enabled: reason.is_none(),
                reason,
                fields,
                press: Press::Verb {
                    record: record.clone(),
                    verb,
                },
            }
        })
        .collect()
}

pub(super) fn unavailable(feed: &Feed, record: &Record, verb: Verb) -> Option<String> {
    if !table(record).contains(&verb) {
        return Some("That action does not apply to this record.".into());
    }
    if let Record::Task(id) = record {
        let task = loaded_task(feed, id)?;
        let reason = match verb {
            Verb::Release if task.hold.is_none() => Some("This task is not held."),
            Verb::Hold if task.hold.is_some() => Some("This task is already held."),
            Verb::Unpark if task.park.is_none() => Some("This task is not parked."),
            Verb::Unpark if task.park_revision.is_none() => {
                Some("The daemon did not supply the park revision.")
            }
            Verb::Park if task.park.is_some() => Some("This task is already parked."),
            Verb::Park if !matches!(task.status.as_str(), "ready" | "review" | "needs_me") => {
                Some("Only ready, review or blocked tasks can be parked.")
            }
            Verb::QueueRemove if task.status != "ready" => Some("This task is not queued."),
            Verb::QueueAdd | Verb::Dispatch if task.hold.is_some() || task.park.is_some() => {
                Some("Release the hold or park before dispatching.")
            }
            Verb::QueueAdd | Verb::Dispatch
                if !matches!(task.status.as_str(), "ready" | "backlog" | "idea") =>
            {
                Some("This task is not ready for dispatch.")
            }
            Verb::Retry
                if feed
                    .board
                    .task(id)
                    .is_none_or(|row| row.rail[0].mark != crate::model::board::Mark::Fail) =>
            {
                Some("This task has no failed worker run.")
            }
            Verb::Unstick
                if task.run.is_none()
                    || !matches!(task.status.as_str(), "running" | "needs_me") =>
            {
                Some("This task has no active process to unstick.")
            }
            Verb::Revert if task.status != "done" => Some("Only a landed task can be reverted."),
            Verb::Discard if task.status == "done" => {
                Some("A landed task cannot be discarded; revert it instead.")
            }
            Verb::Discard if task.status == "dropped" => Some("This task is already dropped."),
            _ => None,
        };
        return reason.map(str::to_owned);
    }
    if let Record::Plan(id) = record
        && let Some(DetailBody::Plan(plan)) = feed
            .detail(&DetailRef::Plan { id: id.to_string() })
            .and_then(|cache| cache.body())
    {
        if matches!(
            plan.state,
            corral_protocol::rows::PlanState::Complete
                | corral_protocol::rows::PlanState::Cancelled
        ) {
            return Some("This plan has already finished.".into());
        }
        if matches!(verb, Verb::AddMember | Verb::MoveMember)
            && plan.role != Some(corral_protocol::rows::PlanRole::Tracker)
        {
            return Some("Membership actions require a published tracker.".into());
        }
    }
    None
}

pub(super) fn authority_reason(feed: &Feed, class: ActClass) -> Option<String> {
    let Some(session) = &feed.session else {
        return Some("Enrol or pair this device before acting.".into());
    };
    if !feed.capable(corral_protocol::app::ACT_V1) {
        return Some("This daemon does not support app commands.".into());
    }
    let permitted = match class {
        ActClass::Reply => session.authority.reply,
        ActClass::Act | ActClass::Confirm => session.authority.act,
        ActClass::Admin => session.authority.admin,
        ActClass::Refused | ActClass::Other => false,
    };
    (!permitted).then(|| "This device does not have the authority required by this action.".into())
}
