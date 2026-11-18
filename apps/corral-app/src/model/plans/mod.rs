//! Pure plan state, actions and feed-backed presentation for every form. A plan is a parent
//! task with children: no plan state, revision or acceptance of its own. Its document is the
//! parent's body; its members are the children, laid out by their `depends_on` edges; its
//! Path to done is the children not yet Done, then the parent. Every engine operation on a plan
//! or a member goes through the shared Actions menu; a Blocked task's served next steps run here
//! as engine operations.

mod brief;
mod brief_view;
mod graph;
mod live;
mod live_story;
mod member;
mod outline;
#[cfg(test)]
mod outline_tests;
mod path;
mod story;
mod view;

pub use brief::plan_brief;
pub use brief_view::{
    BriefAction, MemberShare, OutlineRow, OutlineSecond, OutlineSection, PathItem, PathStep,
    PathToDone, PathWarning, PlanBrief, PlanOutline, StateIndicator,
};
pub use graph::{Edge, Graph, GraphInput, GraphMember, Layer, Node, Point, layout};
pub use live::{current_plan_filtered, graph_input, live_detail, live_phone_page, plan_detail};
pub use outline::plan_outline;
pub use path::path_to_done;
pub use story::{Story, StoryField, StoryStage};
pub use view::{
    Button, ButtonKind, Chip, Detail, DocumentFold, Header, ListNotice, NoticeView, PhonePage,
    PlanView,
};

use std::collections::BTreeSet;

use corral_protocol::detail::{DetailGet, DetailRef};

use crate::model::act::Act;
use crate::model::daemon::Feed;
use crate::model::effect::{Effect, Env};
use crate::model::nav::Form;
use crate::model::work::Record;
use crate::model::{Id, Live, Selection};

/// Captions shared by all form-specific loading notices.
pub const LIST_LOADING: &str = "Loading plans…";
pub const DETAIL_LOADING: &str = "Loading the plan…";

/// Plan-local interaction state; the selected plan belongs to the root selection.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct State {
    pub ids: crate::model::id::IdSupply,
    /// Selected graph member, with its plan, so a selection changed elsewhere never shows
    /// another plan's member.
    pub member: Option<(Id, Id)>,
    /// Plans whose long document the phone shows whole rather than its opening.
    pub documents: BTreeSet<Id>,
}

impl State {
    /// The member selected in `plan`, if any.
    pub fn member_of(&self, plan: &str) -> Option<&Id> {
        self.member
            .as_ref()
            .filter(|(owner, _)| owner == plan)
            .map(|(_, member)| member)
    }
}

/// Every press target exposed by Plans, including the selected-member story.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Live(LiveAction),
}

/// Presses on corrald's plans, by owned ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveAction {
    /// Select a plan in place, or push its page on the phone.
    Plan(Id),
    /// Select a graph member; the phone pushes its task.
    Member { plan: Id, member: Id },
    /// Open a member's task.
    Task(Id),
    /// Open an agent session.
    Session(Id),
    /// Open the candidate under review for a task.
    Review(Id),
    /// Ask about a record with its bounded context; the owner still sends the draft.
    Ask { record: Live },
    /// Run entry `index` of the task's Blocked `next` list, as the board serves it now.
    Next { task: Id, index: usize },
    /// Request the plan's detail again after an error.
    Retry(Id),
    /// Show a long document whole on the phone, or its opening again.
    Document(Id),
}

/// Member cycling keys; list movement is bound only while the list has focus.
pub const KEYS: &[&str] = &["J", "K"];

impl State {
    /// Apply one press on corrald's plans.
    pub fn reduce_live(&mut self, action: LiveAction, feed: &Feed, env: Env) -> Vec<Effect> {
        match action {
            LiveAction::Plan(id) => vec![Effect::OpenLive(Live::Plan(id))],
            LiveAction::Member { plan, member } => {
                self.member = Some((plan, member.clone()));
                if env.form == Form::Phone {
                    vec![Effect::OpenLive(Live::Task(member))]
                } else {
                    vec![]
                }
            }
            LiveAction::Task(id) => vec![Effect::OpenLive(Live::Task(id))],
            LiveAction::Session(id) => vec![Effect::OpenLive(Live::Session(id))],
            LiveAction::Review(task) => vec![Effect::OpenLive(Live::Review(task))],
            LiveAction::Ask { record } => vec![Effect::AskAbout {
                context: Some(record.context(None)),
                text: None,
            }],
            LiveAction::Next { task, index } => {
                let next = feed
                    .board
                    .task(&task)
                    .and_then(|row| row.blocked())
                    .and_then(|blocked| blocked.next.get(index));
                match next {
                    Some(next) => vec![Effect::Act(Act::new(
                        &mut self.ids,
                        next.op.clone(),
                        next.force,
                    ))],
                    None => vec![Effect::Toast(format!(
                        "{task} no longer serves that next step; nothing was sent."
                    ))],
                }
            }
            LiveAction::Retry(id) => vec![Effect::Detail(DetailGet {
                target: DetailRef::Plan { id: id.to_string() },
                etag: None,
                page: None,
            })],
            LiveAction::Document(id) => {
                if !self.documents.remove(&id) {
                    self.documents.insert(id);
                }
                vec![]
            }
        }
    }

    /// Move through feed-backed plans or cycle the selected plan's members.
    /// `filter` is the Plans screen's own scope, so the keys walk the rows on screen.
    pub fn key_live_filtered(
        &mut self,
        key: &str,
        _top: Option<&Live>,
        selection: &Selection,
        feed: &Feed,
        _env: Env,
        filter: &crate::model::projects::ProjectFilter,
    ) -> Option<Vec<Effect>> {
        // The order the list draws, so j and k step to the row below or above on screen.
        let order = outline::display_order(feed, filter);
        let current = live::current_plan_filtered(selection, feed, filter);
        let index = current
            .as_ref()
            .and_then(|id| order.iter().position(|row| row == id));
        match key {
            "j" | "down" | "k" | "up" => {
                if order.is_empty() {
                    return Some(vec![]);
                }
                let next = if matches!(key, "j" | "down") {
                    index.map_or(0, |i| (i + 1).min(order.len() - 1))
                } else {
                    index.unwrap_or(0).saturating_sub(1)
                };
                Some(vec![Effect::Select(Live::Plan(order[next].clone()))])
            }
            "J" | "K" => {
                let plan = current?;
                let members: Vec<Id> = plan_detail(feed, &plan)
                    .map(|detail| {
                        detail
                            .children
                            .iter()
                            .map(|child| Id::new(&child.id))
                            .collect()
                    })
                    .unwrap_or_default();
                if !members.is_empty() {
                    let index = self
                        .member_of(&plan)
                        .and_then(|id| members.iter().position(|member| member == id));
                    let next = members[cycle(key, index, members.len())].clone();
                    self.member = Some((plan, next));
                }
                Some(vec![])
            }
            // The Actions menu of the selected member, else of the plan itself.
            "." => {
                let plan = current?;
                let record = self.member_of(&plan).cloned().unwrap_or(plan);
                Some(vec![Effect::VerbMenu(Record::Task(record))])
            }
            _ => None,
        }
    }
}

/// The member index after J (forward) or K (back), wrapping; with none selected J starts at the
/// first member and K at the second to last, as the mockup does.
fn cycle(key: &str, index: Option<usize>, len: usize) -> usize {
    match (key, index) {
        ("J", Some(i)) => (i + 1) % len,
        ("J", None) => 0,
        (_, Some(i)) => (i + len - 1) % len,
        (_, None) => (len + len - 2) % len,
    }
}
