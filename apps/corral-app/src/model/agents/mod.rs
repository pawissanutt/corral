//! Every session the daemon addresses (runs of every role, the assistant's chats, build
//! sessions) as Sessions and Home list them (`lines`), where sessions start (the New menu and
//! the New build sheet), the Agents switcher over the assistant's conversations with this
//! device's pins (`switcher`), each session's run controls and Session menu, and the Remove
//! sheet of a stopped build: what removing it would delete, as prune's dry run says, before it is
//! removed. A line opens the session page (`model::session`), which every role shares.

use std::collections::BTreeMap;

use corral_protocol::build::{BuildRemove, BuildRemoved};

use crate::model::Id;
use crate::model::daemon::{Feed, Operation};
use crate::model::effect::Effect;
use crate::model::projects::ProjectFilter;

mod control;
mod lines;
mod live;
mod menu;
mod source;
pub(crate) use source::is_code;
#[cfg(test)]
pub(crate) mod source_tests;
mod start;
pub mod switcher;

pub use control::{
    Control, ControlAsk, Kind, RewindPoint, RewindSheet, ask, control_label, control_verbs,
    controls, kind, rewind_blocked, rewind_can, rewind_offered, rewind_sheet,
    status as control_status,
};
pub use lines::{SessionLine, removable, session_line, session_lines};
pub(crate) use live::{briefed_question, clock, placement, unix};
pub use menu::{SESSION_MENU_LABEL, session_verbs};
pub use start::{
    Choice, EffortField, ModelField, ModelPick, NameField, NewBuildForm, NewItem, NewMenu,
    NewPress, Place, PlaceField, ProjectField, Start, StartAction, StartButton, form, new_menu,
};
pub use switcher::{AGENTS_KEY, Pins, Switcher, exact, switcher};

/// Sessions' interaction state: the list's folds, the latest control per session, the rewind
/// sheet, the New build sheet and the Remove sheet.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct State {
    pub ids: crate::model::id::IdSupply,
    /// The Sessions list's folds (its selection is the navigation's, `selection.session`).
    pub list: crate::model::sessions::State,
    /// The latest session control per session, whose outcome it shows.
    pub controls: BTreeMap<Id, control::ControlSent>,
    /// The open rewind sheet.
    pub rewind: Option<control::Rewinding>,
    pub start: Start,
    /// The open Remove sheet.
    pub removing: Option<Removing>,
}

/// A stopped build being removed: the dry run asked first, then the removal once confirmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removing {
    pub session: Id,
    /// The dry run's request key.
    pub dry: Id,
    /// The removal's request key, once confirmed.
    pub sent: Option<Id>,
}

/// Where the Remove sheet stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoveStatus {
    /// Asking prune what it would remove.
    Checking,
    /// The dry run answered.
    Ready,
    Removing,
    /// corrald or prune refused, with its reason.
    Failed(String),
}

/// `Remove build docs-sweep?`: what the removal deletes, as prune's dry run reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveSheet {
    pub session: Id,
    pub title: String,
    /// `the worktree`, `the branch`, `the record`: what the dry run says goes.
    pub removes: Vec<&'static str>,
    /// Prune's own lines.
    pub lines: Vec<String>,
    /// Why nothing, or not everything, would be removed.
    pub kept: Option<String>,
    pub status: RemoveStatus,
    /// `Remove` (`btn-remove-confirm`): the one confirmation; none until the dry run answered,
    /// and none when it would remove nothing.
    pub confirm: Option<Action>,
    pub cancel: Action,
}

fn refused(operation: &Operation) -> Option<String> {
    operation
        .refusal
        .as_ref()
        .map(|refusal| refusal.reason.clone())
        .or_else(|| operation.error.as_ref().map(|error| error.message.clone()))
}

/// How request `key` stands: none while it is on its way, else its reply or its refusal.
fn outcome(feed: &Feed, key: &Id) -> Option<Result<Option<BuildRemoved>, String>> {
    let operation = feed.operations.get(key).filter(|found| !found.loading)?;
    Some(match refused(operation) {
        Some(why) => Err(why),
        None => Ok(operation.decode::<BuildRemoved>()),
    })
}

fn removes(reply: &BuildRemoved) -> Vec<&'static str> {
    [
        (reply.worktree, "the worktree"),
        (reply.branch, "the branch"),
        (reply.record, "the record"),
    ]
    .into_iter()
    .filter_map(|(gone, what)| gone.then_some(what))
    .collect()
}

/// The open Remove sheet.
pub fn remove_sheet(state: &State, feed: &Feed) -> Option<RemoveSheet> {
    let removing = state.removing.as_ref()?;
    let name = feed
        .assistants
        .get(removing.session.as_str())
        .and_then(|row| row.build.as_ref())
        .map_or_else(|| removing.session.to_string(), |build| build.name.clone());
    let dry = outcome(feed, &removing.dry);
    let reply = match &dry {
        Some(Ok(reply)) => reply.clone(),
        _ => None,
    };
    let status = match (&removing.sent, dry) {
        (Some(sent), _) => match outcome(feed, sent) {
            Some(Err(why)) => RemoveStatus::Failed(why),
            _ => RemoveStatus::Removing,
        },
        (None, None) => RemoveStatus::Checking,
        (None, Some(Err(why))) => RemoveStatus::Failed(why),
        (None, Some(Ok(_))) => RemoveStatus::Ready,
    };
    let removes = reply.as_ref().map(removes).unwrap_or_default();
    let confirmable = !removes.is_empty()
        && match &status {
            RemoveStatus::Ready => true,
            RemoveStatus::Failed(_) => removing.sent.is_some(),
            RemoveStatus::Checking | RemoveStatus::Removing => false,
        };
    Some(RemoveSheet {
        session: removing.session.clone(),
        title: format!("Remove build {name}?"),
        removes,
        lines: reply
            .as_ref()
            .map(|reply| reply.lines.clone())
            .unwrap_or_default(),
        kept: reply.and_then(|reply| reply.kept),
        status,
        confirm: confirmable.then_some(Action::ConfirmRemove),
        cancel: Action::CloseRemove,
    })
}

/// What the rewind sheet, the New build sheet and the Remove sheet do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Rewind the open rewind sheet's session to the start of this turn, after one confirm.
    RewindTo(u64),
    CloseRewind,
    /// Open the Remove sheet of a stopped build: prune's dry run first.
    Remove(Id),
    /// Remove the build the Remove sheet shows, as its dry run said.
    ConfirmRemove,
    CloseRemove,
    /// The New build sheet.
    Start(StartAction),
}

impl State {
    /// Handles an action. Every operation goes out as an effect; nothing is simulated here.
    pub fn act(&mut self, action: Action, feed: &Feed, filter: &ProjectFilter) -> Vec<Effect> {
        match action {
            Action::RewindTo(turn) => self.rewind_to(turn),
            Action::CloseRewind => {
                self.rewind = None;
                Vec::new()
            }
            Action::Remove(session) => {
                let key = self.ids.next_id();
                self.removing = Some(Removing {
                    session: session.clone(),
                    dry: key.clone(),
                    sent: None,
                });
                vec![Effect::BuildRemove {
                    key,
                    remove: BuildRemove {
                        id: session.to_string(),
                        dry_run: true,
                    },
                }]
            }
            Action::ConfirmRemove => {
                let confirmable =
                    remove_sheet(self, feed).is_some_and(|sheet| sheet.confirm.is_some());
                let key = self.ids.next_id();
                let Some(removing) = self.removing.as_mut().filter(|_| confirmable) else {
                    return Vec::new();
                };
                removing.sent = Some(key.clone());
                vec![Effect::BuildRemove {
                    key,
                    remove: BuildRemove {
                        id: removing.session.to_string(),
                        dry_run: false,
                    },
                }]
            }
            Action::CloseRemove => {
                self.removing = None;
                Vec::new()
            }
            Action::Start(action) => self.start.act(action, &mut self.ids, feed, filter),
        }
    }

    /// Follows the feed: tells each settled control once, opens a started build's page, and
    /// closes the Remove sheet once its build is removed.
    pub fn settle(&mut self, feed: &Feed) -> Vec<Effect> {
        let mut effects = self.settle_controls(feed);
        effects.extend(self.start.settle(feed));
        let removed = self.removing.as_ref().and_then(|removing| {
            let sent = removing.sent.as_ref()?;
            matches!(outcome(feed, sent), Some(Ok(_))).then(|| removing.session.clone())
        });
        if let Some(session) = removed {
            self.removing = None;
            effects.push(Effect::Toast(format!("Removed {session}")));
        }
        effects
    }
}
