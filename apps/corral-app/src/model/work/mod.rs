mod closed;
mod data;
mod inspector;
mod list;
mod live_changes;
mod live_context;
mod live_task;
mod now_digest;
mod now_keys;
mod now_page;
#[cfg(test)]
mod now_page_tests;
mod served;
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

/// Shared loading caption for an inspector awaiting its task detail.
pub const INSPECTOR_LOADING: &str = "Loading task…";
pub const VERB_MENU_LABEL: &str = "Actions";

pub use closed::*;
pub use data::*;
pub use inspector::*;
pub use list::*;
pub use live_changes::*;
pub use live_context::*;
pub use live_task::*;
pub use now_digest::{NowDigest, NowDigestRow};
pub use now_keys::now_walk_order;
pub use now_page::{
    NowPage, NowPanel, NowScopeAction, NowSection, NowTaskRow, NowWaiting, NowWaitingGroup,
    now_page,
};
pub use served::*;
pub use served_facts::*;
pub use task_page::*;
pub use views::*;
pub use when::*;

use std::borrow::Cow;

use crate::model::daemon;
use crate::model::effect::{Effect, Env};
use crate::model::nav::Destination;
use crate::model::{Id, Live};
use corral_protocol::app::{ChangesQuery, SeenAdvance};
use corral_types::{Next, Op};

/// The selected inspector tab, in the mockup's cycling order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NowTab {
    /// Task progress, dependencies and next actor.
    #[default]
    Story,
    /// Artifact list.
    Evidence,
    /// Link to the structured session and terminal tail.
    Session,
}

impl NowTab {
    /// The inspector's tab strip.
    pub const ALL: [Self; 3] = [Self::Story, Self::Evidence, Self::Session];

    /// The tab's visible label.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Story => "Story",
            Self::Evidence => "Evidence",
            Self::Session => "Session",
        }
    }
}

/// Which kind of change the Changes feed shows. Every event kind belongs to exactly one filter
/// besides All, so the filters split the feed rather than overlap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChangeFilter {
    #[default]
    All,
    /// Blocked tasks, landings that wait, and errors: what waits on the owner.
    NeedsYou,
    /// Gate results, reviews, verdicts and landings.
    Verdicts,
    /// Worker runs: dispatched, resumed, replied to, done, stopped.
    Runs,
    /// The tasks themselves: created, edited, commented, moved, dropped, reopened, artifacts.
    Tasks,
    /// Operations forced past a rule, each naming what it skipped.
    Forced,
}

impl ChangeFilter {
    /// Every filter in display order, the same on every form.
    pub const ALL: [Self; 6] = [
        Self::All,
        Self::NeedsYou,
        Self::Verdicts,
        Self::Runs,
        Self::Tasks,
        Self::Forced,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::NeedsYou => "Needs you",
            Self::Verdicts => "Verdicts",
            Self::Runs => "Runs",
            Self::Tasks => "Tasks",
            Self::Forced => "Forced",
        }
    }

    /// Whether an engine event of `kind` passes.
    pub fn admits(self, kind: corral_types::EventKind) -> bool {
        use corral_types::EventKind as Kind;
        match self {
            Self::All => true,
            Self::NeedsYou => matches!(kind, Kind::Blocked | Kind::Ready | Kind::Error),
            Self::Verdicts => matches!(
                kind,
                Kind::Gate | Kind::ReviewStarted | Kind::Verdict | Kind::SentBack | Kind::Landed
            ),
            Self::Runs => matches!(
                kind,
                Kind::Dispatched | Kind::Resumed | Kind::Replied | Kind::Done | Kind::Stopped
            ),
            Self::Tasks => matches!(
                kind,
                Kind::Created
                    | Kind::Edited
                    | Kind::Commented
                    | Kind::Moved
                    | Kind::Dropped
                    | Kind::Reopened
                    | Kind::Artifact
            ),
            Self::Forced => kind == Kind::Forced,
        }
    }
}

/// The task command this device last sent from Now for one task, kept to show its status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sent {
    pub task: Id,
    /// The operation's request id.
    pub key: Id,
    pub title: String,
}

/// How many tasks' last commands Now keeps.
const SENT_KEPT: usize = 16;

/// Changes fetched per page.
pub const CHANGES_PER_PAGE: u16 = 100;

/// Selection, the inspector tab, the daemon list's expanded groups and filters. The Daemon
/// state's selected task is the root's live selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    pub ids: crate::model::id::IdSupply,
    /// Selected inspector tab.
    pub now_tab: NowTab,
    /// The daemon list shows every queued task, not only the first few.
    pub all_queued: bool,
    /// The daemon list shows every backlog task, not only the first few.
    pub all_backlog: bool,
    /// The daemon list shows every idea, not only the first few.
    pub all_ideas: bool,
    /// Now's Waiting line is open: its queued, backlog and idea rows show and the keys walk them.
    pub waiting_open: bool,
    /// The Changes feed's filter.
    pub filter: ChangeFilter,
    /// Task commands sent from Now, newest last.
    pub sent: Vec<Sent>,
    /// A structured action bound to the record revision at the time it was opened.
    pub composer: Option<VerbComposer>,
    /// The selected record's keyboard-opened action menu.
    pub menu: Option<Record>,
    /// Now's Closed section, read from corrald on request.
    pub closed: ClosedState,
    /// Escape dismisses the shared task selection locally without changing other pages.
    pub digest_only: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            ids: Default::default(),
            now_tab: NowTab::Story,
            all_queued: false,
            all_backlog: false,
            all_ideas: false,
            waiting_open: false,
            filter: ChangeFilter::All,
            sent: Vec::new(),
            composer: None,
            menu: None,
            closed: ClosedState::default(),
            digest_only: false,
        }
    }
}

/// Local controls shared by Now, Changes and More.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Switch the inspector tab.
    NowTab(NowTab),
    /// Open or fold Now's Waiting rows.
    ToggleWaiting,
    /// Open a destination, including entries on the More page.
    Go(Destination),
    /// Return from a pushed task or Changes page.
    Back,
    /// Show every row of a collapsed daemon group (Queued, Backlog or Ideas).
    ShowAll(GroupKey),
    /// Narrow the Changes feed.
    Filter(ChangeFilter),
}

/// Local interaction or a canonical record action against owned feed identities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Press {
    Work(Action),
    /// Select a task in place.
    Select(Id),
    /// Return the redesigned Now side panel to its digest.
    ClearSelection,
    /// Open a live record (a task page, a session, a question, a plan, a review).
    Open(Live),
    /// Open Ask about a record, at the revision the app shows.
    Ask {
        about: Live,
        rev: Option<u64>,
    },
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
    /// Bring run session `<task>_<run>`'s herdr pane forward on corrald's machine.
    Focus(Id),
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
    /// Move this device's seen cursor through the change `seq`.
    Seen(u64),
    /// Fetch the page of changes before `seq`.
    Older(u64),
    /// Reopen the failed tasks subscription.
    RetryBoard,
    /// The Closed section's toggle and pager.
    Closed(ClosedPress),
}

impl From<Action> for Press {
    fn from(action: Action) -> Press {
        Press::Work(action)
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

/// Worker-hue feed glyph for a worker-start event.
pub const START_MARK: &str = "▶";

/// Now's letter command, bound wherever the focus is outside a text field: `]` brings the
/// inspector's next tab forward (Tab stays the platform's focus key). The list keys (moves,
/// Enter) are `commands::list_keys`, bound only while the list has the focus.
pub const KEYS: &[&str] = &["]"];

impl State {
    /// Apply form-independent local controls.
    pub fn act(&mut self, action: Action) -> Vec<Effect> {
        match action {
            Action::NowTab(tab) => self.now_tab = tab,
            Action::ToggleWaiting => self.waiting_open = !self.waiting_open,
            Action::Go(to) => return vec![Effect::Go(to)],
            Action::Back => return vec![Effect::Back],
            Action::ShowAll(GroupKey::Backlog) => self.all_backlog = true,
            Action::ShowAll(GroupKey::Ideas) => self.all_ideas = true,
            Action::ShowAll(_) => self.all_queued = true,
            Action::Filter(filter) => self.filter = filter,
        }
        Vec::new()
    }

    /// Apply local interactions or dispatch canonical record effects against the feed.
    pub fn press(&mut self, press: Press, feed: &daemon::Feed, _env: Env) -> Vec<Effect> {
        match press {
            Press::Work(action) => self.act(action),
            Press::Select(id) => {
                self.digest_only = false;
                vec![Effect::Select(Live::Task(id))]
            }
            Press::ClearSelection => {
                self.digest_only = true;
                Vec::new()
            }
            Press::Open(live) => {
                if matches!(&live, Live::Task(_)) {
                    self.digest_only = false;
                }
                vec![Effect::OpenLive(live)]
            }
            Press::Ask { about, rev } => vec![Effect::AskAbout {
                context: Some(about.context(rev)),
                text: None,
            }],
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
            Press::Focus(session) => {
                self.menu = None;
                vec![crate::model::agents::focus(&mut self.ids, feed, session)]
            }
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
            Press::RetryBoard => vec![Effect::RetryTopic("tasks".into())],
            Press::Seen(through) => vec![Effect::Seen(SeenAdvance { through })],
            Press::Older(before) => vec![Effect::Changes(ChangesQuery {
                before: Some(before),
                after: None,
                limit: CHANGES_PER_PAGE,
            })],
            Press::Closed(press) => self.closed.press(press),
        }
    }

    /// The last command sent from Now for `task`, if any.
    pub fn sent_for(&self, task: &str) -> Option<&Sent> {
        self.sent.iter().rev().find(|sent| sent.task == *task)
    }

    /// Moves the inspector to its next tab, wrapping from Session back to Story.
    pub(super) fn cycle_tab(&mut self) {
        self.now_tab = match self.now_tab {
            NowTab::Story => NowTab::Evidence,
            NowTab::Evidence => NowTab::Session,
            NowTab::Session => NowTab::Story,
        };
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
