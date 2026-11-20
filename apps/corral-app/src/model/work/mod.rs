mod live_changes;
mod live_context;
mod live_task;
mod served_facts;
mod task_page;
mod verb_build;
mod verb_context;
mod verb_forms;
mod verbs;
mod views;
mod when;

pub use verb_context::FormContext;
pub use verb_forms::{STATUSES, VerbComposer};
pub use verbs::{
    Choice, Record, Verb, VerbAction, VerbField, palette_record, record_verbs, session_task,
};

pub use live_changes::*;
pub use live_context::*;
pub use live_task::*;
pub use served_facts::*;
pub use task_page::*;
pub use views::*;
pub use when::*;

use std::borrow::Cow;

use crate::model::daemon;
use crate::model::effect::{Effect, Env};
use crate::model::{Id, Live};
use corral_protocol::app::{ChangesQuery, SeenAdvance};
use corral_types::{Next, Op};

/// Changes fetched per page.
pub const CHANGES_PER_PAGE: u16 = 100;

/// The operations' state: the ids requests go out under, the open operation composer and the
/// keyboard's More menu.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct State {
    pub ids: crate::model::id::IdSupply,
    /// A structured action bound to the record revision at the time it was opened.
    pub composer: Option<VerbComposer>,
    /// The selected record's keyboard-opened action menu.
    pub menu: Option<Record>,
}

/// A canonical record action against owned feed identities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Press {
    /// Open a live record (a task page, a session, a question, a plan, a review).
    Open(Live),
    /// Start an engine operation, opening its composer when it asks for input.
    Verb {
        record: Record,
        verb: Verb,
    },
    SubmitVerb(Vec<String>),
    CancelVerb,
    CloseMenu,
    /// Show another record's menu in the keyboard menu's place: a Session menu's Task entry.
    Menu(Record),
    /// Send a session control to session `session`.
    Control {
        session: Id,
        control: crate::model::agents::Control,
    },
    /// Run entry `index` of the task's Blocked card `next` list, as served now.
    Next {
        task: Id,
        index: usize,
    },
    /// Run a Blocked card's reply step `index` with the words typed into its reply box.
    Answer {
        task: Id,
        index: usize,
        text: String,
    },
    /// Run the task's one next step for its status, with the engine's defaults.
    Step {
        task: Id,
        step: Step,
    },
    /// Move this device's seen cursor through the change `seq`.
    Seen(u64),
    /// Fetch the page of changes before `seq`.
    Older(u64),
    /// Reopen the failed tasks subscription.
    RetryBoard,
    /// Open the artifact finder with this task's token filled in.
    FindArtifacts(Id),
    /// Open the Remove sheet of a build stopped here (prune's dry run first).
    Remove(Id),
}

/// A status's own next step: an operation that needs no words, sent on one press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Idea to Backlog, Backlog to Ready.
    Move(corral_types::Status),
    /// With the engine's resolved profile and base.
    Dispatch,
    /// Onto the run's base; asks once, as every approve does.
    Approve,
}

impl Step {
    /// The operation the step submits for `task`.
    pub fn op(self, task: &Id) -> Op {
        let id = task.to_string();
        match self {
            Step::Move(status) => Op::Move { id, status },
            Step::Dispatch => Op::Dispatch {
                id,
                profile: None,
                base: None,
            },
            Step::Approve => Op::Approve { id, onto: None },
        }
    }
}

/// A button's emphasis, without any layout or colour prescription.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    /// The normal filled button.
    Primary,
    /// The outlined alternative.
    Ghost,
    /// A quiet text control.
    Quiet,
}

/// A labelled control with its action, emphasis and current availability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Button {
    /// Complete visible wording, static unless it embeds a selected id.
    pub label: Cow<'static, str>,
    /// Dispatch this press when present.
    pub action: Option<Press>,
    /// Whether the user can press this control.
    pub enabled: bool,
    /// The button emphasis.
    pub kind: ButtonKind,
    /// Why an offered canonical action is currently unavailable.
    pub reason: Option<String>,
}

impl Button {
    /// Build an enabled, actionable control with its complete model-owned label.
    pub fn new(
        label: impl Into<Cow<'static, str>>,
        action: impl Into<Press>,
        kind: ButtonKind,
    ) -> Self {
        Self {
            label: label.into(),
            action: Some(action.into()),
            enabled: true,
            kind,
            reason: None,
        }
    }

    /// The same control, pressable only when `enabled`.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

impl State {
    /// Apply local interactions or dispatch canonical record effects against the feed.
    pub fn press(&mut self, press: Press, feed: &daemon::Feed, _env: Env) -> Vec<Effect> {
        match press {
            Press::Open(live) => vec![Effect::OpenLive(live)],
            Press::Verb { record, verb } => self.start_verb(record, verb, feed),
            Press::SubmitVerb(values) => self.submit_verb(values),
            Press::CancelVerb => {
                self.composer = None;
                Vec::new()
            }
            Press::CloseMenu => {
                self.menu = None;
                Vec::new()
            }
            Press::Menu(record) => {
                self.menu = Some(record);
                Vec::new()
            }
            // A chosen menu entry closes its menu, as a verb's does.
            Press::Control { session, control } => {
                self.menu = None;
                vec![Effect::Control {
                    session,
                    key: self.ids.next_id(),
                    control,
                    confirmed: false,
                }]
            }
            Press::Next { task, index } => match served_next(feed, &task, index) {
                Some(next) => self.next(&next, false),
                None => vec![no_longer_blocked(&task)],
            },
            Press::Answer { task, index, text } => match served_next(feed, &task, index) {
                Some(Next {
                    op: Op::Reply { id, .. },
                    force,
                }) => self.next(
                    &Next {
                        op: Op::Reply { id, text },
                        force,
                    },
                    false,
                ),
                Some(next) => self.next(&next, false),
                None => vec![no_longer_blocked(&task)],
            },
            Press::Step { task, step } => self.next(&Next::run(step.op(&task)), false),
            Press::RetryBoard => vec![Effect::RetryTopic("tasks".into())],
            Press::Seen(through) => vec![Effect::Seen(SeenAdvance { through })],
            Press::Older(before) => vec![Effect::Changes(ChangesQuery {
                before: Some(before),
                after: None,
                limit: CHANGES_PER_PAGE,
            })],
            Press::FindArtifacts(task) => vec![Effect::FindArtifacts(task)],
            Press::Remove(session) => {
                self.menu = None;
                vec![Effect::RemoveBuild(session)]
            }
        }
    }
}

/// Entry `index` of the task's Blocked card `next` list, as served now.
fn served_next(feed: &daemon::Feed, task: &Id, index: usize) -> Option<Next> {
    feed.board
        .task(task)
        .and_then(|task| task.blocked())
        .and_then(|blocked| blocked.next.get(index))
        .cloned()
}

fn no_longer_blocked(task: &Id) -> Effect {
    Effect::Toast(format!("{task} is no longer blocked; nothing was sent."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::effect::Conn;
    use crate::model::nav::Form;
    use corral_types::{Blocked, Status};

    const ENV: Env = Env {
        conn: Conn::Healthy,
        form: Form::Desktop,
    };

    #[test]
    fn a_menu_operation_without_input_is_sent_at_once_and_one_with_input_opens_its_composer() {
        let feed = daemon::Feed::default();
        let mut state = State::default();
        let task = Record::Task(Id::from("T-9"));
        let effects = state.press(
            Press::Verb {
                record: task.clone(),
                verb: Verb::Gate,
            },
            &feed,
            ENV,
        );
        assert!(matches!(
            effects.as_slice(),
            [Effect::Act(act)] if act.op == Op::Gate { id: "T-9".into() } && !act.force
        ));
        assert!(
            state
                .press(
                    Press::Verb {
                        record: task,
                        verb: Verb::Reply,
                    },
                    &feed,
                    ENV,
                )
                .is_empty()
        );
        let effects = state.press(Press::SubmitVerb(vec!["go on".into()]), &feed, ENV);
        assert!(matches!(
            effects.as_slice(),
            [Effect::Act(act)] if act.op == Op::Reply { id: "T-9".into(), text: "go on".into() }
        ));
    }

    #[test]
    fn a_blocked_cards_reply_box_sends_its_words_as_the_served_reply_step() {
        let mut task = daemon::bare_task(
            "T-8",
            "Paused sessions",
            Status::Blocked,
            Default::default(),
        );
        task.blocked = Some(Blocked {
            reason: "worker needs a reply".into(),
            detail: None,
            next: vec![
                Next::force(Op::Reply {
                    id: "T-8".into(),
                    text: String::new(),
                }),
                Next::run(Op::Stop { id: "T-8".into() }),
            ],
        });
        let mut feed = daemon::Feed::default();
        feed.board_received(
            1,
            vec![daemon::Task::new(daemon::task_row(task))],
            &[],
            true,
        );
        let mut state = State::default();
        let effects = state.press(
            Press::Answer {
                task: Id::new("T-8"),
                index: 0,
                text: "Stay paused".into(),
            },
            &feed,
            ENV,
        );
        assert!(matches!(
            effects.as_slice(),
            [Effect::Act(act)]
                if act.op == Op::Reply { id: "T-8".into(), text: "Stay paused".into() } && act.force
        ));
    }

    #[test]
    fn a_next_step_without_its_words_asks_for_them_and_keeps_force() {
        let mut state = State::default();
        let reply = Next::run(Op::Reply {
            id: "T-3".into(),
            text: String::new(),
        });
        assert!(state.next(&reply, false).is_empty());
        assert_eq!(state.composer.as_ref().map(|c| c.verb), Some(Verb::Reply));
        state.composer = None;
        let forced = Next::force(Op::Dispatch {
            id: "T-3".into(),
            profile: None,
            base: None,
        });
        assert!(matches!(
            state.next(&forced, true).as_slice(),
            [Effect::Act(act)] if act.force && act.confirmed
        ));
    }
}
