//! Pure content for Now, its task page, Changes and the phone More list.
//!
//! Render [`workspace`] as the desktop/terminal dock contents, [`phone_now`] as the phone
//! tab, and [`task_page`] as its pushed task page. Rows carry their resolved facts
//! ([`TaskLine`](crate::model::board::TaskLine)) so the renderer reads no store. Controls carry a
//! [`Press`]: a fixture [`Action`] for [`State::act`], or a live press for [`State::press`],
//! which turns it into root effects (live navigation, Ask context, typed canonical commands,
//! changes paging and the per-device seen cursor). The mockup's Split and Zoom controls have no
//! handlers; their descriptors intentionally carry no action. In the Daemon data state,
//! [`daemon_list`], [`daemon_inspector`], [`live_changes`] and their siblings build Now and
//! Changes from the daemon's feed instead of the fixtures.

mod data;
mod inspector;
mod list;
mod live_changes;
mod live_context;
mod live_task;
mod served;
mod served_facts;
mod standing;
mod views;
mod when;

pub use data::*;
pub use inspector::*;
pub use list::*;
pub use live_changes::*;
pub use live_context::*;
pub use live_task::*;
pub use served::*;
pub use served_facts::*;
pub use standing::*;
pub use views::*;
pub use when::*;

use std::borrow::Cow;

use corral_protocol::app::{ChangesQuery, SeenAdvance};
use corral_protocol::cmd::{Command, Expect, task::TaskCommand};

use crate::model::act::{self, Act};
use crate::model::daemon;
use crate::model::effect::{Effect, Env, Page};
use crate::model::nav::{Destination, SettingsSection};
use crate::model::{Id, Live};

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

/// The lower-right context tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CtxTab {
    /// The selected task's terminal tail.
    #[default]
    Live,
    /// The selected task's plan and highlighted member.
    PlanGraph,
}

impl CtxTab {
    /// The context tab strip.
    pub const ALL: [Self; 2] = [Self::Live, Self::PlanGraph];

    /// The tab's visible label.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Live => "Live",
            Self::PlanGraph => "Plan graph",
        }
    }
}

/// The lower-left dock tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LowTab {
    /// Newest changes since this device's last visit.
    #[default]
    Since,
    /// Tasks with the owner's dispatch hold.
    Parked,
}

impl LowTab {
    /// The lower dock tab strip.
    pub const ALL: [Self; 2] = [Self::Since, Self::Parked];

    /// The fixture's complete tab label, including its snapshot count or clock.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Since => "Since your last visit · 18:40",
            Self::Parked => "Parked 32",
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

/// A task command Now offers when the task's state allows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskVerb {
    /// Set the owner's dispatch hold on a queued task.
    Hold,
    /// Release the task's hold.
    Release,
    /// Run the task again after its run failed.
    Retry { run: Option<String> },
}

impl TaskVerb {
    /// The command's title, as its button and receipt name it.
    pub fn title(&self, task: &str) -> String {
        match self {
            Self::Hold => format!("Hold {task}"),
            Self::Release => format!("Release {task}"),
            Self::Retry { .. } => format!("Retry {task}"),
        }
    }

    fn command(&self, task: &str) -> TaskCommand {
        let id = task.to_owned();
        match self {
            Self::Hold => TaskCommand::Hold { id, reason: None },
            Self::Release => TaskCommand::Release { id },
            Self::Retry { run } => TaskCommand::Retry {
                id,
                run: run.clone(),
                profile: None,
            },
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

/// Selection, dock tabs, the daemon list's expanded groups and filters, initially focused on
/// T-890's story in the fixture. The Daemon state's selected task is the root's live selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    /// Fixture task id selected in the list and all inspector/context content.
    pub sel: &'static str,
    /// Selected inspector tab.
    pub now_tab: NowTab,
    /// Selected context tab.
    pub ctx_tab: CtxTab,
    /// Selected lower-left dock tab.
    pub low_tab: LowTab,
    /// The daemon list shows every queued task, not only the first few.
    pub all_queued: bool,
    /// The daemon list shows every parked task, not only the first few.
    pub all_parked: bool,
    /// The daemon list shows every held task, not only the first few.
    pub all_held: bool,
    /// The Changes feed's filter.
    pub filter: ChangeFilter,
    /// The board goal the daemon list is narrowed to.
    pub goal: Option<String>,
    /// Task commands sent from Now, newest last.
    pub sent: Vec<Sent>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            sel: "T-890",
            now_tab: NowTab::Story,
            ctx_tab: CtxTab::Live,
            low_tab: LowTab::Since,
            all_queued: false,
            all_parked: false,
            all_held: false,
            filter: ChangeFilter::All,
            goal: None,
            sent: Vec::new(),
        }
    }
}

/// Every handled fixture or form-independent control in the Now, task, Changes and More
/// builders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Select a desktop row without changing destination.
    Select(&'static str),
    /// Open a feed/phone task row, selecting it first.
    OpenTask(&'static str),
    /// Switch the inspector tab.
    NowTab(NowTab),
    /// Switch the context tab.
    CtxTab(CtxTab),
    /// Switch the lower-left tab.
    LowTab(LowTab),
    /// Open the selected task's full session.
    OpenSession(&'static str),
    /// Open a plan review or context graph.
    OpenPlan(&'static str),
    /// Open Ask with the task as its context chip.
    AskAbout(&'static str),
    /// Open the needs-you task's handover sheet.
    OpenQuestion,
    /// Open a destination, including entries on the More page.
    Go(Destination),
    /// Return from a pushed task or Changes page.
    Back,
    /// Show the mockup's goals-filter explanation.
    Goals,
    /// Explain the collapsed queued rows, with phone-specific counts.
    MoreQueued,
    /// Explain the parked summary.
    ParkedSummary,
    /// Open the illustrative PM question's sample notice.
    SampleQuestion,
    /// Show the fixture's per-device seen-cursor notice without altering snapshot marks.
    MarkSeen,
    /// Open connection settings from a connection banner.
    Connection,
    /// Show the mockup's service-manager restart notice.
    RestartDaemon,
    /// Show every row of a collapsed daemon group (Queued, Held or Parked).
    ShowAll(GroupKey),
    /// Narrow the Changes feed.
    Filter(ChangeFilter),
}

/// What pressing a control does: a fixture or form-independent [`Action`], or a live press on
/// owned daemon ids that [`State::press`] turns into root effects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Press {
    Work(Action),
    /// Select a task in place.
    Select(Id),
    /// Open a live record (a task page, a session, a question, a plan, a review).
    Open(Live),
    /// Open Ask about a record, at the revision the app shows.
    Ask {
        about: Live,
        rev: Option<u64>,
    },
    /// Send a task command, preconditioned on the task's owner revision.
    Run {
        task: Id,
        verb: TaskVerb,
    },
    /// Ask corrald for the result of a command whose outcome is unknown, under its own id.
    Recover(Id),
    /// Narrow the daemon list to a goal, or show every goal.
    Goal(Option<String>),
    /// Move this device's seen cursor through the change `seq`.
    Seen(u64),
    /// Fetch the page of changes before `seq`.
    Older(u64),
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

/// A labelled control; `None` actions reproduce unimplemented mockup controls visibly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Button {
    /// Complete visible wording, static unless it embeds a selected id.
    pub label: Cow<'static, str>,
    /// Dispatch this press when present.
    pub action: Option<Press>,
    /// Whether the user can press this control.
    pub enabled: bool,
    /// The mockup's button emphasis.
    pub kind: ButtonKind,
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

/// NSH key names handled by the Now workspace.
pub const KEYS: &[&str] = &["j", "k", "down", "up", "enter", "tab"];

impl State {
    /// Select a fixture task for every Now panel; navigation is the app's responsibility.
    pub fn select(&mut self, id: &'static str) {
        self.sel = id;
    }

    /// Apply a fixture or form-independent action, returning all cross-domain effects.
    pub fn act(&mut self, action: Action, env: Env) -> Vec<Effect> {
        use crate::model::nav::Form;
        match action {
            Action::Select(id) => self.select(id),
            Action::OpenTask(id) => {
                self.select(id);
                return vec![Effect::Open(Page::Task(id))];
            }
            Action::NowTab(tab) => self.now_tab = tab,
            Action::CtxTab(tab) => self.ctx_tab = tab,
            Action::LowTab(tab) => self.low_tab = tab,
            Action::OpenSession(id) => return vec![Effect::Open(Page::Session(id))],
            Action::OpenPlan(id) => return vec![Effect::Open(Page::Plan(id))],
            Action::AskAbout(id) => {
                return vec![Effect::Ask {
                    chip: Some(id),
                    text: None,
                }];
            }
            Action::OpenQuestion => return vec![Effect::Open(Page::Question)],
            Action::Go(to) => return vec![Effect::Go(to)],
            Action::Back => return vec![Effect::Back],
            Action::Connection => return vec![Effect::GoSettings(SettingsSection::Connection)],
            Action::Goals => return vec![Effect::Toast("Goals is a filter: g toggles it".into())],
            Action::MoreQueued => {
                return vec![Effect::Toast(
                    if env.form == Form::Phone {
                        "+19 more queued, collapsed by reason"
                    } else {
                        "Expanded: 15 more queued, collapsed by reason (capacity)"
                    }
                    .into(),
                )];
            }
            Action::ParkedSummary => {
                return vec![Effect::Toast(
                    if env.form == Form::Phone {
                        "Parked by you"
                    } else {
                        "Parked: 32 tasks with your dispatch hold"
                    }
                    .into(),
                )];
            }
            Action::SampleQuestion => {
                return vec![Effect::Toast("Sample question (illustrative)".into())];
            }
            Action::MarkSeen => {
                return vec![Effect::Toast(
                    "Seen-cursor moved to 22:12 for pawis-desktop only".into(),
                )];
            }
            Action::RestartDaemon => {
                return vec![Effect::Toast(
                    "Restart requested via service manager (systemctl --user restart corrald)"
                        .into(),
                )];
            }
            Action::ShowAll(GroupKey::Parked) => self.all_parked = true,
            Action::ShowAll(GroupKey::Held) => self.all_held = true,
            Action::ShowAll(_) => self.all_queued = true,
            Action::Filter(filter) => self.filter = filter,
        }
        Vec::new()
    }

    /// Apply any press: fixture actions through [`State::act`], live presses into root effects
    /// against the daemon's feed.
    pub fn press(&mut self, press: Press, feed: &daemon::Feed, env: Env) -> Vec<Effect> {
        match press {
            Press::Work(action) => self.act(action, env),
            Press::Select(id) => vec![Effect::Select(Live::Task(id))],
            Press::Open(live) => vec![Effect::OpenLive(live)],
            Press::Ask { about, rev } => vec![Effect::AskAbout {
                context: Some(about.context(rev)),
                text: None,
                target: None,
            }],
            Press::Run { task, verb } => self.run(task, verb, feed),
            Press::Recover(key) => vec![Effect::Recover(key)],
            Press::Goal(goal) => {
                self.goal = goal;
                Vec::new()
            }
            Press::Seen(through) => vec![Effect::Seen(SeenAdvance { through })],
            Press::Older(before) => vec![Effect::Changes(ChangesQuery {
                before: Some(before),
                after: None,
                limit: CHANGES_PER_PAGE,
            })],
        }
    }

    /// The last command sent from Now for `task`, if any.
    pub fn sent_for(&self, task: &str) -> Option<&Sent> {
        self.sent.iter().rev().find(|sent| sent.task == *task)
    }

    /// Builds the task command against the loaded detail's owner revision. Nothing is sent
    /// without that revision, nor while this task's previous command has no known outcome: a
    /// new identity after an uncertain one could run the change twice.
    fn run(&mut self, task: Id, verb: TaskVerb, feed: &daemon::Feed) -> Vec<Effect> {
        if let Some(previous) = self.sent_for(&task)
            && act::status(feed, &previous.key).is_some_and(|status| status.uncertain())
        {
            return vec![Effect::Toast(format!(
                "{} has no known result yet; nothing new was sent.",
                previous.title
            ))];
        }
        let Some(detail) = loaded_task(feed, &task) else {
            return vec![Effect::Toast(format!(
                "{task} detail has not loaded; nothing was sent."
            ))];
        };
        let act = Act::new(verb.title(&task), Command::Task(verb.command(&task)))
            .expect(Expect::TaskRevision {
                task: task.to_string(),
                revision: detail.owner_revision.clone(),
            })
            .about(Live::Task(task.clone()));
        self.sent.retain(|sent| sent.task != task);
        self.sent.push(Sent {
            task,
            key: act.key.clone(),
            title: act.title.clone(),
        });
        if self.sent.len() > SENT_KEPT {
            self.sent.remove(0);
        }
        vec![Effect::Act(act)]
    }

    /// Moves the inspector to its next tab, wrapping from Session back to Story.
    pub(super) fn cycle_tab(&mut self) {
        self.now_tab = match self.now_tab {
            NowTab::Story => NowTab::Evidence,
            NowTab::Evidence => NowTab::Session,
            NowTab::Session => NowTab::Story,
        };
    }

    /// Port Now's j/k, arrows, Enter and cycling Tab handler for the fixture preview.
    ///
    /// `page` prevents handling keys for another domain's pushed page. Collapsed summaries
    /// and extra plan/PM rows are not keyboard-selectable, as in the mockup. Empty/elsewhere
    /// projections omit their hidden question task from the selection order.
    pub fn key(&mut self, key: &str, page: Option<Page>, env: Env) -> Option<Vec<Effect>> {
        self.key_with_question(key, page, env, question_open(env))
    }

    /// The key handler with the inbox's question-open projection supplied by the app.
    /// This mirrors `grouped_list_with_question` without coupling the two domain modules.
    pub fn key_with_question(
        &mut self,
        key: &str,
        page: Option<Page>,
        env: Env,
        question_open: bool,
    ) -> Option<Vec<Effect>> {
        if page.is_some_and(|page| !matches!(page, Page::Task(_))) {
            return None;
        }
        match key {
            "j" | "down" | "ArrowDown" | "k" | "up" | "ArrowUp" => {
                let mut ids = list::selectable_tasks(env, question_open).peekable();
                let mut selected = ids.peek().copied();
                let mut previous = selected;
                while let Some(id) = ids.next() {
                    if id == self.sel {
                        selected = if matches!(key, "j" | "down" | "ArrowDown") {
                            Some(ids.peek().copied().unwrap_or(id))
                        } else {
                            previous
                        };
                        break;
                    }
                    previous = Some(id);
                }
                if let Some(id) = selected {
                    self.select(id);
                }
                Some(Vec::new())
            }
            "enter" | "Enter" => Some(vec![Effect::Open(if self.sel == "T-868" {
                Page::Question
            } else {
                Page::Session(self.sel)
            })]),
            "tab" | "Tab" => {
                self.cycle_tab();
                Some(Vec::new())
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::effect::{Conn, DataMode};
    use crate::model::nav::Form;

    #[test]
    fn selection_walks_group_boundaries_without_wrapping() {
        let env = Env {
            conn: Conn::Healthy,
            mode: DataMode::Live,
            form: Form::Desktop,
        };
        let ids = visible_task_ids(env, true);
        let mut state = State::default();
        state.select(ids[0]);
        state.key("k", None, env);
        assert_eq!(state.sel, ids[0]);
        for id in ids.iter().skip(1) {
            state.key("j", None, env);
            assert_eq!(state.sel, *id);
        }
        state.key("j", None, env);
        assert_eq!(state.sel, "T-852");
        for id in ids.iter().rev().skip(1) {
            state.key("k", None, env);
            assert_eq!(state.sel, *id);
        }
        state.key("k", None, env);
        assert_eq!(state.sel, "T-868");
    }

    #[test]
    fn hidden_question_cannot_receive_keyboard_selection() {
        let env = Env {
            conn: Conn::Healthy,
            mode: DataMode::Elsewhere,
            form: Form::Desktop,
        };
        let mut state = State::default();
        state.key("k", None, env);
        assert_eq!(state.sel, "T-890");
        assert!(!visible_task_ids(env, false).contains(&"T-868"));
    }

    #[test]
    fn a_task_command_needs_the_loaded_owner_revision_and_is_never_reissued_while_uncertain() {
        let env = Env {
            conn: Conn::Healthy,
            mode: DataMode::Live,
            form: Form::Desktop,
        };
        let mut feed = daemon::Feed::default();
        let mut state = State::default();
        let effects = state.press(
            Press::Run {
                task: Id::from("T-9"),
                verb: TaskVerb::Release,
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
            Press::Run {
                task: Id::from("T-9"),
                verb: TaskVerb::Release,
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
