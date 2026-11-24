//! Engine operations this device submits (`op.submit { op, force }`), the one local question a
//! destructive or forced one asks first, and where each stands. The engine decides every
//! precondition; a refusal comes back with typed next steps, and the app offers each one.

use corral_types::{Next, Op, Refusal, Status};

use crate::model::daemon::Feed;
use crate::model::id::{Id, IdSupply};
use crate::model::live::Live;

/// What the one local confirm sheet asks about: an engine operation, a session control that
/// cannot be taken back by pressing again (Terminate, a rewind), ending a chat for good, or
/// leaving Configuration with unsaved edits, which drops them; `Leave` holds where the owner
/// was going.
#[derive(Debug, Clone, PartialEq)]
pub enum Confirming {
    Op(Act),
    Control(crate::model::agents::ControlAsk),
    /// End chat `chat` (`A-n`); `title` is `End thread A-n`.
    EndThread {
        chat: String,
        title: String,
    },
    Leave(Box<crate::model::nav::NavState>),
}

/// The leave question's title; its first word is what a terminal types to confirm.
pub const LEAVE_TITLE: &str = "Discard unsaved changes to corral.toml";

impl Confirming {
    /// Ending chat `chat` for good.
    pub fn end_thread(chat: String) -> Confirming {
        Confirming::EndThread {
            title: format!("End thread {chat}"),
            chat,
        }
    }

    pub fn title(&self) -> &str {
        match self {
            Self::Op(act) => &act.title,
            Self::Control(ask) => &ask.title,
            Self::EndThread { title, .. } => title,
            Self::Leave(_) => LEAVE_TITLE,
        }
    }

    /// The heading over the rows.
    pub fn heading(&self) -> &'static str {
        match self {
            Self::Op(_) | Self::Control(_) | Self::EndThread { .. } => {
                crate::model::commands::CONFIRM_EFFECT_HEADING
            }
            Self::Leave(_) => "Draft",
        }
    }

    /// The label-value rows the sheet lists.
    pub fn rows(&self) -> Vec<(String, String)> {
        match self {
            Self::Op(act) => act.rows(),
            Self::Control(ask) => ask.rows.clone(),
            Self::EndThread { chat, .. } => vec![(
                chat.clone(),
                "It stops answering and stays readable in Agents under Show All; your next message in Ask starts a new thread."
                    .into(),
            )],
            Self::Leave(_) => vec![(
                "corral.toml".into(),
                "Your edits are dropped; the file stays as it was last saved. Cancel keeps editing."
                    .into(),
            )],
        }
    }

    /// Whether the sheet asks about something sent to corrald (or simulated by the preview).
    pub fn sends(&self) -> bool {
        !matches!(self, Self::Leave(_))
    }
}

/// One intended engine operation.
#[derive(Debug, Clone, PartialEq)]
pub struct Act {
    /// The request id of this submission.
    pub key: Id,
    /// What the owner sees: "Dispatch anyway T-42".
    pub title: String,
    pub op: Op,
    pub force: bool,
    /// The record the operation concerns.
    pub about: Option<Live>,
    /// The press that made it already was its one local confirmation: a refusal sheet's forced
    /// entry, which lists what it skips, or the confirm sheet itself.
    pub confirmed: bool,
}

impl Act {
    pub fn new(ids: &mut IdSupply, op: Op, force: bool) -> Act {
        Act::with_key(ids.next_id(), op, force)
    }

    pub fn with_key(key: Id, op: Op, force: bool) -> Act {
        let title = match target(&op) {
            Some(id) => format!("{} {id}", label(&op, force)),
            None => label(&op, force),
        };
        let about = target(&op).map(|id| Live::Task(Id::new(id)));
        Act {
            key,
            title,
            op,
            force,
            about,
            confirmed: false,
        }
    }

    /// Destructive or forced operations ask once, locally, before they are sent: `drop`,
    /// `stop`, `approve`, `retry` with `fresh`, `move` to Done or Dropped, and anything forced.
    pub fn asks(&self) -> bool {
        self.force
            || matches!(
                &self.op,
                Op::Drop { .. }
                    | Op::Stop { .. }
                    | Op::Approve { .. }
                    | Op::Retry { fresh: true, .. }
                    | Op::Move {
                        status: Status::Done | Status::Dropped,
                        ..
                    }
            )
    }

    /// What a forced operation skips, as the engine's force table names it.
    pub fn skips(&self) -> Option<&'static str> {
        self.force.then(|| skips(&self.op)).flatten()
    }

    /// The operation and every argument it carries, as the confirm sheet lists them: the CLI
    /// line first, then what it skips when forced.
    pub fn rows(&self) -> Vec<(String, String)> {
        let next = Next {
            op: self.op.clone(),
            force: self.force,
        };
        let mut rows = vec![("runs".to_owned(), next.command())];
        if let Some(effect) = effect(&self.op) {
            rows.push(("effect".to_owned(), effect.to_owned()));
        }
        if let Some(skips) = self.skips() {
            rows.push(("skips".to_owned(), skips.to_owned()));
        }
        rows
    }
}
/// The sheet a refused operation opens: why, and one button per next step, the first one
/// recommended. A forced step reads "… anyway", lists what it skips and is the one confirm.
#[derive(Debug, Clone, PartialEq)]
pub struct RefusalSheet {
    /// The refused operation, "Dispatch T-42".
    pub title: String,
    pub refusal: Refusal,
}

/// One next step as its sheet button shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NextButton {
    pub label: String,
    /// The CLI line it runs, for the reader and the screen reader.
    pub command: String,
    /// For a forced step: what the engine will skip.
    pub skips: Option<&'static str>,
    pub primary: bool,
}

impl RefusalSheet {
    pub fn buttons(&self) -> Vec<NextButton> {
        self.refusal
            .next
            .iter()
            .enumerate()
            .map(|(index, next)| NextButton {
                label: label(&next.op, next.force),
                command: next.command(),
                skips: next.force.then(|| skips(&next.op)).flatten(),
                primary: index == 0,
            })
            .collect()
    }
}

/// The task an operation concerns.
pub fn target(op: &Op) -> Option<&str> {
    match op {
        Op::Edit { id, .. }
        | Op::Comment { id, .. }
        | Op::Move { id, .. }
        | Op::Dispatch { id, .. }
        | Op::Reply { id, .. }
        | Op::Stop { id }
        | Op::Retry { id, .. }
        | Op::Done { id, .. }
        | Op::Gate { id }
        | Op::Review { id, .. }
        | Op::Verdict { id, .. }
        | Op::Changes { id, .. }
        | Op::Approve { id, .. }
        | Op::Drop { id }
        | Op::Reopen { id }
        | Op::ArtifactAdd { id, .. } => Some(id),
        Op::New { .. }
        | Op::LensDraft { .. }
        | Op::LensRepair { .. }
        | Op::UsageAdd { .. }
        | Op::Sync => None,
    }
}

/// The verb a forced form puts before "anyway": "Move anyway", "Dispatch anyway",
/// "Land anyway".
const fn forced_verb(op: &Op) -> &'static str {
    match op {
        Op::Move { .. } => "Move",
        Op::Dispatch { .. } => "Dispatch",
        Op::Approve { .. } => "Land",
        _ => "Run",
    }
}

/// A button's words for an operation: what the menu or a refusal's next step says. An approve
/// naming its target says where it lands, so a refusal listing the existing branches reads as
/// one button per branch.
pub fn label(op: &Op, force: bool) -> String {
    if let Op::Approve {
        onto: Some(onto), ..
    } = op
    {
        return if force {
            format!("Land onto {onto} anyway")
        } else {
            format!("Approve onto {onto}")
        };
    }
    if force {
        return format!("{} anyway", forced_verb(op));
    }
    match op {
        Op::New { .. } => "New task".into(),
        Op::LensDraft { .. } => "Draft lens".into(),
        Op::LensRepair { .. } => "Repair lens".into(),
        Op::Edit { .. } => "Edit".into(),
        Op::Comment { .. } => "Comment".into(),
        Op::Move { status, .. } => format!("Move to {}", status.as_str()),
        Op::Dispatch { .. } => "Dispatch".into(),
        Op::Reply { .. } => "Reply".into(),
        Op::Stop { .. } => "Stop".into(),
        Op::Retry { fresh: true, .. } => "Retry from base".into(),
        Op::Retry { .. } => "Retry".into(),
        Op::Done { .. } => "Mark done".into(),
        Op::Gate { .. } => "Run gate".into(),
        Op::Review { .. } => "Start review".into(),
        Op::Verdict { approve: true, .. } => "Record approve".into(),
        Op::Verdict { .. } => "Record changes".into(),
        Op::Changes { .. } => "Request changes".into(),
        Op::Approve { .. } => "Approve and land".into(),
        Op::Drop { .. } => "Drop".into(),
        Op::Reopen { .. } => "Reopen".into(),
        Op::ArtifactAdd { .. } => "Add artifact".into(),
        Op::UsageAdd { .. } => "Add usage".into(),
        Op::Sync => "Sync".into(),
    }
}

/// What `force` skips for an operation, from the engine's force table.
pub const fn skips(op: &Op) -> Option<&'static str> {
    match op {
        Op::Move { .. } => Some("every status rule: any status to any status"),
        Op::Dispatch { .. } => Some("unfinished dependencies and the running-slot limit"),
        Op::Approve { .. } => Some("the gate check and the Review status rule"),
        _ => None,
    }
}

/// The lasting effect a destructive operation has beyond its name.
const fn effect(op: &Op) -> Option<&'static str> {
    match op {
        Op::Drop { .. } => Some("Moves the task to Dropped and stops its sessions."),
        Op::Stop { .. } => Some("Kills the live session; the task becomes Blocked."),
        Op::Approve { .. } => Some(
            "Rebases onto the target (the run's base unless --onto names another), fast-forwards it, removes the worktree and branch.",
        ),
        Op::Retry { fresh: true, .. } => {
            Some("Starts a new worker run from the base, not the task branch.")
        }
        Op::Move {
            status: Status::Done | Status::Dropped,
            ..
        } => Some("Stops the task's live sessions; its worktree and branch are kept."),
        _ => None,
    }
}

/// Where a submitted operation stands, as far as this device knows.
#[derive(Debug, Clone, PartialEq)]
pub enum ActStatus {
    /// Sent; no answer yet.
    Pending,
    /// The daemon answered that it is still running and named no operation to follow (an older
    /// corrald); the outcome arrives as the task's status.
    Running,
    Done,
    /// The engine refused it and named what gets past.
    Refused(Refusal),
    Failed(String),
}

impl ActStatus {
    /// One line for a receipt.
    pub fn line(&self) -> String {
        match self {
            ActStatus::Pending => "Sent · waiting for corrald".into(),
            ActStatus::Running => "Running · the task shows the outcome".into(),
            ActStatus::Done => "Done".into(),
            ActStatus::Refused(refusal) => format!("Refused: {}", refusal.reason),
            ActStatus::Failed(why) if why.is_empty() => "Failed".into(),
            ActStatus::Failed(why) => format!("Failed: {why}"),
        }
    }

    /// An outcome that will not change.
    pub fn settled(&self) -> bool {
        !matches!(self, ActStatus::Pending)
    }
}

/// The operation's status from the client's reply table, if it was sent.
pub fn status(feed: &Feed, key: &str) -> Option<ActStatus> {
    let operation = feed.operations.get(key)?;
    if let Some(refusal) = &operation.refusal {
        return Some(ActStatus::Refused(refusal.clone()));
    }
    if let Some(error) = &operation.error {
        return Some(ActStatus::Failed(error.message.clone()));
    }
    if operation.loading || operation.value.is_none() {
        return Some(ActStatus::Pending);
    }
    Some(match operation.decode::<corral_protocol::op::OpReply>() {
        // A named operation is followed to its outcome (`running_operation`).
        Some(corral_protocol::op::OpReply::Running {
            operation: Some(_), ..
        }) => ActStatus::Pending,
        Some(corral_protocol::op::OpReply::Running { .. }) => ActStatus::Running,
        _ => ActStatus::Done,
    })
}

/// What following a running operation needs next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Follow {
    /// Ask corrald for this operation's outcome.
    Ask(String),
    /// A request is in flight.
    Wait,
    /// The outcome is in, or the operation can no longer be followed.
    Finished,
}

/// Errors after which the same operation can be asked for again: the request never reached
/// corrald, or its reply was lost with the connection.
fn transient(code: &str) -> bool {
    matches!(code, "offline" | "disconnected" | "busy" | "unavailable")
}

/// The operation a `running` reply names: what is followed under its key.
pub fn running_operation(operation: &crate::model::daemon::Operation) -> Option<String> {
    if operation.loading || operation.error.is_some() {
        return None;
    }
    match operation.decode::<corral_protocol::op::OpReply>()? {
        corral_protocol::op::OpReply::Running { operation, .. } => operation,
        corral_protocol::op::OpReply::Done { .. } => None,
    }
}

/// The next step for operation `id`, followed under a key whose reply table entry is
/// `operation`. A lost reply keeps the id, so the outcome is asked for again, never guessed.
pub fn follow(operation: Option<&crate::model::daemon::Operation>, id: &str) -> Follow {
    let Some(operation) = operation else {
        return Follow::Finished;
    };
    if operation.loading {
        return Follow::Wait;
    }
    if let Some(error) = &operation.error {
        return if transient(&error.code) {
            Follow::Ask(id.to_owned())
        } else {
            Follow::Finished
        };
    }
    if operation.refusal.is_some() {
        return Follow::Finished;
    }
    match operation.decode::<corral_protocol::op::OpReply>() {
        Some(corral_protocol::op::OpReply::Running {
            operation: Some(next),
            ..
        }) => Follow::Ask(next),
        Some(_) => Follow::Finished,
        None => Follow::Wait,
    }
}

/// The task a finished operation reports, as the engine's reply names it: the id a new task got.
pub fn created(feed: &Feed, key: &str) -> Option<String> {
    match feed
        .operations
        .get(key)?
        .decode::<corral_protocol::op::OpReply>()?
    {
        corral_protocol::op::OpReply::Done { task, .. } => task,
        corral_protocol::op::OpReply::Running { .. } => None,
    }
}

/// The receipt a settled operation toasts. A new task has no id until the engine gives it one,
/// so its receipt names the id from the reply: "New task T-21 · Done".
pub fn receipt(feed: &Feed, act: &Act, status: &ActStatus) -> String {
    // A lens draft or repair files a task too; its receipt names it.
    let new = matches!(
        act.op,
        Op::New { .. } | Op::LensDraft { .. } | Op::LensRepair { .. }
    ) && *status == ActStatus::Done;
    match new.then(|| created(feed, &act.key)).flatten() {
        Some(task) => format!("{} {task} · {}", act.title, status.line()),
        None => format!("{} · {}", act.title, status.line()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dispatch() -> Op {
        Op::Dispatch {
            id: "T-42".into(),
            profile: None,
            base: None,
        }
    }

    #[test]
    fn only_destructive_or_forced_operations_ask_first() {
        let asks = |op: Op, force| Act::with_key(Id::new("k"), op, force).asks();
        assert!(!asks(dispatch(), false));
        assert!(asks(dispatch(), true));
        assert!(asks(Op::Drop { id: "T-1".into() }, false));
        assert!(asks(
            Op::Move {
                id: "T-1".into(),
                status: Status::Done,
            },
            false
        ));
        assert!(!asks(
            Op::Move {
                id: "T-1".into(),
                status: Status::Ready,
            },
            false
        ));
        assert!(asks(
            Op::Retry {
                id: "T-1".into(),
                profile: None,
                fresh: true,
                note: None,
            },
            false
        ));
    }

    #[test]
    fn a_forced_entry_reads_anyway_and_names_what_it_skips() {
        let act = Act::with_key(Id::new("k"), dispatch(), true);
        assert_eq!(act.title, "Dispatch anyway T-42");
        assert!(act.rows().iter().any(|(label, _)| label == "skips"));
        let land = Act::with_key(
            Id::new("k"),
            Op::Approve {
                id: "T-1".into(),
                onto: None,
            },
            true,
        );
        assert!(land.title.starts_with("Land anyway"));
    }
    /// A draft or repair that outlasts corrald's reply was left at "running" with no task. The
    /// named operation is followed under the same key until the outcome names the task.
    #[test]
    fn a_running_operation_is_followed_until_its_outcome_names_the_task() {
        use crate::model::daemon::{Feed, Operation};
        use corral_protocol::op::OpReply;
        let reply = |reply: OpReply| Operation {
            method: corral_protocol::op::OP_SUBMIT.into(),
            loading: false,
            value: serde_json::to_value(reply).ok(),
            error: None,
            refusal: None,
        };
        let mut feed = Feed::default();
        let running = reply(OpReply::Running {
            seq: 1,
            operation: Some("a-1".into()),
        });
        assert_eq!(running_operation(&running).as_deref(), Some("a-1"));
        assert_eq!(follow(Some(&running), "a-1"), Follow::Ask("a-1".into()));
        // The follow-up's reply is lost with the connection: the same operation is asked for.
        let lost = Operation {
            value: None,
            error: Some(crate::model::daemon::ClientError {
                code: "disconnected".into(),
                message: String::new(),
            }),
            ..running.clone()
        };
        assert_eq!(follow(Some(&lost), "a-1"), Follow::Ask("a-1".into()));
        feed.operations.insert(Id::new("k"), running);
        assert_eq!(status(&feed, "k"), Some(ActStatus::Pending));
        let done = reply(OpReply::Done {
            seq: 2,
            task: Some("T-9".into()),
        });
        assert_eq!(running_operation(&done), None);
        assert_eq!(follow(Some(&done), "a-1"), Follow::Finished);
        feed.operations.insert(Id::new("k"), done);
        assert_eq!(status(&feed, "k"), Some(ActStatus::Done));
        assert_eq!(created(&feed, "k").as_deref(), Some("T-9"));
    }
}
