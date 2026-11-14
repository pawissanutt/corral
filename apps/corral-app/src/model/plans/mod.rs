//! Plans content for all forms, from two sources. The fixture preview (`fixture.rs`) keeps the
//! mockup's six plans and the synthetic S-900 tracker; daemon mode (`live*.rs`) builds the same
//! descriptors from corrald's plan rows, plan details, revision diffs and board. `list` and
//! `live_list` supply grouped rows; `detail` and `live_detail` the selected review, tracker,
//! approved empty state, document or diff. Phone renderers use `phone_page`/`live_phone_page`.
//! `layout` resolves graph geometry, including compact vertical-layer rows. Screens dispatch
//! each descriptor's `Action` to `State::reduce` (fixture) or `State::reduce_live` (daemon); the
//! app executes the returned effects. This module imports no UI runtime.

mod data;
mod fixture;
mod graph;
mod live;
mod live_doc;
mod live_role;
mod live_story;
mod story;
mod view;

pub use data::{
    DOCUMENT, Document, PLANS, Plan, PlanState, Proposal, Review, SCALE, SCALE_LAYERS, ScaleMember,
    ScaleState, plan,
};
pub use fixture::{detail, landed, list, phone_page, scale};
pub use graph::{
    CardGeometry, Edge, Graph, GraphInput, GraphMember, Layer, Node, Point, fixture_graph, layout,
};
pub use live::{current_plan, graph_input, live_detail, live_list, live_phone_page, plan_detail};
pub use story::{Story, StoryField, StoryStage, story};
pub use view::{
    ApprovedView, AwaitingView, Blocker, Button, ButtonKind, Chip, Compose, Decide, DecideNote,
    DecisionRow, Detail, DiffLine, DiffLineKind, DiffView, DocumentSection, DocumentView,
    ExecutingView, Header, LegendEntry, ListNotice, ListSection, MemberSquare, NoticeView,
    PhonePage, PipelineStage, PlanList, PlanRow, ProposedRow, ReviewRow, ScaleLayerView, ScaleLine,
    ScaleView, SectionBody, SquareState,
};

use corral_protocol::cmd::Command;
use corral_protocol::cmd::plan::PlanCommand;
use corral_protocol::detail::{DetailGet, DetailRef};

use crate::model::act::{self, Act};
use crate::model::board::{Actor, task};
use crate::model::daemon::Feed;
use crate::model::effect::{Effect, Env, Page, Risk};
use crate::model::nav::{Destination, Form};
use crate::model::{Id, Live, Selection};

/// The fixture's Plans selection and synthetic-tracker toggle, and the daemon-mode sub-state.
/// The selected live plan itself is the root's `Selection.plan`.
#[derive(Debug, Clone, PartialEq)]
pub struct State {
    /// Selected fixture plan, retained while the scale tracker is open.
    pub plan: &'static str,
    /// Whether the synthetic S-900 tracker replaces fixture plan detail.
    pub scale: bool,
    /// Selected fixture member; each tracker falls back to its first member if absent here.
    pub member: &'static str,
    /// Member, lens, request-changes note and sent commands for corrald's plans.
    pub live: LiveState,
}

impl Default for State {
    fn default() -> Self {
        Self {
            plan: "T-870",
            scale: false,
            member: "T-868",
            live: LiveState::default(),
        }
    }
}

/// What the detail of one served plan shows instead of its overview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lens {
    /// The plan's document (executing and approved plans; the review shows it already).
    Document,
    /// The diff between two revisions.
    Diff { from: String, to: String },
}

/// A command sent about one plan revision, kept so its status stays visible and so an uncertain
/// result is never repeated under a new identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sent {
    pub plan: Id,
    /// The command identity (`cmd_id`).
    pub key: Id,
    pub revision: String,
}

/// A request-changes note, bound to the revision it was written about: a newer revision
/// arriving never moves it silently, and sending it on another revision is the owner's
/// explicit choice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    pub plan: Id,
    /// The revision shown when the note was opened, or the one it was last sent on.
    pub revision: Option<String>,
    pub text: String,
}

/// Daemon-mode state private to Plans; every entry names the plan it belongs to, so a selection
/// changed elsewhere (palette, Inbox) never shows another plan's member, lens or note.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LiveState {
    /// Selected graph member, with its plan.
    pub member: Option<(Id, Id)>,
    /// Document or diff in place of the overview, with its plan.
    pub lens: Option<(Id, Lens)>,
    /// The request-changes note being written.
    pub compose: Option<Draft>,
    /// The latest acceptance sent from this device.
    pub accept: Option<Sent>,
    /// The latest request for changes sent from this device.
    pub changes: Option<Sent>,
    /// The owner's resolutions of one plan's owner decisions, by decision text.
    pub resolutions: Option<(Id, Vec<(String, String)>)>,
}

impl LiveState {
    /// The member selected in `plan`, if any.
    pub fn member_of(&self, plan: &str) -> Option<&Id> {
        self.member
            .as_ref()
            .filter(|(owner, _)| owner == plan)
            .map(|(_, member)| member)
    }

    /// The lens open on `plan`, if any.
    pub fn lens_of(&self, plan: &str) -> Option<&Lens> {
        self.lens
            .as_ref()
            .filter(|(owner, _)| owner == plan)
            .map(|(_, lens)| lens)
    }

    /// The words of the note being written for `plan`, if it has one.
    pub fn compose_of(&self, plan: &str) -> Option<&String> {
        self.draft_of(plan).map(|draft| &draft.text)
    }

    /// The note being written for `plan`, with the revision it was written about.
    pub fn draft_of(&self, plan: &str) -> Option<&Draft> {
        self.compose.as_ref().filter(|draft| draft.plan == plan)
    }

    /// The owner's resolution of one of `plan`'s owner decisions, as typed; empty when none.
    pub fn resolution_of(&self, plan: &str, decision: &str) -> &str {
        self.resolutions
            .as_ref()
            .filter(|(owner, _)| owner == plan)
            .and_then(|(_, all)| all.iter().find(|(known, _)| known == decision))
            .map_or("", |(_, text)| text.as_str())
    }
}

/// Every press target exposed by Plans, including the selected-member story.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Select a fixture plan or S-900 in the desktop list.
    SelectPlan(&'static str),
    /// Select a fixture plan and push its phone detail page.
    PhonePlan(&'static str),
    /// Select a fixture graph member; phone also pushes its task.
    SelectMember(&'static str),
    /// Fixture compact graph rows open a task directly on every form.
    OpenTask(&'static str),
    /// A needs-you fixture member opens its question rather than its task.
    Question,
    /// Fixture acceptance with a fresh confirmation.
    Accept,
    /// The fixture's request-changes receipt.
    RequestChanges,
    /// The fixture's revision-diff receipt.
    Diff,
    /// The fixture's document-opening receipt.
    OpenDocument,
    /// Open fixture Ask with the plan or member context chip.
    Ask(&'static str),
    /// Open the fixture member's session.
    OpenSession(&'static str),
    /// Navigate to the fixture Review.
    ReviewCandidate,
    /// Any press on corrald's plans.
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
    /// Open an open question by key.
    Question(Id),
    /// Open an agent session.
    Session(Id),
    /// Open the candidate under review for a task.
    Review(Id),
    /// Ask about a record with its bounded context and row revision.
    Ask(Live, Option<u64>),
    /// Accept the shown revision through corrald's preview and a fresh confirmation.
    Accept(Id),
    /// Open the request-changes field.
    Changes(Id),
    /// The request-changes note as typed.
    Draft(Id, String),
    /// Send the note against the shown revision.
    SendChanges(Id),
    /// Close the field without sending.
    CancelChanges(Id),
    /// Show the diff from the last reviewed earlier revision to the current one.
    Diff(Id),
    /// Show the plan's document.
    Document(Id),
    /// Back to the review or tracker.
    Overview(Id),
    /// Compare the current revision with another earlier one.
    DiffFrom { plan: Id, from: String },
    /// The owner's resolution of one owner decision, as typed.
    Resolve {
        plan: Id,
        decision: String,
        text: String,
    },
    /// Fetch the diff's next page.
    MoreDiff(Id),
    /// Request the plan's detail again after an error.
    Retry(Id),
    /// Ask corrald for the result of an uncertain command, under the same identity.
    Recover(Id),
}

/// NSH spellings of the Plans screen keys; uppercase J/K cycle members.
pub const KEYS: &[&str] = &["j", "k", "down", "up", "x", "J", "K"];

impl State {
    /// Select S-900 without changing the fixture selection, or leave scale for a fixture plan.
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

    /// Apply one fixture press target; offline acceptance never opens a confirmation.
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
            // Served presses reach `reduce_live`; the preview never runs them.
            Action::Live(_) => vec![],
        }
    }

    /// Apply one press on corrald's plans. Commands leave as typed `Act`s carrying the plan's
    /// own revision; the root previews, confirms and submits them.
    pub fn reduce_live(&mut self, action: LiveAction, feed: &Feed, env: Env) -> Vec<Effect> {
        let live = &mut self.live;
        match action {
            LiveAction::Plan(id) => vec![Effect::OpenLive(Live::Plan(id))],
            LiveAction::Member { plan, member } => {
                live.member = Some((plan, member.clone()));
                if env.form == Form::Phone {
                    vec![Effect::OpenLive(Live::Task(member))]
                } else {
                    vec![]
                }
            }
            LiveAction::Task(id) => vec![Effect::OpenLive(Live::Task(id))],
            LiveAction::Question(key) => vec![Effect::OpenLive(Live::Question(key))],
            LiveAction::Session(id) => vec![Effect::OpenLive(Live::Session(id))],
            LiveAction::Review(task) => vec![Effect::OpenLive(Live::Review(task))],
            LiveAction::Ask(record, rev) => vec![Effect::AskAbout {
                context: Some(record.context(rev)),
                text: None,
                target: None,
            }],
            LiveAction::Accept(id) => {
                let Ok(revision) = live_doc::accept_gate(live, feed, &id, env) else {
                    return vec![];
                };
                let notes =
                    plan_detail(feed, &id).and_then(|detail| live_doc::notes(live, &id, detail));
                let act = Act::new(
                    format!("Accept plan {id} {revision}"),
                    Command::Plan(PlanCommand::Accept {
                        id: id.to_string(),
                        revision: revision.clone(),
                        owner_ok: true,
                        notes,
                    }),
                )
                .confirmed()
                .about(Live::Plan(id.clone()));
                live.accept = Some(Sent {
                    plan: id,
                    key: act.key.clone(),
                    revision,
                });
                vec![Effect::Act(act)]
            }
            LiveAction::Changes(id) => {
                // An open note (unsent, failed, or written for an earlier revision) is kept
                // with its words; one that was sent and done gives way to a fresh one.
                if live_doc::open_draft(live, feed, &id).is_none() {
                    if live.changes.as_ref().is_some_and(|sent| {
                        sent.plan == id
                            && act::status(feed, &sent.key).is_some_and(|status| status.settled())
                    }) {
                        live.changes = None;
                    }
                    live.compose = Some(Draft {
                        revision: plan_detail(feed, &id).and_then(|detail| detail.revision.clone()),
                        plan: id,
                        text: String::new(),
                    });
                }
                vec![]
            }
            LiveAction::Draft(id, text) => {
                if live_doc::changes_in_flight(live, feed, &id) {
                    return vec![];
                }
                match live.compose.as_mut().filter(|draft| draft.plan == id) {
                    Some(draft) => draft.text = text,
                    None => {
                        live.compose = Some(Draft {
                            revision: plan_detail(feed, &id)
                                .and_then(|detail| detail.revision.clone()),
                            plan: id,
                            text,
                        });
                    }
                }
                vec![]
            }
            LiveAction::SendChanges(id) => {
                let Ok(revision) = live_doc::changes_gate(live, feed, &id, env) else {
                    return vec![];
                };
                let Some(text) = live
                    .compose_of(&id)
                    .map(|text| text.trim().to_owned())
                    .filter(|text| !text.is_empty())
                else {
                    return vec![];
                };
                let act = Act::new(
                    format!("Request changes on plan {id} {revision}"),
                    Command::Plan(PlanCommand::RequestChanges {
                        id: id.to_string(),
                        revision: revision.clone(),
                        text,
                    }),
                )
                .about(Live::Plan(id.clone()));
                if let Some(draft) = live.compose.as_mut().filter(|draft| draft.plan == id) {
                    draft.revision = Some(revision.clone());
                }
                live.changes = Some(Sent {
                    plan: id,
                    key: act.key.clone(),
                    revision,
                });
                vec![Effect::Act(act)]
            }
            LiveAction::CancelChanges(id) => {
                if !live_doc::changes_in_flight(live, feed, &id) {
                    live.compose = None;
                    if live.changes.as_ref().is_some_and(|sent| sent.plan == id) {
                        live.changes = None;
                    }
                }
                vec![]
            }
            LiveAction::Diff(id) => {
                let Some((from, to)) = plan_detail(feed, &id).and_then(live_doc::diff_range) else {
                    return vec![];
                };
                open_diff(live, feed, id, from, to)
            }
            LiveAction::Document(id) => {
                live.lens = Some((id, Lens::Document));
                vec![]
            }
            LiveAction::Overview(id) => {
                if live.lens.as_ref().is_some_and(|(plan, _)| *plan == id) {
                    live.lens = None;
                }
                vec![]
            }
            LiveAction::MoreDiff(id) => {
                let Some(Lens::Diff { from, to }) = live.lens_of(&id) else {
                    return vec![];
                };
                let target = DetailRef::PlanDiff {
                    id: id.to_string(),
                    from: from.clone(),
                    to: to.clone(),
                };
                let next = feed
                    .detail(&target)
                    .filter(|cache| !cache.loading)
                    .and_then(|cache| cache.pages.last())
                    .and_then(|page| page.next.clone());
                next.map_or_else(Vec::new, |page| {
                    vec![Effect::Detail(DetailGet {
                        target,
                        etag: None,
                        page: Some(page),
                    })]
                })
            }
            LiveAction::Retry(id) => vec![Effect::Detail(DetailGet {
                target: DetailRef::Plan { id: id.to_string() },
                etag: None,
                page: None,
            })],
            LiveAction::Recover(key) => vec![Effect::Recover(key)],
            LiveAction::Resolve {
                plan,
                decision,
                text,
            } => {
                if live_doc::accept_in_flight(live, feed, &plan) {
                    return vec![];
                }
                if live
                    .resolutions
                    .as_ref()
                    .is_none_or(|(owner, _)| *owner != plan)
                {
                    live.resolutions = Some((plan, vec![]));
                }
                if let Some((_, all)) = live.resolutions.as_mut() {
                    match all.iter_mut().find(|(known, _)| *known == decision) {
                        Some(entry) => entry.1 = text,
                        None => all.push((decision, text)),
                    }
                }
                vec![]
            }
            LiveAction::DiffFrom { plan, from } => {
                let Some(to) = plan_detail(feed, &plan).and_then(|detail| detail.revision.clone())
                else {
                    return vec![];
                };
                open_diff(live, feed, plan, from, to)
            }
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
                    self.member = members[cycle(key, index, members.len())];
                }
            }
            _ => return None,
        }
        Some(vec![])
    }

    /// The same keys over corrald's plans: j/k move the selection through the list order, J/K
    /// cycle the selected plan's members. The synthetic tracker (`x`) is fixture-only.
    pub fn key_live(
        &mut self,
        key: &str,
        _top: Option<&Live>,
        selection: &Selection,
        feed: &Feed,
        _env: Env,
    ) -> Option<Vec<Effect>> {
        let order = live::order(feed);
        let current = live::current_plan(selection, feed);
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
                            .members
                            .iter()
                            .map(|member| Id::from(&member.id))
                            .collect()
                    })
                    .unwrap_or_default();
                if !members.is_empty() {
                    let index = self
                        .live
                        .member_of(&plan)
                        .and_then(|id| members.iter().position(|member| member == id));
                    let next = members[cycle(key, index, members.len())].clone();
                    self.live.member = Some((plan, next));
                }
                Some(vec![])
            }
            _ => None,
        }
    }
}

/// Show the diff between two revisions of `plan`, requesting its first page unless cached.
fn open_diff(live: &mut LiveState, feed: &Feed, plan: Id, from: String, to: String) -> Vec<Effect> {
    let target = DetailRef::PlanDiff {
        id: plan.to_string(),
        from: from.clone(),
        to: to.clone(),
    };
    live.lens = Some((plan, Lens::Diff { from, to }));
    if feed.detail(&target).is_some() {
        vec![]
    } else {
        vec![Effect::Detail(DetailGet {
            target,
            etag: None,
            page: None,
        })]
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
