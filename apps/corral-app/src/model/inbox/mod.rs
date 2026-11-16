//! Pure inbox state, canonical actions and feed-backed handover descriptors.

mod attention;
mod context;
#[cfg(test)]
mod context_tests;
mod list;
mod live;
mod live_background;
mod live_evidence;
mod live_list;
mod live_notice;
mod live_sheet;
mod provenance;
mod sheet;
mod sheet_types;
mod view;

pub use attention::{AttentionLine, attention_counts_filtered, attention_line, owner_moves};
pub use context::{question_context_refs, question_dependencies};
pub use list::*;
pub use live::{
    Desk, Draft, Sent, can_answer, choice, clock, is_open, is_question, ordered, ordered_filtered,
    question_detail, question_ref, question_state as served_state, reply_value, selected, target,
};
pub use provenance::{provenance, via_name};
pub use sheet::*;
pub use view::*;

use crate::model::effect::Env;
use crate::model::nav::{Destination, Form};
use crate::model::{Id, Live};

/// Dependency emphasis in a question's situation band.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitKind {
    You,
    Plan,
    Info,
}

/// Effective question state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuestionState {
    /// Still answerable.
    Open,
    /// This device's answer is on its way to corrald.
    Pending,
    /// This device's answer may or may not have been applied; never resent automatically.
    Uncertain,
    /// Answered by the owner on this device.
    Mine,
    /// Answered on another device, the PM pane or by an agent, making this copy read-only.
    Elsewhere,
    /// Cancelled or expired.
    Closed,
}

/// Inbox-local selection, drafts and transient interaction state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct State {
    pub ids: crate::model::id::IdSupply,
    /// Whether the phone thread sheet is open.
    pub thread_open: bool,
    /// Served: selection, drafts and sent commands.
    pub desk: Desk,
    /// Grows by one when “Ask the asker” moves focus to the follow-up field.
    pub reply_focus: u32,
}

/// Every interaction carried by the inbox builders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Open the phone thread sheet.
    Thread,
    /// Close that sheet without changing the question.
    CloseThread,
    /// Navigate to a destination.
    Go(Destination),
    /// Served: select a row in place.
    Row(Id),
    /// Served: open a row's own page.
    OpenRow(Id),
    /// Served: open a live route.
    Live(Live),
    /// Served: choose an option of a question.
    Pick(Id, usize),
    /// Served: select a question's evidence block.
    Proof(Id, usize),
    /// Served: edit a question's alternate answer.
    Text(Id, String),
    /// Served: edit the follow-up to the asker.
    Reply(Id, String),
    /// Served: answer the question at its canonical revision.
    Submit(Id),
    /// Served: post the follow-up to the asker.
    Followup(Id),
    /// Served: ask corrald for the result of an uncertain command, by its same identity.
    Recover(Id),
    /// Served: revalidate a question's detail.
    Refetch(Id),
    /// Served: ask about the question, to the PM when true.
    AskAbout(Id, bool),
    /// Served: move focus to the follow-up field.
    FocusReply,
    /// Served: open the next open item after this one.
    Next(Id),
}

impl Action {
    /// Edit the alternate answer when a question is selected.
    pub fn draft(key: Option<&Id>, text: String) -> Option<Action> {
        key.map(|key| Action::Text(key.clone(), text))
    }
}

/// The inbox's letter commands, bound wherever the focus is outside a text field: `f` opens the
/// selected row, `e` cycles a question's evidence. Its list keys (moves, Enter, digits) are
/// `commands::list_keys`, bound only while the list has the focus.
pub const KEYS: &[&str] = &["f", "e"];

impl State {
    /// The alternate answer for the selected question.
    pub fn draft_of(&self, key: Option<&Id>) -> String {
        key.and_then(|key| self.desk.draft(key))
            .map(|draft| draft.text.clone())
            .unwrap_or_default()
    }

    /// A copy without any typed text, so views that do not show it are not made again
    /// while the owner types.
    pub fn blind(&self) -> State {
        let mut without = self.clone();
        for draft in without.desk.drafts.values_mut() {
            draft.text.clear();
            draft.reply.clear();
        }
        without
    }
}

pub(crate) fn phone(env: Env) -> bool {
    env.form == Form::Phone
}
