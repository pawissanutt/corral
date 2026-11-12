//! Pure content for Now, its task page, Changes and the phone More list.
//!
//! Render [`workspace`] as the desktop/terminal dock contents, [`phone_now`] as the phone
//! tab, and [`task_page`] as its pushed task page. Rows carry their resolved facts
//! ([`TaskLine`](crate::model::board::TaskLine)) so the renderer reads no store. Pass button
//! actions to [`State::act`]. The mockup's Split, Zoom and Changes filter controls have no
//! handlers; their descriptors intentionally carry no action. Graph references contain ids
//! only, for the Plans renderer. In the Daemon data state, [`daemon_list`] and its siblings
//! build Now from the daemon's board instead of the fixtures.

mod data;
mod inspector;
mod list;
mod served;
mod served_facts;
mod views;

pub use data::*;
pub use inspector::*;
pub use list::*;
pub use served::*;
pub use served_facts::*;
pub use views::*;

use std::borrow::Cow;

use crate::model::effect::{Effect, Env, Page};
use crate::model::nav::{Destination, SettingsSection};

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

    /// The complete tab label, including its snapshot count or clock.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Since => "Since your last visit · 18:40",
            Self::Parked => "Parked 32",
        }
    }
}

/// Selection, dock tabs and the daemon list's expanded groups, initially focused on T-890's
/// story.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    /// Task id selected in the list and all inspector/context content.
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
        }
    }
}

/// Every handled control in the Now, task, Changes and More builders.
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
    /// Show the per-device seen-cursor notice without altering snapshot marks.
    MarkSeen,
    /// Open connection settings from a connection banner.
    Connection,
    /// Show the mockup's service-manager restart notice.
    RestartDaemon,
    /// Show every row of a collapsed daemon group (Queued, Held or Parked).
    ShowAll(GroupKey),
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
    /// Dispatch this action when present.
    pub action: Option<Action>,
    /// Whether the user can press this control.
    pub enabled: bool,
    /// The mockup's button emphasis.
    pub kind: ButtonKind,
}

impl Button {
    /// Build an enabled, actionable control with its complete model-owned label.
    pub fn new(label: impl Into<Cow<'static, str>>, action: Action, kind: ButtonKind) -> Self {
        Self {
            label: label.into(),
            action: Some(action),
            enabled: true,
            kind,
        }
    }
}

/// Worker-hue feed glyph for a worker-start event.
pub const START_MARK: &str = "▶";

/// NSH key names handled by the Now workspace.
pub const KEYS: &[&str] = &["j", "k", "down", "up", "enter", "tab"];

impl State {
    /// Select a task for every Now panel; navigation is the app's responsibility.
    pub fn select(&mut self, id: &'static str) {
        self.sel = id;
    }

    /// Apply a button or row action, returning all cross-domain effects.
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
        }
        Vec::new()
    }

    /// Moves the inspector to its next tab, wrapping from Session back to Story.
    pub(super) fn cycle_tab(&mut self) {
        self.now_tab = match self.now_tab {
            NowTab::Story => NowTab::Evidence,
            NowTab::Evidence => NowTab::Session,
            NowTab::Session => NowTab::Story,
        };
    }

    /// Port Now's j/k, arrows, Enter and cycling Tab handler.
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
}
