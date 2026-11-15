//! Typed canonical commands the app asks corrald to run, the device authority they need, and
//! their truthful status. The command identity is allocated once per intended command and kept
//! through preview, confirmation, submission and recovery.

use corral_protocol::app::ActClass;
use corral_protocol::cmd::{self, Command};
use corral_protocol::rpc::ErrorCode;

use crate::model::daemon::{CommandStatus, Feed};
use crate::model::id::{Id, IdSupply};
use crate::model::live::Live;

/// A device authority a command or message needs (`AppSession.authority`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Authority {
    /// Answer questions, follow up, reply to a session.
    Reply,
    /// Submit allowlisted owner commands.
    Act,
    /// Talk to the assistant.
    Assistant,
    /// Change daemon administration and automation settings.
    Admin,
}

impl Authority {
    /// The refusal shown when the device lacks it; nothing is sent.
    pub const fn refusal(self) -> &'static str {
        match self {
            Authority::Reply => "Refused: this device may not answer or reply. Nothing was sent.",
            Authority::Act => "Refused: this device may not run owner commands. Nothing was sent.",
            Authority::Admin => {
                "Refused: this device may not administer corrald. Nothing was sent."
            }
            Authority::Assistant => {
                "Refused: this device may not message the assistant. Nothing was sent."
            }
        }
    }
}

/// One intended canonical command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Act {
    /// The `cmd_id`, the same through preview, submit and recovery.
    pub key: Id,
    /// What the owner sees: "Accept plan T-885 P2".
    pub title: String,
    pub command: Command,
    pub expect: Option<cmd::Expect>,
    /// Confirm-class: preview, then a fresh confirmation on this device. Set from the
    /// command's canonical class, so a caller cannot forget it; corrald still requires the
    /// proof and refuses without one.
    pub confirm: bool,
    /// The record the command concerns.
    pub about: Option<Live>,
}

impl Act {
    /// A new intended command, allocated from the reducer's injected identity state.
    pub fn new(ids: &mut IdSupply, title: impl Into<String>, command: Command) -> Act {
        Act::with_key(ids.next_id(), title, command)
    }

    /// The command under an identity the caller already holds.
    pub fn with_key(key: Id, title: impl Into<String>, command: Command) -> Act {
        let confirm = matches!(command.app_class(), ActClass::Confirm | ActClass::Admin);
        Act {
            key,
            title: title.into(),
            command,
            expect: None,
            confirm,
            about: None,
        }
    }

    pub fn expect(mut self, expect: cmd::Expect) -> Act {
        self.expect = Some(expect);
        self
    }

    pub fn confirmed(mut self) -> Act {
        self.confirm = true;
        self
    }

    pub fn about(mut self, live: Live) -> Act {
        self.about = Some(live);
        self
    }

    /// Use the same command classification as corrald, including annotation replies.
    pub fn authority(&self) -> Authority {
        match self.command.app_class() {
            ActClass::Reply => Authority::Reply,
            ActClass::Admin => Authority::Admin,
            _ => Authority::Act,
        }
    }

    /// What this command sends, read from the frozen command and never looked up again: the
    /// family and verb it runs, every argument it carries in full (a note as written), then the
    /// precondition it is bound to. Absent optional arguments are left out.
    pub fn command_rows(&self) -> Vec<(String, String)> {
        let mut rows = vec![("runs".to_owned(), self.command.name().replace('.', " "))];
        if let Ok(serde_json::Value::Object(mut fields)) = serde_json::to_value(&self.command)
            && let Some(serde_json::Value::Object(args)) = fields.remove("args")
        {
            rows.extend(args.into_iter().filter_map(|(name, value)| {
                let shown = match value {
                    serde_json::Value::Null => return None,
                    _ if name == "verb" => return None,
                    serde_json::Value::String(text) => text,
                    other => other.to_string(),
                };
                Some((name.replace('_', " "), shown))
            }));
        }
        if let Some(cmd::Expect::TaskRevision { task, revision }) = &self.expect {
            rows.push((
                "only if".to_owned(),
                format!("{task} is still at owner revision {revision}"),
            ));
        }
        rows
    }
}

/// Where a command stands, as far as this device knows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActStatus {
    /// Waiting for corrald's exact preview.
    Previewing,
    /// The preview is on screen, waiting for this device's confirmation.
    Confirming,
    /// Held by the client until the session can send it.
    Queued,
    /// Sent; no answer yet.
    Pending,
    Running,
    Done,
    Failed(String),
    Refused(String),
    /// A precondition moved; the detail shows the winner.
    Conflict(String),
    /// The result was lost and corrald could not say; never resent automatically.
    Unknown,
    /// corrald confirms it never ran.
    Abandoned,
}

impl ActStatus {
    /// One truthful line for a receipt, a row or a button.
    pub fn line(&self) -> String {
        match self {
            ActStatus::Previewing => "Preparing the exact preview…".into(),
            ActStatus::Confirming => "Waiting for your confirmation".into(),
            ActStatus::Queued => "Queued on this device".into(),
            ActStatus::Pending => "Sent · waiting for corrald".into(),
            ActStatus::Running => "Running".into(),
            ActStatus::Done => "Done".into(),
            ActStatus::Failed(why) => with_reason("Failed", why),
            ActStatus::Refused(why) => with_reason("Refused", why),
            ActStatus::Conflict(why) => with_reason("Changed meanwhile", why),
            ActStatus::Unknown => "Result unknown · it may have run; check before repeating".into(),
            ActStatus::Abandoned => "Not run · corrald never executed it".into(),
        }
    }

    /// A final outcome that will not change.
    pub fn settled(&self) -> bool {
        matches!(
            self,
            ActStatus::Done
                | ActStatus::Failed(_)
                | ActStatus::Refused(_)
                | ActStatus::Conflict(_)
                | ActStatus::Abandoned
        )
    }

    /// Sent without a known outcome: never repeat under a new identity.
    pub fn uncertain(&self) -> bool {
        matches!(
            self,
            ActStatus::Pending | ActStatus::Running | ActStatus::Unknown
        )
    }
}

fn with_reason(head: &str, why: &str) -> String {
    if why.is_empty() {
        head.to_owned()
    } else {
        format!("{head}: {why}")
    }
}

/// The command's status from the client's command journal, if it was ever sent.
pub fn status(feed: &Feed, key: &str) -> Option<ActStatus> {
    let state = feed.commands.get(key)?;
    let reason = || {
        state
            .result
            .as_ref()
            .and_then(|result| result.error.as_ref())
            .map(|error| error.message.clone())
            .or_else(|| state.error.as_ref().map(|error| error.message.clone()))
            .unwrap_or_default()
    };
    Some(match state.status {
        CommandStatus::Queued => ActStatus::Queued,
        CommandStatus::Pending => ActStatus::Pending,
        CommandStatus::Running => ActStatus::Running,
        CommandStatus::Done => ActStatus::Done,
        CommandStatus::Failed => ActStatus::Failed(reason()),
        CommandStatus::Refused => ActStatus::Refused(reason()),
        CommandStatus::Conflict => ActStatus::Conflict(reason()),
        CommandStatus::Unknown => ActStatus::Unknown,
        CommandStatus::Abandoned => ActStatus::Abandoned,
    })
}

/// Whether corrald refused the command only because it lacked a fresh confirmation: its typed
/// `confirm_required` error, read from the command's result first, as its status reason is.
/// Nothing ran; the same identity may be sent again with a proof.
pub fn needs_confirmation(feed: &Feed, key: &str) -> bool {
    feed.commands.get(key).is_some_and(|state| {
        let code = state
            .result
            .as_ref()
            .and_then(|result| result.error.as_ref())
            .map(|error| error.code.as_str())
            .or_else(|| state.error.as_ref().map(|error| error.code.as_str()));
        state.status == CommandStatus::Refused
            && code.is_some_and(|code| {
                serde_json::from_value::<ErrorCode>(code.into()).ok()
                    == Some(ErrorCode::ConfirmRequired)
            })
    })
}

/// What waits for a fresh confirmation on this device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirmable {
    Act(Act),
    /// An assistant permission prompt answered with presence.
    Prompt {
        answer: corral_protocol::assistant::PromptAnswer,
        title: String,
    },
    DeviceAuthority {
        key: Id,
        title: String,
        change: corral_protocol::app::DeviceSetAuthority,
    },
}

impl Confirmable {
    pub fn key(&self) -> Id {
        match self {
            Confirmable::Act(act) => act.key.clone(),
            Confirmable::Prompt { answer, .. } => Id::from(&answer.cmd_id),
            Confirmable::DeviceAuthority { key, .. } => key.clone(),
        }
    }

    pub fn title(&self) -> &str {
        match self {
            Confirmable::Act(act) => &act.title,
            Confirmable::Prompt { title, .. } | Confirmable::DeviceAuthority { title, .. } => title,
        }
    }
}

/// A live confirmation: the command, the correlation of corrald's preview of it, and that
/// preview once it arrived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirmation {
    pub subject: Confirmable,
    pub preview: Id,
    /// Kept with the open sheet: the client's bounded operation table may drop the reply
    /// while the owner is still reading it.
    pub ready: Option<corral_protocol::app::ActPreview>,
}

impl Confirmation {
    /// What the sheet can show and prove: the kept preview, else the operation's state.
    pub fn state(&self, feed: &Feed) -> PreviewState {
        match &self.ready {
            Some(ready) => PreviewState::Ready(ready.clone()),
            None => preview(feed, &self.preview),
        }
    }
}

/// What the confirmation sheet can show of corrald's preview so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewState {
    Loading,
    Ready(corral_protocol::app::ActPreview),
    Failed(String),
}

/// The preview's state from the client's operation table.
pub fn preview(feed: &Feed, correlation: &str) -> PreviewState {
    let Some(operation) = feed.operations.get(correlation) else {
        return PreviewState::Loading;
    };
    if let Some(error) = &operation.error {
        return PreviewState::Failed(error.message.clone());
    }
    match operation.decode::<corral_protocol::app::ActPreview>() {
        Some(preview) if preview.class == corral_protocol::app::ActClass::Refused => {
            PreviewState::Failed("corrald refuses this command for this device".into())
        }
        Some(preview) => PreviewState::Ready(preview),
        None if operation.loading || operation.value.is_none() => PreviewState::Loading,
        None => PreviewState::Failed("corrald sent a preview this app cannot read".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::daemon::{ClientError, CommandState, Operation};
    use corral_protocol::app::ActPreview;
    use corral_protocol::cmd::plan::PlanCommand;
    use corral_protocol::cmd::review::ReviewCommand;
    use corral_protocol::cmd::task::TaskCommand;

    #[test]
    fn only_outcomes_settle_and_lost_results_stay_uncertain() {
        assert!(ActStatus::Done.settled());
        assert!(ActStatus::Abandoned.settled());
        assert!(!ActStatus::Unknown.settled());
        assert!(ActStatus::Unknown.uncertain());
        assert!(ActStatus::Pending.uncertain());
        assert!(!ActStatus::Refused(String::new()).uncertain());
        assert_eq!(ActStatus::Refused(String::new()).line(), "Refused");
    }

    #[test]
    fn confirm_class_commands_preview_before_they_are_sent() {
        for command in [
            Command::Review(ReviewCommand::Approve {
                id: "T-2".into(),
                sha: "abc1234".into(),
            }),
            Command::Plan(PlanCommand::Accept {
                id: "T-2".into(),
                revision: "P1".into(),
                owner_ok: true,
                notes: None,
            }),
            Command::Task(TaskCommand::Revert { id: "T-2".into() }),
            Command::Task(TaskCommand::Move {
                id: "T-2".into(),
                status: "done".into(),
                force: false,
                owner_ok: true,
                reason: None,
            }),
        ] {
            assert_eq!(command.app_class(), ActClass::Confirm);
            let act = Act::with_key(Id::new("app-1"), command.name(), command);
            assert!(act.confirm, "{} must preview first", act.command.name());
        }
    }

    #[test]
    fn act_and_reply_class_commands_do_not_require_a_preview() {
        for command in [
            Command::Review(ReviewCommand::RequestChanges {
                id: "T-2".into(),
                text: "add a check".into(),
                escalate: false,
            }),
            Command::Plan(PlanCommand::RequestChanges {
                id: "T-2".into(),
                revision: "P1".into(),
                text: "add a check".into(),
            }),
            Command::Task(TaskCommand::Retry {
                id: "T-2".into(),
                run: None,
                profile: None,
            }),
        ] {
            assert_eq!(command.app_class(), ActClass::Act);
            let act = Act::with_key(Id::new("app-2"), command.name(), command);
            assert!(
                !act.confirm,
                "{} must not require a preview",
                act.command.name()
            );
        }
        let command = Command::Task(TaskCommand::Reply {
            id: "T-2".into(),
            text: "ok".into(),
        });
        assert_eq!(command.app_class(), ActClass::Reply);
        assert!(!Act::with_key(Id::new("app-3"), "Reply to T-2", command).confirm);
    }

    fn refused(code: &str) -> CommandState {
        CommandState {
            device: None,
            method: "act.submit".into(),
            request: serde_json::Value::Null,
            status: CommandStatus::Refused,
            result: None,
            error: Some(ClientError {
                code: code.into(),
                message: "confirmation_required".into(),
            }),
            entry: None,
        }
    }

    #[test]
    fn uncertain_reducer_retries_keep_the_injected_command_identity() {
        use crate::model::assistant::{Action, State};
        use crate::model::effect::Effect;

        let mut state = State {
            ids: IdSupply::new("launch-c"),
            ..State::default()
        };
        let mut feed = Feed::default();
        let action = Action::FileEntry("entry-1".into());
        let effects = state.act_live(action.clone(), &feed);
        let key = match effects.as_slice() {
            [Effect::Act(act)] => act.key.clone(),
            other => panic!("expected one command, got {other:?}"),
        };
        let mut command = refused("disconnected");
        command.status = CommandStatus::Unknown;
        feed.commands.insert(key.clone(), command);
        let ids_after_intent = state.ids.clone();
        let retry = state.act_live(action, &feed);
        assert!(matches!(retry.as_slice(), [Effect::Recover(same)] if *same == key));
        assert_eq!(state.ids, ids_after_intent);
        assert_eq!(status(&feed, &key), Some(ActStatus::Unknown));
    }

    #[test]
    fn only_the_typed_confirm_required_refusal_asks_for_a_confirmation() {
        let mut feed = Feed::default();
        feed.commands
            .insert(Id::new("app-1"), refused("confirm_required"));
        feed.commands.insert(Id::new("app-2"), refused("refused"));
        let mut failed = refused("confirm_required");
        failed.status = CommandStatus::Failed;
        feed.commands.insert(Id::new("app-3"), failed);
        assert!(needs_confirmation(&feed, "app-1"));
        assert!(!needs_confirmation(&feed, "app-2"));
        assert!(!needs_confirmation(&feed, "app-3"));
        assert!(!needs_confirmation(&feed, "app-4"));
    }

    #[test]
    fn an_open_sheet_keeps_its_preview_after_the_reply_cache_drops_it() {
        let ready = ActPreview {
            preview_id: "p-1".into(),
            digest: "d-1".into(),
            class: ActClass::Confirm,
            effects: vec!["Execute plan.accept as owner".into()],
            expires_at: "2026-11-14T16:20:53Z".into(),
            operation_token: None,
        };
        let mut feed = Feed::default();
        feed.operations.insert(
            Id::new("op-1"),
            Operation {
                method: "act.preview".into(),
                loading: false,
                value: Some(serde_json::to_value(&ready).unwrap()),
                error: None,
            },
        );
        let mut sheet = Confirmation {
            subject: Confirmable::Act(Act::with_key(
                Id::new("app-1"),
                "Accept plan T-7 P3",
                Command::Task(TaskCommand::Reply {
                    id: "T-7".into(),
                    text: "ok".into(),
                }),
            )),
            preview: Id::new("op-1"),
            ready: None,
        };
        assert_eq!(sheet.state(&feed), PreviewState::Ready(ready.clone()));
        feed.operations.clear();
        assert_eq!(sheet.state(&feed), PreviewState::Loading);
        sheet.ready = Some(ready.clone());
        assert_eq!(sheet.state(&feed), PreviewState::Ready(ready));
    }
}
