//! Plans content for all forms. `list` supplies grouped rows; `detail` supplies the selected
//! review, tracker, approved empty state or synthetic scale tracker. Phone renderers use
//! `phone_page` for navigation titles and decide actions. `layout` resolves graph geometry,
//! including compact vertical-layer rows. Screens dispatch each descriptor's `Action` to
//! `State::reduce`; the app executes the returned effects. This module imports no UI runtime.

mod data;
mod graph;
mod story;
mod view;

pub use data::{
    DOCUMENT, Document, PLANS, Plan, PlanState, Proposal, Review, SCALE, SCALE_LAYERS, ScaleMember,
    ScaleState, plan,
};
pub use graph::{CardGeometry, Edge, Graph, Layer, Node, Point, layout};
pub use story::{Story, StoryField, StoryStage, story};
pub use view::{
    ApprovedView, AwaitingView, Blocker, Button, ButtonKind, Chip, Detail, DocumentSection,
    ExecutingView, Header, LegendEntry, ListSection, MemberSquare, PhonePage, PipelineStage,
    PlanList, PlanRow, ProposedRow, ReviewRow, ScaleLayerView, ScaleLine, ScaleView, SectionBody,
    SquareState, detail, landed, list, phone_page, scale,
};

use crate::model::board::{Actor, task};
use crate::model::effect::{Effect, Env, Page, Risk};
use crate::model::nav::{Destination, Form};

/// The mockup's Plans selection and synthetic-tracker toggle.
#[derive(Debug, Clone, PartialEq)]
pub struct State {
    /// Selected real plan, retained while the scale tracker is open.
    pub plan: &'static str,
    /// Whether the synthetic S-900 tracker replaces real-plan detail.
    pub scale: bool,
    /// Selected member; each tracker falls back to its first member if absent here.
    pub member: &'static str,
}

impl Default for State {
    fn default() -> Self {
        Self {
            plan: "T-870",
            scale: false,
            member: "T-868",
        }
    }
}

/// Every press target exposed by Plans, including the selected-member story.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Select a real plan or S-900 in the desktop list.
    SelectPlan(&'static str),
    /// Select a plan and push its phone detail page.
    PhonePlan(&'static str),
    /// Select a graph member; phone also pushes its task.
    SelectMember(&'static str),
    /// Compact graph rows open a task directly on every form.
    OpenTask(&'static str),
    /// A needs-you member opens its question rather than its task.
    Question,
    /// Request revision-checked acceptance with a fresh confirmation.
    Accept,
    /// Open the mockup's request-changes comment box toast.
    RequestChanges,
    /// Show the revision-diff toast.
    Diff,
    /// Show the document-opening toast.
    OpenDocument,
    /// Open Ask with the plan or member context chip.
    Ask(&'static str),
    /// Open the selected member's session.
    OpenSession(&'static str),
    /// Navigate to Review from the member story.
    ReviewCandidate,
}

/// NSH spellings of the Plans screen keys; uppercase J/K cycle members.
pub const KEYS: &[&str] = &["j", "k", "down", "up", "x", "J", "K"];

impl State {
    /// Select S-900 without changing the real-plan selection, or leave scale for a real plan.
    pub fn open(&mut self, id: &'static str) {
        self.scale = id == "S-900";
        if !self.scale {
            self.plan = id;
        }
    }

    /// Select a member, opening needs-you questions on every form and tasks on phone.
    pub fn select_member(&mut self, id: &'static str, env: Env) -> Vec<Effect> {
        self.member = id;
        if task(id).actor == Actor::You {
            vec![Effect::Open(Page::Question)]
        } else if env.form == Form::Phone {
            vec![Effect::Open(Page::Task(id))]
        } else {
            vec![]
        }
    }

    /// Apply one Plans press target; offline acceptance never opens a confirmation.
    pub fn reduce(&mut self, action: Action, env: Env) -> Vec<Effect> {
        match action {
            Action::SelectPlan(id) => {
                self.open(id);
                vec![]
            }
            Action::PhonePlan(id) => {
                self.open(id);
                vec![Effect::Open(Page::Plan(id))]
            }
            Action::SelectMember(id) => self.select_member(id, env),
            Action::OpenTask(id) => vec![Effect::Open(if task(id).actor == Actor::You {
                Page::Question
            } else {
                Page::Task(id)
            })],
            Action::Question => vec![Effect::Open(Page::Question)],
            Action::Accept if env.conn.refuses() => vec![],
            Action::Accept => vec![Effect::Confirm(Risk::AcceptPlan, None)],
            Action::RequestChanges => vec![Effect::Toast(
                if env.form == Form::Phone {
                    "Request changes"
                } else {
                    "Request changes: comment box opened"
                }
                .into(),
            )],
            Action::Diff => vec![Effect::Toast("Diff P1 → P2 opened".into())],
            Action::OpenDocument => vec![Effect::Toast("Opened plan document".into())],
            Action::Ask(id) => vec![Effect::Ask {
                chip: Some(id),
                text: None,
            }],
            Action::OpenSession(id) => vec![Effect::Open(Page::Session(id))],
            Action::ReviewCandidate => vec![Effect::Go(Destination::Review)],
        }
    }

    /// Port of `SCREENS.plans.keys`. Member cycling changes selection only, never navigation.
    pub fn key(&mut self, key: &str, _page: Option<Page>, _env: Env) -> Option<Vec<Effect>> {
        let index = PLANS.iter().position(|p| p.id == self.plan);
        match key {
            "j" | "down" => {
                let next = index.map_or(0, |i| (i + 1).min(PLANS.len() - 1));
                self.open(PLANS[next].id);
            }
            "k" | "up" => {
                let next = index.unwrap_or(0).saturating_sub(1);
                self.open(PLANS[next].id);
            }
            "x" => self.scale = !self.scale,
            "J" | "K" => {
                let members = plan(self.plan).members;
                if !members.is_empty() {
                    let index = members.iter().position(|id| *id == self.member);
                    let next = match (key, index) {
                        ("J", Some(i)) => (i + 1) % members.len(),
                        ("J", None) => 0,
                        (_, Some(i)) => (i + members.len() - 1) % members.len(),
                        (_, None) => (members.len() + members.len() - 2) % members.len(),
                    };
                    self.member = members[next];
                }
            }
            _ => return None,
        }
        Some(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::State;

    #[test]
    fn opening_real_plan_leaves_scale_tracker() {
        let mut state = State::default();
        state.open("S-900");
        assert!(state.scale);
        state.open("T-870");
        assert!(!state.scale);
        assert_eq!(state.plan, "T-870");
    }
}
