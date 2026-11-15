//! Inbox and question content. Render [`inbox`] for the list and selection preview, and
//! [`question`] for the focused question; [`sheet_desk`] also exposes the inline desktop
//! handover. Each takes a [`Src`]: the explicitly selected fixture preview, or the served
//! `feed.inbox` rows and question details. Builders own wording, section order, state stamps
//! and connection branches; the renderer only supplies presentation and dispatches [`Action`]
//! through [`State::act`] (fixture) or [`State::act_live`] (served).

mod attention;
mod context;
#[cfg(test)]
mod context_tests;
mod data;
mod evidence;
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

pub use attention::{AttentionLine, attention_counts, attention_line, owner_moves};
pub use context::{question_context_refs, question_dependencies};
pub use data::*;
pub use evidence::assessment;
pub use list::*;
pub use live::{
    Desk, Draft, Sent, can_answer, choice, clock, is_open, is_question, ordered, question_detail,
    question_ref, question_state as served_state, reply_value, selected, target,
};
pub use provenance::{provenance, via_name};
pub use sheet::*;
pub use view::*;

use crate::model::effect::{Conn, DataMode, Effect, Env, Page};
use crate::model::nav::{Destination, Form};
use crate::model::{Id, Live};

/// The five fixture rows in keyboard-selection order, including the assistant's two prompts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Selection {
    /// The blocking worker question.
    #[default]
    Question,
    /// The plan awaiting acceptance.
    Plan,
    /// The illustrative PM question.
    Pm,
    /// The assistant's file-write permission prompt.
    Permission,
    /// The assistant's mode-confirmation prompt.
    Confirmation,
}

impl Selection {
    /// List row order for j/k, even when an answered row remains stamped in place.
    pub const ALL: [Selection; 5] = [
        Self::Question,
        Self::Plan,
        Self::Pm,
        Self::Permission,
        Self::Confirmation,
    ];
}

/// The selected fixture evidence tab, in the source's cycle order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Evidence {
    /// The real patch hunk.
    #[default]
    Diff,
    /// The HostQueueObserver interface.
    File,
    /// Illustrative gate tail with a real title.
    Log,
    /// Illustrative rendered terminal screenshot with a real title.
    Shot,
}

impl Evidence {
    /// Tabs and the cycle order for e.
    pub const ALL: [Evidence; 4] = [Self::Diff, Self::File, Self::Log, Self::Shot];
}

/// Locally stored fixture answer state; the elsewhere data mode overrides it for display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    /// Awaiting an owner answer.
    #[default]
    Open,
    /// Revision-bound answer waits on this device.
    Queued,
    /// This owner answered on this device.
    Mine,
}

/// Effective question state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuestionState {
    /// Still answerable.
    Open,
    /// Fixture only: queued locally, still showing editable options as in the source.
    Queued,
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

/// The complete inbox and question state: the fixture's mockup state and the served desk.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct State {
    /// Fixture: selected inbox row.
    pub sel: Selection,
    /// Fixture: zero-based answer option; reducer inputs only accept existing options.
    pub choice: usize,
    /// Fixture: evidence tab currently selected.
    pub evidence: Evidence,
    /// Fixture: locally stored answer status.
    pub status: Status,
    /// Fixture: the alternate-answer textarea.
    pub draft: String,
    /// Whether the phone thread sheet is open.
    pub thread_open: bool,
    /// Served: selection, drafts and sent commands.
    pub desk: Desk,
    /// Grows by one when “Ask the asker” moves focus to the follow-up field.
    pub reply_focus: u32,
}

/// Every interaction carried by the inbox builders. The first group is the fixture
/// preview's; the second carries served ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Select the inline desktop row.
    Select(Selection),
    /// Open the question page from a list or card.
    OpenQuestion,
    /// Open the plan review by fixture id.
    PlanOpen(&'static str),
    /// Choose a zero-based radio option.
    Choose(usize),
    /// Select an evidence tab or an option's proof link.
    Evidence(Evidence),
    /// Edit the alternate answer.
    Draft(String),
    /// Answer the open revision or queue it while disconnected.
    Answer,
    /// Open the phone thread sheet.
    Thread,
    /// Close that sheet without changing the question.
    CloseThread,
    /// Show the source's illustrative reply-send toast, without appending a fake message.
    SendThread,
    /// Move to the next item, plan T-885.
    NextQ,
    /// Navigate to a destination.
    Go(Destination),
    /// Open a fixture task/session/thread page.
    Open(Page),
    /// Ask about the question with its context chip.
    Ask,
    /// Execute an explicitly toast-only mockup control.
    Toast(&'static str),
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
    /// The alternate-answer edit for a served question, or the fixture's.
    pub fn draft(key: Option<&Id>, text: String) -> Action {
        match key {
            Some(key) => Action::Text(key.clone(), text),
            None => Action::Draft(text),
        }
    }
}

/// The inbox's letter commands, bound wherever the focus is outside a text field: `f` opens the
/// selected row, `e` cycles a question's evidence. Its list keys (moves, Enter, digits) are
/// `commands::list_keys`, bound only while the list has the focus.
pub const KEYS: &[&str] = &["f", "e"];

impl State {
    /// Resolve the fixture's elsewhere override without losing its stored answer state.
    pub fn question_state(&self, env: Env) -> QuestionState {
        if env.mode == DataMode::Elsewhere {
            return QuestionState::Elsewhere;
        }
        match self.status {
            Status::Open => QuestionState::Open,
            Status::Queued => QuestionState::Queued,
            Status::Mine => QuestionState::Mine,
        }
    }

    /// The alternate answer the field shows: the served question's, else the fixture's.
    pub fn draft_of(&self, key: Option<&Id>) -> String {
        match key {
            Some(key) => self
                .desk
                .draft(key)
                .map(|draft| draft.text.clone())
                .unwrap_or_default(),
            None => self.draft.clone(),
        }
    }

    /// A copy without any typed text, so views that do not show it are not made again
    /// while the owner types.
    pub fn blind(&self) -> State {
        let mut without = self.clone();
        without.draft.clear();
        for draft in without.desk.drafts.values_mut() {
            draft.text.clear();
            draft.reply.clear();
        }
        without
    }

    /// Dispatch a fixture action; all nonlocal work is returned as effects.
    pub fn act(&mut self, action: Action, env: Env) -> Vec<Effect> {
        match action {
            Action::Select(sel) => self.inbox_sel(sel),
            Action::OpenQuestion => self.open_question(),
            Action::PlanOpen(id) => self.plan_open(id),
            Action::Choose(choice) => self.choose(choice),
            Action::Evidence(evidence) => {
                self.evidence = evidence;
                Vec::new()
            }
            Action::Draft(draft) => {
                self.draft = draft;
                Vec::new()
            }
            Action::Answer => self.answer(env),
            Action::Thread => self.thread(),
            Action::CloseThread => {
                self.thread_open = false;
                Vec::new()
            }
            Action::SendThread => self.send_thread(),
            Action::NextQ => self.next_q(),
            Action::Go(to) => vec![Effect::Go(to)],
            Action::Open(page) => vec![Effect::Open(page)],
            Action::Ask => vec![Effect::Ask {
                chip: Some(Q.key),
                text: None,
            }],
            Action::Toast(text) => vec![Effect::Toast(text.into())],
            // Served actions are dispatched through `act_live`.
            _ => Vec::new(),
        }
    }

    /// Select a desktop inbox row without opening a focused page.
    pub fn inbox_sel(&mut self, sel: Selection) -> Vec<Effect> {
        self.sel = sel;
        Vec::new()
    }

    /// Push the worker question, preserving option, evidence and draft state.
    pub fn open_question(&mut self) -> Vec<Effect> {
        vec![Effect::Open(Page::Question)]
    }

    /// Choose a valid option. Keyboard handlers deliberately remain active after answering,
    /// matching the source even though the rendered radio controls become disabled.
    pub fn choose(&mut self, choice: usize) -> Vec<Effect> {
        if choice < Q.options.len() {
            self.choice = choice;
        }
        Vec::new()
    }

    /// Answer only an open question. Both offline and down queue the checkable revision;
    /// a queued, locally answered or elsewhere-answered question ignores another answer.
    /// The source toast names the numbered option even when an alternate draft is present.
    pub fn answer(&mut self, env: Env) -> Vec<Effect> {
        if self.question_state(env) != QuestionState::Open {
            return Vec::new();
        }
        if matches!(env.conn, Conn::Offline | Conn::Down) {
            self.status = Status::Queued;
            vec![Effect::Toast(format!(
                "Queued: {} · option {} · sends on reconnect if rev {} is current",
                Q.key,
                self.choice + 1,
                Q.rev
            ))]
        } else {
            self.status = Status::Mine;
            vec![Effect::Toast(format!(
                "Answered {} · option {} · you on {}",
                Q.key,
                self.choice + 1,
                env.device()
            ))]
        }
    }

    /// Open the thread sheet; its title, body, reply and Done control come from the view.
    pub fn thread(&mut self) -> Vec<Effect> {
        self.thread_open = true;
        Vec::new()
    }

    /// Preserve the source's illustrative send action without inventing thread persistence.
    pub fn send_thread(&mut self) -> Vec<Effect> {
        vec![Effect::Toast(
            "Sent to worker T-868 · posted to the question thread by you".into(),
        )]
    }

    /// Open T-885 as the next question-like item, in either form.
    pub fn next_q(&mut self) -> Vec<Effect> {
        self.plan_open("T-885")
    }

    /// Open the supplied plan on desktop, phone or terminal through the app router.
    pub fn plan_open(&mut self, id: &'static str) -> Vec<Effect> {
        vec![Effect::Open(Page::Plan(id))]
    }

    /// Port the screen-local key handlers; only Page::Question selects the focus handler.
    /// Unhandled keys return None for the shell's global navigation/answer/back bindings.
    pub fn key(&mut self, key: &str, page: Option<Page>, _env: Env) -> Option<Vec<Effect>> {
        if page == Some(Page::Question) {
            return match key {
                "1" | "k" => Some(self.choose(0)),
                "2" | "j" => Some(self.choose(1)),
                "e" => {
                    self.evidence = match self.evidence {
                        Evidence::Diff => Evidence::File,
                        Evidence::File => Evidence::Log,
                        Evidence::Log => Evidence::Shot,
                        Evidence::Shot => Evidence::Diff,
                    };
                    Some(Vec::new())
                }
                _ => None,
            };
        }
        let index = Selection::ALL
            .iter()
            .position(|sel| *sel == self.sel)
            .unwrap_or(0);
        match key {
            "j" | "down" => Some(self.inbox_sel(Selection::ALL[(index + 1).min(4)])),
            "k" | "up" => Some(self.inbox_sel(Selection::ALL[index.saturating_sub(1)])),
            "enter" | "f" => Some(match self.sel {
                Selection::Question => self.open_question(),
                Selection::Plan => self.plan_open("T-885"),
                _ => Vec::new(),
            }),
            "1" | "2" if self.sel == Selection::Question => {
                Some(self.choose(if key == "1" { 0 } else { 1 }))
            }
            _ => None,
        }
    }
}

/// The fixture navigation badge: zero for Empty, otherwise three or two board items plus two
/// assistant prompts. Answered/queued question rows remain in the list but not the badge.
pub fn needs_you_count(state: &State, env: Env) -> u32 {
    if env.mode == DataMode::Empty {
        0
    } else if state.question_state(env) == QuestionState::Open {
        5
    } else {
        4
    }
}

pub(crate) fn phone(env: Env) -> bool {
    env.form == Form::Phone
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(conn: Conn, mode: DataMode) -> Env {
        Env {
            conn,
            mode,
            form: Form::Desktop,
        }
    }

    #[test]
    fn disconnected_answers_queue_once_without_claiming_a_live_answer() {
        for conn in [Conn::Offline, Conn::Down] {
            let mut state = State {
                choice: 1,
                draft: "alternative".into(),
                ..State::default()
            };
            assert_eq!(state.answer(env(conn, DataMode::Live)).len(), 1);
            assert_eq!(state.status, Status::Queued);
            assert!(state.answer(env(Conn::Healthy, DataMode::Live)).is_empty());
            assert_eq!(state.status, Status::Queued);
        }
        for conn in [Conn::Healthy, Conn::Degraded, Conn::Relay] {
            let mut state = State::default();
            assert_eq!(state.answer(env(conn, DataMode::Live)).len(), 1);
            assert_eq!(state.status, Status::Mine);
            assert!(state.answer(env(conn, DataMode::Live)).is_empty());
        }
        let mut state = State::default();
        assert!(
            state
                .answer(env(Conn::Healthy, DataMode::Elsewhere))
                .is_empty()
        );
        assert_eq!(state.status, Status::Open);
    }

    #[test]
    fn needs_you_badge_excludes_settled_question_but_keeps_assistant_prompts() {
        for conn in Conn::ALL {
            for mode in DataMode::ALL {
                for status in [Status::Open, Status::Queued, Status::Mine] {
                    let state = State {
                        status,
                        ..State::default()
                    };
                    let expected = if mode == DataMode::Empty {
                        0
                    } else if mode == DataMode::Elsewhere || status != Status::Open {
                        4
                    } else {
                        5
                    };
                    assert_eq!(needs_you_count(&state, env(conn, mode)), expected);
                }
            }
        }
    }
}
