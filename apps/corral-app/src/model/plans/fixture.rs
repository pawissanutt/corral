//! The mockup's Plans preview: the six fixture plans, the T-885 review document and the
//! synthetic S-900 tracker, built into the shared descriptors. Never used in daemon mode.

use super::view::{pipeline, square};
use super::{
    Action, ApprovedView, AwaitingView, Blocker, Button, ButtonKind, Chip, Decide, DecideNote,
    Detail, Document, DocumentSection, ExecutingView, Header, LegendEntry, ListSection,
    MemberSquare, PLANS, PhonePage, Plan, PlanList, PlanRow, PlanState, ProposedRow, ReviewRow,
    SCALE, SCALE_LAYERS, ScaleLayerView, ScaleLine, ScaleState, ScaleView, SectionBody,
    SquareState, State, fixture_graph, plan, story,
};
use crate::model::Id;
use crate::model::board::{Actor, Mark, task};
use crate::model::effect::Env;
use crate::model::hue::Hue;
use crate::model::nav::Form;
use crate::model::text::span;

/// Number of published members whose land stage has passed.
pub fn landed(p: &Plan) -> usize {
    p.members
        .iter()
        .filter(|id| task(id).rail[3] == Mark::Done)
        .count()
}

fn member_bar(p: &Plan) -> Vec<MemberSquare> {
    p.members
        .iter()
        .map(|&id| {
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
            square(Id::from(id), state)
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
        id: Id::from(p.id),
        rev: p.rev.map(str::to_owned),
        id_label: if phone {
            p.id.into()
        } else {
            p.rev
                .map_or_else(|| p.id.into(), |rev| format!("{} {rev}", p.id))
        },
        title: p.title.into(),
        hue: actor_hue(p.state),
        sub,
        members: if p.state == PlanState::Executing {
            member_bar(p)
        } else {
            vec![]
        },
        progress: (p.state == PlanState::Executing)
            .then(|| format!("{}/{}", landed(p), p.members.len())),
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
        id: Id::from("S-900"),
        rev: None,
        id_label: "S-900".into(),
        title: "64-member tracker".into(),
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
        notice: None,
        sections,
    }
}

fn bullets(items: &[&str]) -> SectionBody {
    SectionBody::Bullets(items.iter().map(|&item| item.to_owned()).collect())
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
            body: bullets(doc.changes),
        },
        DocumentSection {
            heading: "Keeps",
            body: bullets(doc.retained),
        },
        DocumentSection {
            heading: "Not doing",
            body: bullets(doc.non_goals),
        },
        DocumentSection {
            heading: "Proposed members · published on accept",
            body: SectionBody::Proposed(
                doc.proposed
                    .iter()
                    .map(|m| ProposedRow {
                        mark: Mark::Next,
                        name: m.name.into(),
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
                            rev: r.rev.into(),
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
    let mut buttons = vec![];
    if phone && !off {
        buttons.push(Button::new("Changes", Action::RequestChanges));
    }
    buttons.push(accept);
    if !phone && !off {
        buttons.push(Button::new("Request changes", Action::RequestChanges));
        buttons.push(Button::new("Diff P1 → P2", Action::Diff).quiet());
        buttons.push(Button::new("Open document", Action::OpenDocument).quiet());
    }
    let doc = &super::DOCUMENT;
    AwaitingView {
        header: Header {
            plan: None,
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
            title: PLANS[0].title.into(),
            meta: if phone {
                vec![]
            } else {
                vec![span("replay of 12:10–19:12 ")]
            },
            illustrative: if phone { None } else { Some("timing replay") },
            actions: vec![],
            stale: None,
        },
        pipeline: pipeline("Design · awaiting you", "Members 0/3".into(), true),
        sections: document(doc),
        artifact: vec![span("Document ").dim(), span(doc.artifact).mono().dim()],
        decide: Decide {
            buttons,
            compose: None,
            note: (off && !phone).then(|| DecideNote {
                text: "Refused offline: acceptance is revision-checked when it applies.".into(),
                hue: Hue::Fail,
            }),
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
    ExecutingView {
        header: Header {
            plan: None,
            chips: vec![Chip {
                label: "executing".into(),
                hue: Hue::Corral,
            }],
            id: p.id.into(),
            title: p.title.into(),
            meta: vec![
                span(format!(
                    "accepted {} · delivers to main",
                    p.accepted.unwrap_or("")
                ))
                .dim(),
            ],
            illustrative: None,
            actions: if phone {
                vec![]
            } else {
                vec![Button::new("Ask about this", Action::Ask(p.id))]
            },
            stale: None,
        },
        pipeline: pipeline(
            "Design",
            format!("Members {}/{} landed", landed(p), p.members.len()),
            false,
        ),
        warning: Some(format!(
            "{n} of {n} acceptance scenarios are not mapped to any member · final verify has no owner"
        )),
        warning_mark: Mark::Stale,
        members_heading: "Members · live state",
        members: fixture_graph(p),
        blockers_heading: "Blocking final acceptance",
        blocker_count: p.blockers.len(),
        blockers: p
            .blockers
            .iter()
            .map(|b| {
                let you = b.contains("your answer");
                Blocker {
                    mark: if you { Mark::Active } else { Mark::Na },
                    text: (*b).into(),
                    hue: if you { Hue::You } else { Hue::Neutral },
                }
            })
            .collect(),
        selected: selected.map(Id::from),
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
                            Id::from(m.id),
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
                more: (hidden > 0).then(|| format!("+{hidden} more in this layer")),
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
            plan: None,
            chips: vec![Chip {
                label: "executing".into(),
                hue: Hue::Corral,
            }],
            id: "S-900".into(),
            title: "Store tracker: 64 members".into(),
            meta: vec![],
            illustrative: Some("synthetic scale test · 64 members · 6 layers"),
            actions: vec![],
            stale: None,
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

/// Resolve the selected fixture or synthetic detail for the current form.
pub fn detail(state: &State, env: Env) -> Detail {
    if state.scale {
        return Detail::Scale(scale(state, env));
    }
    let p = plan(state.plan);
    match p.state {
        PlanState::Awaiting => Detail::Awaiting(awaiting(env)),
        PlanState::Executing => Detail::Executing(executing(state, p, env)),
        PlanState::Approved => Detail::Approved(ApprovedView {
            header: None,
            heading: if env.form == Form::Phone {
                "Approved · not started".into()
            } else {
                "T-385 P5 · approved, no tracker published".into()
            },
            body: if env.form == Form::Phone {
                p.note.unwrap_or("").into()
            } else {
                "4 approvals. Publishing members is a separate owner action.".into()
            },
            actions: vec![],
            stale: None,
        }),
    }
}

/// Build the pushed phone page; forces phone wording regardless of the caller's form.
pub fn phone_page(state: &State, env: Env) -> PhonePage {
    let p = plan(state.plan);
    let executing = !state.scale && p.state == PlanState::Executing;
    PhonePage {
        title: if state.scale {
            "S-900".into()
        } else if p.state == PlanState::Awaiting {
            "Plan review".into()
        } else {
            p.id.into()
        },
        right: executing.then(|| Button::new("Ask about this", Action::Ask(p.id))),
        right_label: executing.then_some("Ask the assistant about this plan"),
        detail: detail(
            state,
            Env {
                form: Form::Phone,
                ..env
            },
        ),
    }
}
