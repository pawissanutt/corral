//! Content descriptors for the list, review document, trackers and phone plan pages.

use super::{
    Action, Document, Graph, PLANS, Plan, PlanState, SCALE, SCALE_LAYERS, ScaleState, State, Story,
    layout, plan, story,
};
use crate::model::board::{Actor, Mark, task};
use crate::model::effect::Env;
use crate::model::hue::Hue;
use crate::model::nav::Form;
use crate::model::text::{Rich, span};

/// A labelled state chip; selection never changes its hue.
#[derive(Debug, Clone, PartialEq)]
pub struct Chip {
    /// Complete chip text.
    pub label: String,
    /// State colour.
    pub hue: Hue,
}

/// Button emphasis from the source template, not renderer-specific styling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    /// Normal command button.
    Standard,
    /// Secondary command.
    Ghost,
    /// Quiet utility action.
    Quiet,
}

/// One complete press target, including offline disabled state.
#[derive(Debug, Clone, PartialEq)]
pub struct Button {
    /// Visible action label.
    pub label: &'static str,
    /// Domain reducer input.
    pub action: Action,
    /// False only when the mockup refuses the command.
    pub enabled: bool,
    /// The decide bar's primary action.
    pub primary: bool,
    /// Command emphasis.
    pub kind: ButtonKind,
}

impl Button {
    /// A secondary command descriptor, useful in the selected-member story too.
    pub fn new(label: &'static str, action: Action) -> Self {
        Self {
            label,
            action,
            enabled: true,
            primary: false,
            kind: ButtonKind::Ghost,
        }
    }
}

/// A member-strip square's state, preserving landed versus merely neutral waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SquareState {
    /// Needs an owner response.
    You,
    /// Landed on the delivery branch.
    Landed,
    /// Corral or a worker is progressing.
    Active,
    /// Held work.
    Held,
    /// Not moving yet.
    Waiting,
}

/// One member square, in member/id order.
#[derive(Debug, Clone, PartialEq)]
pub struct MemberSquare {
    /// Member id for accessibility.
    pub id: &'static str,
    /// Strip-state treatment.
    pub state: SquareState,
    /// State hue; landed and waiting are distinct neutral treatments.
    pub hue: Hue,
}

/// A list row, with all desktop and phone wording already resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanRow {
    /// Real plan or S-900 id.
    pub id: &'static str,
    /// Revision retained as data even when the phone omits it from the id label.
    pub rev: Option<&'static str>,
    /// Id plus revision on desktop; id only on phone.
    pub id_label: String,
    /// Full plan title.
    pub title: &'static str,
    /// Actor colour for the row's state.
    pub hue: Hue,
    /// Complete second line, excluding the separately supplied member squares.
    pub sub: Rich,
    /// Executing members' state squares.
    pub members: Vec<MemberSquare>,
    /// Executing landed/total fraction, otherwise absent.
    pub progress: Option<String>,
    /// Neutral selection emphasis.
    pub selected: bool,
    /// Synthetic marker, absent for real plans and phone scale rows.
    pub illustrative: Option<&'static str>,
    /// Desktop selection or phone navigation action.
    pub action: Action,
}

/// One section of the grouped Plans list.
#[derive(Debug, Clone, PartialEq)]
pub struct ListSection {
    /// Group heading.
    pub label: &'static str,
    /// Mockup section count, absent for phone Scale test.
    pub count: Option<u32>,
    /// Heading state colour.
    pub hue: Hue,
    /// Phone Scale test's synthetic badge.
    pub illustrative: Option<&'static str>,
    /// Ordered plan rows.
    pub rows: Vec<PlanRow>,
}

/// Plans list content, including the phone's large navigation title.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanList {
    /// Navigation title.
    pub title: &'static str,
    /// Phone uses the large title treatment.
    pub large: bool,
    /// Awaiting, executing, approved and synthetic sections in order.
    pub sections: Vec<ListSection>,
}

/// A pipeline stage above a document or tracker.
#[derive(Debug, Clone, PartialEq)]
pub struct PipelineStage {
    /// Complete stage label including any progress count.
    pub label: String,
    /// Done, active or next mark.
    pub mark: Mark,
    /// Active stage colour; done and future stages are neutral.
    pub hue: Hue,
}

/// A detail header, resolved for the current idiom.
#[derive(Debug, Clone, PartialEq)]
pub struct Header {
    /// Status chips, in source order.
    pub chips: Vec<Chip>,
    /// Id and optional revision.
    pub id: String,
    /// Full screen title.
    pub title: &'static str,
    /// Acceptance attribution or review timing replay.
    pub meta: Rich,
    /// Explicit illustrative marker, when the source has one.
    pub illustrative: Option<&'static str>,
    /// Desktop Ask button; phone puts this in its navigation bar.
    pub ask: Option<Button>,
}

/// One proposed-member row in the review document.
#[derive(Debug, Clone, PartialEq)]
pub struct ProposedRow {
    /// Untouched member mark.
    pub mark: Mark,
    /// Proposed name, not a published task id.
    pub name: &'static str,
    /// “can start now” or “after <name>”.
    pub sub: String,
}

/// A review verdict with supersession already resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewRow {
    /// Reviewed revision.
    pub rev: &'static str,
    /// Original verdict mark for revision-aware inspection.
    pub original_mark: Mark,
    /// Stale for superseded P1, otherwise the original mark.
    pub mark: Mark,
    /// Verdict plus superseded aside, with styles preserved.
    pub verdict: Rich,
    /// Reviewer, time and summary with dim attribution.
    pub note: Rich,
    /// Source's subdued superseded row treatment.
    pub superseded: bool,
}

/// Renderable document-section content; the renderer does not branch on domain state.
#[derive(Debug, Clone, PartialEq)]
pub enum SectionBody {
    /// One complete paragraph.
    Paragraph(Rich),
    /// Plain bullet list.
    Bullets(&'static [&'static str]),
    /// Proposed unpublished members.
    Proposed(Vec<ProposedRow>),
    /// Revision-bound review history.
    Reviews(Vec<ReviewRow>),
}

/// Heading and content for one section of the awaiting review document.
#[derive(Debug, Clone, PartialEq)]
pub struct DocumentSection {
    /// Section heading.
    pub heading: &'static str,
    /// Paragraph, list, proposed members or reviews.
    pub body: SectionBody,
}

/// Awaiting P2 review, including decide bar and refusal explanation.
#[derive(Debug, Clone, PartialEq)]
pub struct AwaitingView {
    /// Needs-you/reviewed header.
    pub header: Header,
    /// Design, members, aggregate verification and final acceptance.
    pub pipeline: Vec<PipelineStage>,
    /// Complete document in source order.
    pub sections: Vec<DocumentSection>,
    /// Document artifact attribution.
    pub artifact: Rich,
    /// Desktop or phone decide actions with disabled state resolved.
    pub decide: Vec<Button>,
    /// Offline explanation on desktop; absent on phone as in the mockup.
    pub refusal: Option<&'static str>,
    /// Exact command and actor/via attribution; absent on phone or offline.
    pub attribution: Rich,
}

/// One final-acceptance blocker.
#[derive(Debug, Clone, PartialEq)]
pub struct Blocker {
    /// Needs-your-answer active mark or not-applicable mark.
    pub mark: Mark,
    /// Full blocker phrase.
    pub text: &'static str,
    /// Needs-you hue only for the answer blocker.
    pub hue: Hue,
}

/// Published tracker content and its selected-member inspector.
#[derive(Debug, Clone, PartialEq)]
pub struct ExecutingView {
    /// Executing state, acceptance attribution and Ask.
    pub header: Header,
    /// Four plan stages.
    pub pipeline: Vec<PipelineStage>,
    /// Unmapped-scenario warning, including the source's “final verify has no owner”.
    pub warning: String,
    /// Warning state mark.
    pub warning_mark: Mark,
    /// Members heading.
    pub members_heading: &'static str,
    /// Precomputed dependency cards or compact rows.
    pub graph: Graph,
    /// Blocking-final-acceptance heading.
    pub blockers_heading: &'static str,
    /// Number shown alongside the blockers heading.
    pub blocker_count: usize,
    /// Ordered blockers.
    pub blockers: Vec<Blocker>,
    /// Resolved selected task id, with first-member fallback.
    pub selected: Option<&'static str>,
    /// Desktop/terminal inspector; absent on phone, where members push task pages.
    pub story: Option<Story>,
}

/// Approved plan's intentionally unpublished empty state.
#[derive(Debug, Clone, PartialEq)]
pub struct ApprovedView {
    /// Desktop and phone use different empty-state headings.
    pub heading: &'static str,
    /// The idiom-specific explanation.
    pub body: &'static str,
}

/// An expanded synthetic member that moves or needs attention.
#[derive(Debug, Clone, PartialEq)]
pub struct ScaleLine {
    /// Synthetic task id.
    pub id: &'static str,
    /// Deterministically generated name.
    pub name: String,
    /// Actor/state dot hue.
    pub hue: Hue,
    /// Needs you, held budget, or gating phrase.
    pub status: &'static str,
    /// Status text hue (moving is dim/neutral).
    pub status_hue: Hue,
}

/// A synthetic layer strip with limited expanded active members.
#[derive(Debug, Clone, PartialEq)]
pub struct ScaleLayerView {
    /// Zero-based layer number.
    pub index: usize,
    /// Layer heading.
    pub label: String,
    /// Every member square in id order, including collapsed landed and waiting members.
    pub squares: Vec<MemberSquare>,
    /// Counts phrase with prior-layer dependency already resolved.
    pub summary: String,
    /// Landed count.
    pub landed: usize,
    /// Moving or needs-you count, including held members.
    pub moving: usize,
    /// Waiting count.
    pub waiting: usize,
    /// At most four lines on desktop and three on phone.
    pub shown: Vec<ScaleLine>,
    /// Remaining expanded members, not remaining total members.
    pub hidden: usize,
    /// “+N more in this layer”, absent when all active lines are shown.
    pub more: Option<String>,
}

/// One scale legend item, keeping the two neutral strip treatments distinguishable.
#[derive(Debug, Clone, PartialEq)]
pub struct LegendEntry {
    /// Visible legend label.
    pub label: &'static str,
    /// Square treatment independent of the label.
    pub state: SquareState,
    /// State hue.
    pub hue: Hue,
}

/// Synthetic 64-member tracker, represented as strips on all forms.
#[derive(Debug, Clone, PartialEq)]
pub struct ScaleView {
    /// Executing header with the explicit synthetic marker.
    pub header: Header,
    /// Total member count.
    pub total: usize,
    /// Landed member count.
    pub landed: usize,
    /// Four plan stages with computed landed/total.
    pub pipeline: Vec<PipelineStage>,
    /// Explanation of strip collapse and square order.
    pub explanation: &'static str,
    /// All six layers.
    pub layers: Vec<ScaleLayerView>,
    /// Legend labels, strip states and hues in source order.
    pub legend: Vec<LegendEntry>,
}

/// Selected detail variant, ready for a renderer to walk.
#[derive(Debug, Clone, PartialEq)]
pub enum Detail {
    /// Revision review and decision bar.
    Awaiting(AwaitingView),
    /// Published tracker and member story.
    Executing(ExecutingView),
    /// Approved, but not yet published.
    Approved(ApprovedView),
    /// Explicitly synthetic 64-member tracker.
    Scale(ScaleView),
}

/// Phone plan-page navigation metadata plus its complete detail content.
#[derive(Debug, Clone, PartialEq)]
pub struct PhonePage {
    /// Plan review, tracker id or S-900.
    pub title: &'static str,
    /// Parent list label.
    pub back_label: &'static str,
    /// Back navigation action.
    pub back: crate::model::effect::Effect,
    /// Navigation-bar Ask action, only for real executing trackers.
    pub right: Option<Button>,
    /// Accessibility label for the navigation-bar action.
    pub right_label: Option<&'static str>,
    /// Review, executing, approved or scale body and decide actions.
    pub detail: Detail,
}

/// Number of published members whose land stage has passed.
pub fn landed(p: &Plan) -> usize {
    p.members
        .iter()
        .filter(|id| task(id).rail[3] == Mark::Done)
        .count()
}

fn square(id: &'static str, state: SquareState) -> MemberSquare {
    MemberSquare {
        id,
        state,
        hue: match state {
            SquareState::You => Hue::You,
            SquareState::Active => Hue::Corral,
            SquareState::Held => Hue::Held,
            SquareState::Landed | SquareState::Waiting => Hue::Neutral,
        },
    }
}

fn member_bar(p: &Plan) -> Vec<MemberSquare> {
    p.members
        .iter()
        .map(|id| {
            let t = task(id);
            let state = if t.actor == Actor::You {
                SquareState::You
            } else if t.rail[3] == Mark::Done {
                SquareState::Landed
            } else if matches!(t.actor, Actor::Corral | Actor::Worker) {
                SquareState::Active
            } else if t.actor == Actor::Held {
                SquareState::Held
            } else {
                SquareState::Waiting
            };
            square(id, state)
        })
        .collect()
}

fn actor_hue(state: PlanState) -> Hue {
    match state {
        PlanState::Awaiting => Hue::You,
        PlanState::Executing => Hue::Corral,
        PlanState::Approved => Hue::Neutral,
    }
}

fn row(p: &Plan, state: &State, phone: bool) -> PlanRow {
    let sub = match p.state {
        PlanState::Awaiting => vec![span("reviewed · approve P2 · 3 members").dim()],
        PlanState::Executing => vec![
            span("criteria ").hue(if phone { Hue::Held } else { Hue::Neutral }),
            span(format!(
                "{}/{} mapped",
                p.mapped.unwrap_or(0),
                p.scenarios.unwrap_or(0)
            ))
            .hue(Hue::Held),
        ],
        PlanState::Approved => vec![span(p.note.unwrap_or("")).dim()],
    };
    PlanRow {
        id: p.id,
        rev: p.rev,
        id_label: if phone {
            p.id.into()
        } else {
            p.rev
                .map_or_else(|| p.id.into(), |rev| format!("{} {rev}", p.id))
        },
        title: p.title,
        hue: actor_hue(p.state),
        sub,
        members: if p.state == PlanState::Executing {
            member_bar(p)
        } else {
            vec![]
        },
        progress: if p.state == PlanState::Executing {
            Some(format!("{}/{}", landed(p), p.members.len()))
        } else {
            None
        },
        selected: state.plan == p.id && !state.scale,
        illustrative: None,
        action: if phone {
            Action::PhonePlan(p.id)
        } else {
            Action::SelectPlan(p.id)
        },
    }
}

/// Grouped list content with phone navigation and desktop revisions resolved.
pub fn list(state: &State, env: Env) -> PlanList {
    let phone = env.form == Form::Phone;
    let scale_row = PlanRow {
        id: "S-900",
        rev: None,
        id_label: "S-900".into(),
        title: "64-member tracker",
        hue: Hue::Neutral,
        sub: if phone {
            vec![span("6 layers · 64 members")]
        } else {
            vec![]
        },
        members: vec![],
        progress: None,
        selected: state.scale,
        illustrative: if phone { None } else { Some("synthetic") },
        action: if phone {
            Action::PhonePlan("S-900")
        } else {
            Action::SelectPlan("S-900")
        },
    };
    let groups = [
        ("Awaiting your acceptance", PlanState::Awaiting, 1),
        ("Executing", PlanState::Executing, 4),
        ("Approved · not started", PlanState::Approved, 1),
    ];
    let mut sections = groups
        .into_iter()
        .map(|(label, kind, count)| ListSection {
            label,
            count: Some(count),
            hue: actor_hue(kind),
            illustrative: None,
            rows: PLANS
                .iter()
                .filter(|p| p.state == kind)
                .map(|p| row(p, state, phone))
                .collect(),
        })
        .collect::<Vec<_>>();
    sections.push(ListSection {
        label: "Scale test",
        count: if phone { None } else { Some(1) },
        hue: Hue::Neutral,
        illustrative: if phone { Some("synthetic") } else { None },
        rows: vec![scale_row],
    });
    PlanList {
        title: "Plans",
        large: phone,
        sections,
    }
}

fn pipeline(first: &str, members: String, awaiting: bool) -> Vec<PipelineStage> {
    [
        (
            first.to_owned(),
            if awaiting { Mark::Active } else { Mark::Done },
        ),
        (members, if awaiting { Mark::Next } else { Mark::Active }),
        ("Aggregate verify".into(), Mark::Next),
        ("Final accept".into(), Mark::Next),
    ]
    .into_iter()
    .map(|(label, mark)| PipelineStage {
        label,
        mark,
        hue: if mark == Mark::Active {
            Hue::Corral
        } else {
            Hue::Neutral
        },
    })
    .collect()
}

fn document(doc: &Document) -> Vec<DocumentSection> {
    vec![
        DocumentSection {
            heading: "Problem",
            body: SectionBody::Paragraph(vec![span(doc.problem)]),
        },
        DocumentSection {
            heading: "Goal",
            body: SectionBody::Paragraph(vec![span(doc.goal)]),
        },
        DocumentSection {
            heading: "Changes",
            body: SectionBody::Bullets(doc.changes),
        },
        DocumentSection {
            heading: "Keeps",
            body: SectionBody::Bullets(doc.retained),
        },
        DocumentSection {
            heading: "Not doing",
            body: SectionBody::Bullets(doc.non_goals),
        },
        DocumentSection {
            heading: "Proposed members · published on accept",
            body: SectionBody::Proposed(
                doc.proposed
                    .iter()
                    .map(|m| ProposedRow {
                        mark: Mark::Next,
                        name: m.name,
                        sub: m
                            .needs
                            .first()
                            .map_or_else(|| "can start now".into(), |dep| format!("after {dep}")),
                    })
                    .collect(),
            ),
        },
        DocumentSection {
            heading: "Reviewer verdicts · bound to plan revision",
            body: SectionBody::Reviews(
                doc.reviews
                    .iter()
                    .map(|r| {
                        let superseded = r.rev == "P1";
                        let mut verdict = vec![span(r.verdict)];
                        if superseded {
                            verdict.push(span(" · superseded").dim());
                        }
                        ReviewRow {
                            rev: r.rev,
                            original_mark: r.mark,
                            mark: if superseded { Mark::Stale } else { r.mark },
                            verdict,
                            note: vec![
                                span(format!("{} {} ·", r.who, r.at)).dim(),
                                span(format!(" {}", r.note)),
                            ],
                            superseded,
                        }
                    })
                    .collect(),
            ),
        },
        DocumentSection {
            heading: "What accepting does",
            body: SectionBody::Paragraph(vec![
                span("Publishes 3 member tasks to "),
                span("main").bold(),
                span(
                    " delivery and creates tracker T-893. Autopilot may admit the first member; gates, independent review and landing checks stay unchanged.",
                ),
            ]),
        },
    ]
}

fn awaiting(env: Env) -> AwaitingView {
    let phone = env.form == Form::Phone;
    let off = env.conn.refuses();
    let mut accept = Button::new(
        if phone {
            if off {
                "Accept · refused offline"
            } else {
                "Accept · publish 3"
            }
        } else {
            "Accept · publish 3 members"
        },
        Action::Accept,
    );
    accept.enabled = !off;
    accept.primary = true;
    accept.kind = ButtonKind::Standard;
    let mut decide = vec![];
    if phone && !off {
        decide.push(Button::new("Changes", Action::RequestChanges));
    }
    decide.push(accept);
    if !phone && !off {
        decide.push(Button::new("Request changes", Action::RequestChanges));
        decide.push(Button {
            kind: ButtonKind::Quiet,
            ..Button::new("Diff P1 → P2", Action::Diff)
        });
        decide.push(Button {
            kind: ButtonKind::Quiet,
            ..Button::new("Open document", Action::OpenDocument)
        });
    }
    let doc = &super::DOCUMENT;
    AwaitingView {
        header: Header {
            chips: vec![
                Chip {
                    label: "needs you".into(),
                    hue: Hue::You,
                },
                Chip {
                    label: "reviewed · approve P2".into(),
                    hue: Hue::Ok,
                },
            ],
            id: "T-885 P2".into(),
            title: PLANS[0].title,
            meta: if phone {
                vec![]
            } else {
                vec![span("replay of 12:10–19:12 ")]
            },
            illustrative: if phone { None } else { Some("timing replay") },
            ask: None,
        },
        pipeline: pipeline("Design · awaiting you", "Members 0/3".into(), true),
        sections: document(doc),
        artifact: vec![span("Document ").dim(), span(doc.artifact).mono().dim()],
        decide,
        refusal: if off && !phone {
            Some("Refused offline: acceptance is revision-checked when it applies.")
        } else {
            None
        },
        attribution: if off || phone {
            vec![]
        } else {
            vec![
                span("Runs "),
                span("corral plan accept T-885 --rev P2").mono(),
                span(" after a fresh confirm\n"),
                span("actor=owner · via=app:pawis-desktop").mono(),
            ]
        },
    }
}

fn executing(state: &State, p: &Plan, env: Env) -> ExecutingView {
    let phone = env.form == Form::Phone;
    let selected = p
        .members
        .iter()
        .copied()
        .find(|id| *id == state.member)
        .or_else(|| p.members.first().copied());
    let n = p.scenarios.unwrap_or(0);
    let width = match env.form {
        Form::Phone => 354.0,
        Form::Desktop => 760.0,
        Form::Terminal => 0.0,
    };
    ExecutingView {
        header: Header {
            chips: vec![Chip {
                label: "executing".into(),
                hue: Hue::Corral,
            }],
            id: p.id.into(),
            title: p.title,
            meta: vec![
                span(format!(
                    "accepted {} · delivers to main",
                    p.accepted.unwrap_or("")
                ))
                .dim(),
            ],
            illustrative: None,
            ask: if phone {
                None
            } else {
                Some(Button::new("Ask about this", Action::Ask(p.id)))
            },
        },
        pipeline: pipeline(
            "Design",
            format!("Members {}/{} landed", landed(p), p.members.len()),
            false,
        ),
        warning: format!(
            "{n} of {n} acceptance scenarios are not mapped to any member · final verify has no owner"
        ),
        warning_mark: Mark::Stale,
        members_heading: "Members · live state",
        graph: layout(p, selected.unwrap_or(""), width, phone),
        blockers_heading: "Blocking final acceptance",
        blocker_count: p.blockers.len(),
        blockers: p
            .blockers
            .iter()
            .map(|b| {
                let you = b.contains("your answer");
                Blocker {
                    mark: if you { Mark::Active } else { Mark::Na },
                    text: b,
                    hue: if you { Hue::You } else { Hue::Neutral },
                }
            })
            .collect(),
        selected,
        story: if phone { None } else { selected.map(story) },
    }
}

/// Build all synthetic layer strips with the source's phone/desktop active-row limit.
pub fn scale(_state: &State, env: Env) -> ScaleView {
    let phone = env.form == Form::Phone;
    let mut offset = 0;
    let layers = SCALE_LAYERS
        .iter()
        .enumerate()
        .map(|(index, spec)| {
            let members = &SCALE[offset..offset + spec[0]];
            offset += spec[0];
            let landed = members
                .iter()
                .filter(|m| m.state == ScaleState::Landed)
                .count();
            let waiting = members
                .iter()
                .filter(|m| m.state == ScaleState::Waiting)
                .count();
            let moving = members.len() - landed - waiting;
            let shown = members
                .iter()
                .filter(|m| !matches!(m.state, ScaleState::Landed | ScaleState::Waiting))
                .take(if phone { 3 } else { 4 })
                .map(|m| ScaleLine {
                    id: m.id,
                    name: m.name(),
                    hue: match m.state {
                        ScaleState::You => Hue::You,
                        ScaleState::Held => Hue::Held,
                        _ => Hue::Corral,
                    },
                    status: match m.state {
                        ScaleState::You => "needs you",
                        ScaleState::Held => "held · budget",
                        _ => "gating",
                    },
                    status_hue: match m.state {
                        ScaleState::You => Hue::You,
                        ScaleState::Held => Hue::Held,
                        _ => Hue::Neutral,
                    },
                })
                .collect::<Vec<_>>();
            let hidden = moving - shown.len();
            let mut pieces = vec![];
            if landed > 0 {
                pieces.push(format!("{landed} landed"));
            }
            if moving > 0 {
                pieces.push(format!("{moving} moving or need you"));
            }
            if waiting > 0 {
                pieces.push(format!(
                    "{waiting} waiting on layer {}",
                    index.saturating_sub(1)
                ));
            }
            ScaleLayerView {
                index,
                label: format!("Layer {index}"),
                squares: members
                    .iter()
                    .map(|m| {
                        square(
                            m.id,
                            match m.state {
                                ScaleState::Landed => SquareState::Landed,
                                ScaleState::Active => SquareState::Active,
                                ScaleState::You => SquareState::You,
                                ScaleState::Held => SquareState::Held,
                                ScaleState::Waiting => SquareState::Waiting,
                            },
                        )
                    })
                    .collect(),
                summary: pieces.join(" · "),
                landed,
                moving,
                waiting,
                shown,
                hidden,
                more: if hidden > 0 {
                    Some(format!("+{hidden} more in this layer"))
                } else {
                    None
                },
            }
        })
        .collect();
    let total = SCALE.len();
    let landed = SCALE
        .iter()
        .filter(|m| m.state == ScaleState::Landed)
        .count();
    ScaleView {
        header: Header {
            chips: vec![Chip {
                label: "executing".into(),
                hue: Hue::Corral,
            }],
            id: "S-900".into(),
            title: "Store tracker: 64 members",
            meta: vec![],
            illustrative: Some("synthetic scale test · 64 members · 6 layers"),
            ask: None,
        },
        total,
        landed,
        pipeline: pipeline("Design", format!("Members {landed}/{total} landed"), false),
        explanation: "Landed members collapse into the layer strip; only members that move or need you get a line. Each square is one member, left to right in id order.",
        layers,
        legend: [
            ("landed", SquareState::Landed, Hue::Neutral),
            ("moving", SquareState::Active, Hue::Corral),
            ("needs you", SquareState::You, Hue::You),
            ("held", SquareState::Held, Hue::Held),
            ("waiting", SquareState::Waiting, Hue::Neutral),
        ]
        .into_iter()
        .map(|(label, state, hue)| LegendEntry { label, state, hue })
        .collect(),
    }
}

/// Resolve the selected real-plan or synthetic detail for the current form.
pub fn detail(state: &State, env: Env) -> Detail {
    if state.scale {
        return Detail::Scale(scale(state, env));
    }
    let p = plan(state.plan);
    match p.state {
        PlanState::Awaiting => Detail::Awaiting(awaiting(env)),
        PlanState::Executing => Detail::Executing(executing(state, p, env)),
        PlanState::Approved => Detail::Approved(ApprovedView {
            heading: if env.form == Form::Phone {
                "Approved · not started"
            } else {
                "T-385 P5 · approved, no tracker published"
            },
            body: if env.form == Form::Phone {
                p.note.unwrap_or("")
            } else {
                "4 approvals. Publishing members is a separate owner action."
            },
        }),
    }
}

/// Build the pushed phone page; forces phone wording regardless of the caller's form.
pub fn phone_page(state: &State, env: Env) -> PhonePage {
    let p = plan(state.plan);
    let executing = !state.scale && p.state == PlanState::Executing;
    PhonePage {
        title: if state.scale {
            "S-900"
        } else if p.state == PlanState::Awaiting {
            "Plan review"
        } else {
            p.id
        },
        back_label: "Plans",
        back: crate::model::effect::Effect::Back,
        right: if executing {
            Some(Button::new("Ask about this", Action::Ask(p.id)))
        } else {
            None
        },
        right_label: if executing {
            Some("Ask the assistant about this plan")
        } else {
            None
        },
        detail: detail(
            state,
            Env {
                form: Form::Phone,
                ..env
            },
        ),
    }
}
