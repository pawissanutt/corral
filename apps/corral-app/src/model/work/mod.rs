mod closed;
mod data;
mod inspector;
mod list;
mod live_changes;
mod live_context;
mod live_task;
mod now_asks;
mod now_digest;
mod now_keys;
mod now_page;
#[cfg(test)]
mod now_page_tests;
mod served;
mod served_facts;
mod standing;
mod task_page;
mod verb_build;
mod verb_forms;
mod verbs;
mod views;
mod when;

pub use verb_forms::VerbComposer;
pub use verbs::{Record, Verb, VerbAction, VerbField, palette_record, record_verbs};
#[cfg(test)]
mod verb_tests;

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
pub use now_asks::{NowAskKind, NowAskPanel, NowAskRow};
pub use now_digest::{NowDigest, NowDigestRow};
pub use now_keys::{NowWalk, now_walk_order};
pub use now_page::{
    NowPage, NowPanel, NowRow, NowScopeAction, NowSection, NowTaskRow, NowWaiting, NowWaitingGroup,
    now_page,
};
pub use served::*;
pub use served_facts::*;
pub use standing::*;
pub use task_page::*;
pub use views::*;
pub use when::*;

use std::borrow::Cow;

use crate::model::daemon;
use crate::model::effect::{Effect, Env};
use crate::model::nav::Destination;
use crate::model::{Id, Live};
use corral_protocol::app::{ChangesQuery, SeenAdvance};

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

/// Which events the Changes feed shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChangeFilter {
    #[default]
    All,
    /// Questions and attention: what waits on the owner.
    NeedsYou,
    /// Gate results, review verdicts and landings.
    Verdicts,
    /// Worker runs and the queue.
    Runs,
}

impl ChangeFilter {
    /// Every filter in display order; Runs is desktop and terminal only, as in the mockup.
    pub const ALL: [Self; 4] = [Self::All, Self::NeedsYou, Self::Verdicts, Self::Runs];

    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::NeedsYou => "Needs you",
            Self::Verdicts => "Verdicts",
            Self::Runs => "Runs",
        }
    }

    /// Whether a daemon change of `class` passes (the change vocabulary's class names).
    pub fn admits_class(self, class: &str) -> bool {
        match self {
            Self::All => true,
            Self::NeedsYou => matches!(class, "attention" | "question"),
            Self::Verdicts => matches!(class, "gate" | "review" | "land" | "annotation"),
            Self::Runs => matches!(class, "work" | "queue" | "automation"),
        }
    }
}

/// The task command this device last sent from Now for one task, kept to show its status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sent {
    pub task: Id,
    /// The command's `cmd_id`.
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
    /// The daemon list shows every parked task, not only the first few.
    pub all_parked: bool,
    /// The daemon list shows every held task, not only the first few.
    pub all_held: bool,
    /// Now's Waiting line is open: its queued, held and parked rows show and the keys walk them.
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
    /// An owner move selected in the redesigned Now list, not in Inbox's own desk.
    pub selected_ask: Option<Id>,
    /// Escape dismisses the shared task selection locally without changing other pages.
    pub digest_only: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            ids: Default::default(),
            now_tab: NowTab::Story,
            all_queued: false,
            all_parked: false,
            all_held: false,
            waiting_open: false,
            filter: ChangeFilter::All,
            sent: Vec::new(),
            composer: None,
            menu: None,
            closed: ClosedState::default(),
            selected_ask: None,
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
    /// Show every row of a collapsed daemon group (Queued, Held or Parked).
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
    /// Select an owner move in the redesigned Now side panel.
    SelectAsk(Id),
    /// Return the redesigned Now side panel to its digest.
    ClearSelection,
    /// Open a live record (a task page, a session, a question, a plan, a review).
    Open(Live),
    /// Open Ask about a record, at the revision the app shows.
    Ask {
        about: Live,
        rev: Option<u64>,
    },
    /// Start a canonical record action, opening its structured composer when needed.
    Verb {
        record: Record,
        verb: Verb,
    },
    SubmitVerb(Vec<String>),
    CancelVerb,
    CloseMenu,
    /// Ask corrald for the result of a command whose outcome is unknown, under its own id.
    Recover(Id),
    /// Move this device's seen cursor through the change `seq`.
    Seen(u64),
    /// Fetch the page of changes before `seq`.
    Older(u64),
    /// Reopen the failed board subscription.
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
            Action::ShowAll(GroupKey::Parked) => self.all_parked = true,
            Action::ShowAll(GroupKey::Held) => self.all_held = true,
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
                self.selected_ask = None;
                self.digest_only = false;
                vec![Effect::Select(Live::Task(id))]
            }
            Press::SelectAsk(key) => {
                self.selected_ask = Some(key);
                self.digest_only = false;
                Vec::new()
            }
            Press::ClearSelection => {
                self.selected_ask = None;
                self.digest_only = true;
                Vec::new()
            }
            Press::Open(live) => {
                if matches!(&live, Live::Task(_)) {
                    self.selected_ask = None;
                    self.digest_only = false;
                }
                vec![Effect::OpenLive(live)]
            }
            Press::Ask { about, rev } => vec![Effect::AskAbout {
                context: Some(about.context(rev)),
                text: None,
                target: None,
            }],
            Press::Verb { record, verb } => self.start_verb(record, verb, feed),
            Press::SubmitVerb(values) => self.submit_verb(values, feed),
            Press::CancelVerb => {
                self.composer = None;
                Vec::new()
            }
            Press::CloseMenu => {
                self.menu = None;
                Vec::new()
            }
            Press::Recover(key) => vec![Effect::Recover(key)],
            Press::RetryBoard => vec![Effect::RetryTopic("board".into())],
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::effect::Conn;
    use crate::model::nav::Form;

    #[test]
    fn a_task_command_needs_the_loaded_owner_revision_and_is_never_reissued_while_uncertain() {
        let env = Env {
            conn: Conn::Healthy,
            form: Form::Desktop,
        };
        let mut feed = daemon::Feed::default();
        let mut state = State::default();
        let effects = state.press(
            Press::Verb {
                record: Record::Task(Id::from("T-9")),
                verb: Verb::Release,
            },
            &feed,
            env,
        );
        assert!(matches!(effects.as_slice(), [Effect::Toast(_)]));
        assert!(state.sent.is_empty());
        state.sent.push(Sent {
            task: Id::from("T-9"),
            key: Id::from("cmd-1"),
            title: "Release T-9".into(),
        });
        feed.commands.insert(
            Id::from("cmd-1"),
            daemon::CommandState {
                device: None,
                method: "act.submit".into(),
                request: serde_json::Value::Null,
                status: daemon::CommandStatus::Unknown,
                result: None,
                error: None,
                entry: None,
            },
        );
        let effects = state.press(
            Press::Verb {
                record: Record::Task(Id::from("T-9")),
                verb: Verb::Release,
            },
            &feed,
            env,
        );
        assert!(
            matches!(effects.as_slice(), [Effect::Toast(text)] if text.contains("no known result"))
        );
        assert_eq!(state.sent.len(), 1);
    }
}
