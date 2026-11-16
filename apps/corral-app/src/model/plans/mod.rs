//! Pure plan state, canonical actions and feed-backed presentation for every form.

mod graph;
mod live;
mod live_doc;
mod live_role;
mod live_story;
mod planning;
mod story;
mod view;

pub use graph::{CardGeometry, Edge, Graph, GraphInput, GraphMember, Layer, Node, Point, layout};
pub use live::{
    current_plan_filtered, graph_input, live_detail, live_list_filtered, live_phone_page,
    plan_detail,
};
pub use story::{Story, StoryField, StoryStage};
pub use view::{
    ApprovedView, AwaitingView, Blocker, Button, ButtonKind, Chip, Compose, Decide, DecideNote,
    DecisionRow, Detail, DiffLine, DiffLineKind, DiffView, DocumentSection, DocumentView,
    ExecutingView, Header, ListNotice, ListSection, MemberSquare, NoticeView, PhonePage,
    PipelineStage, PlanList, PlanRow, PlanningView, ProposedRow, ReviewRow, SectionBody,
    SquareState,
};

use corral_protocol::cmd::Command;
use corral_protocol::cmd::plan::PlanCommand;
use corral_protocol::detail::{DetailGet, DetailRef};

use crate::model::act::{self, Act};
use crate::model::daemon::Feed;
use crate::model::effect::{Effect, Env};
use crate::model::nav::Form;
use crate::model::{Id, Live, Selection};
/// Captions shared by all form-specific loading notices.
pub const LIST_LOADING: &str = "Loading plans…";
pub const DETAIL_LOADING: &str = "Loading the plan…";

/// Plan-local interaction state; the selected plan belongs to the root selection.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct State {
    pub ids: crate::model::id::IdSupply,
    pub live: LiveState,
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
    /// Request the plan's detail again after an error.
    Retry(Id),
    /// Ask corrald for the result of an uncertain command, under the same identity.
    Recover(Id),
}

/// Member cycling keys; list movement is bound only while the list has focus.
pub const KEYS: &[&str] = &["J", "K"];

impl State {
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
                    &mut self.ids,
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
                    &mut self.ids,
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
        let order = live::order_filtered(feed, filter);
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
