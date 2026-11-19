//! The Agents destination: one roster of every session the daemon addresses (runs of every role,
//! the assistant's chats, build sessions), where sessions start (the New menu and the New build
//! sheet), and each session's run controls and Session menu. A row opens the session page
//! (`model::session`), which every role shares.

use std::collections::BTreeMap;

use crate::model::Id;
use crate::model::daemon::Feed;
use crate::model::effect::Effect;
use crate::model::projects::ProjectFilter;

mod control;
mod live;
mod menu;
mod prompt;
mod roster;
#[cfg(test)]
pub(crate) mod roster_tests;
mod start;

pub use control::{
    Control, ControlAsk, Kind, RewindPoint, RewindSheet, ask, control_label, control_verbs,
    controls, kind, rewind_sheet, status as control_status,
};
pub(crate) use live::{clock, placement, unix};
pub use menu::{SESSION_MENU_LABEL, session_verbs};
pub use prompt::{Prompt, QUESTION_CARD, answer as answer_prompt, prompt};
pub use roster::{RoleTab, Roster, RosterRole, RosterRow, RowState, Show, ShowTab, roster};
pub use start::{
    Choice, EffortField, ModelField, ModelPick, NameField, NewBuildForm, NewItem, NewMenu,
    NewPress, Place, PlaceField, ProjectField, Start, StartAction, StartButton, form, new_menu,
};

/// Agents interaction state: the roster's selection and filters, the latest control per
/// session, the rewind sheet and the New build sheet.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct State {
    pub ids: crate::model::id::IdSupply,
    /// Roster selection belongs to this screen, not the session route.
    pub roster_session: Option<Id>,
    pub roster_role: RosterRole,
    pub show: Show,
    /// The latest session control per session, whose outcome it shows.
    pub controls: BTreeMap<Id, control::ControlSent>,
    /// The open rewind sheet.
    pub rewind: Option<control::Rewinding>,
    pub start: Start,
}

/// What the roster, the Session menu, the rewind sheet and the New build sheet do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    SelectRoster(Id),
    ClearRoster,
    SetRosterRole(RosterRole),
    SetShow(Show),
    OpenSession(Id),
    Back,
    /// Send a session control (the confirm sheet first for one that asks).
    Control {
        session: Id,
        control: Control,
    },
    /// Rewind the open rewind sheet's session to the start of this turn, after one confirm.
    RewindTo(u64),
    CloseRewind,
    /// Answer the agent's question with an option, words, or both.
    Answer {
        session: Id,
        question: String,
        option: Option<String>,
        text: Option<String>,
    },
    /// The New build sheet.
    Start(StartAction),
}

impl State {
    /// Handles an action. Every operation goes out as an effect; nothing is simulated here.
    pub fn act(&mut self, action: Action, feed: &Feed, filter: &ProjectFilter) -> Vec<Effect> {
        match action {
            Action::SelectRoster(id) => {
                self.roster_session = Some(id);
                Vec::new()
            }
            Action::ClearRoster => {
                self.roster_session = None;
                Vec::new()
            }
            Action::SetRosterRole(role) => {
                self.roster_role = role;
                Vec::new()
            }
            Action::SetShow(show) => {
                self.show = show;
                Vec::new()
            }
            Action::OpenSession(id) => {
                vec![Effect::OpenLive(crate::model::Live::Session(id))]
            }
            Action::Back => vec![Effect::Go(crate::model::Destination::Agents)],
            Action::Control { session, control } => vec![Effect::Control {
                session,
                key: self.ids.next_id(),
                control,
                confirmed: false,
            }],
            Action::RewindTo(turn) => self.rewind_to(turn),
            Action::CloseRewind => {
                self.rewind = None;
                Vec::new()
            }
            Action::Answer {
                session,
                question,
                option,
                text,
            } => prompt::answer(&session, question, option, text)
                .into_iter()
                .collect(),
            Action::Start(action) => self.start.act(action, &mut self.ids, feed, filter),
        }
    }

    /// Follows the feed: tells each settled control once, and opens a started build's page.
    pub fn settle(&mut self, feed: &Feed) -> Vec<Effect> {
        let mut effects = self.settle_controls(feed);
        effects.extend(self.start.settle(feed));
        effects
    }
}
