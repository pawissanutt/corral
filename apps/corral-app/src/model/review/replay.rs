//! The fixture replay of T-868's 2026-11-08 review: the explicit preview's content.

use crate::model::effect::Env;
use crate::model::hue::Hue;
use crate::model::nav::Form;
use crate::model::text::span;

use super::data::{
    CANDIDATES, Candidate, Comment, FILES, File, GATE_LOG, GATE_LOG_NOTE, HunkSource, REPORT,
    SCREENSHOT, VERDICTS,
};
use super::diff::{Anchor, hunk_header, hunk_lines};
use super::view::{
    AcceptanceView, Body, ButtonStyle, CandidateGroup, CandidateRow, CommentView, Composer,
    DecideBar, DetailView, DiffView, FileBody, FileView, Header, HunkView, LogView, MessageView,
    PageView, ReportView, ScreenshotView, VerdictView, button, chip, effective_layout,
    hunk_content, layouts, tabs, verdict_view,
};
use super::{APPROVAL_REFUSED, Action, Compose, Layout, State, Tab};

fn fixture_comment(comment: Comment) -> CommentView {
    CommentView {
        key: "fixture".into(),
        anchor: None,
        messages: vec![MessageView {
            attribution: vec![
                span(comment.who).bold(),
                span(" · "),
                span(comment.at),
                span(" · on "),
                span(comment.sha),
            ],
            text: comment.text.into(),
        }],
        chips: Vec::new(),
        composer: Some(Composer {
            draft: "fixture".into(),
            target: Compose::Fixture,
            placeholder: "Reply or add a line comment as owner…",
            label: "Reply to this thread",
            send: "Reply",
            cancel: None,
            refusal: None,
        }),
        actions: Vec::new(),
        status: Vec::new(),
    }
}

/// Builds the fixture's file headings and bodies using an already resolved layout.
pub fn files(layout: Layout) -> Vec<FileView> {
    FILES.iter().map(|file| file_view(file, layout)).collect()
}

fn file_view(file: &File, layout: Layout) -> FileView {
    let body = match hunk_header(file) {
        Some(header) if file.hunk != HunkSource::Collapsed => {
            let anchors: Vec<Anchor> = file
                .comment
                .map(|comment| Anchor {
                    after: comment.after,
                    discussion: 0,
                })
                .into_iter()
                .collect();
            FileBody::Hunks {
                hunks: vec![HunkView {
                    header: header.into(),
                    content: hunk_content(hunk_lines(file), &anchors, layout),
                }],
                notes: Vec::new(),
                tail: None,
            }
        }
        _ => FileBody::Collapsed(button(
            format!(
                "{} changed lines collapsed · expand",
                file.added + file.deleted
            ),
            Action::ExpandFile(file.path),
            ButtonStyle::Ghost,
        )),
    };
    FileView {
        path: file.path.into(),
        heading: vec![
            span(file.path).mono(),
            span(format!(" +{}", file.added)).hue(Hue::Ok),
            span(format!(" −{}", file.deleted)).hue(Hue::Fail),
        ],
        illustrative: file.illustrative.then_some("lines illustrative"),
        ask: file
            .comment
            .map(|_| button("Ask about this hunk", Action::AskHunk, ButtonStyle::Quiet)),
        collapse: None,
        discussions: file.comment.map(fixture_comment).into_iter().collect(),
        loose: Vec::new(),
        annotatable: false,
        candidate: None,
        body,
    }
}

/// Resolves the fixture's stale strike, stale mark, owner hue and older-candidate chips.
pub fn verdicts(phone: bool) -> Vec<VerdictView> {
    VERDICTS
        .iter()
        .map(|verdict| {
            verdict_view(
                phone,
                verdict.who,
                verdict.sha,
                verdict.time,
                verdict.mark,
                verdict.text,
                verdict.stale,
            )
        })
        .collect()
}

fn report() -> ReportView {
    ReportView {
        heading: REPORT.heading.into(),
        paragraphs: REPORT.paragraphs.iter().map(|&p| p.into()).collect(),
        acceptance_heading: REPORT.acceptance_heading,
        acceptance: REPORT
            .acceptance
            .iter()
            .map(|item| AcceptanceView {
                mark: Some(item.mark),
                text: item.text.into(),
            })
            .collect(),
        source: vec![
            span("Source: ").dim(),
            span(REPORT.source).mono().dim(),
            span(REPORT.source_note).dim(),
        ],
    }
}

fn screenshot() -> ScreenshotView {
    ScreenshotView {
        evidence: &SCREENSHOT,
        terminal: vec![
            span(SCREENSHOT.heading).bold(),
            span("\n\n› draft kept: \"tighten the withdraw race test\"\n\n"),
            span("Esc").bold(),
            span("  aborted turn · restored 1 pane item\n     "),
            span("external identities retained: 2").hue(Hue::Ok),
            span("\n     queued  1 steering · 1 follow-up\n\n› tighten the withdraw race test"),
        ],
        caption: vec![
            span(SCREENSHOT.source).dim(),
            span(" · ").dim(),
            span(SCREENSHOT.description).dim(),
        ],
        cursor: true,
    }
}

fn candidate_groups() -> Vec<CandidateGroup> {
    let row = |candidate: &Candidate| CandidateRow {
        task: candidate.task.into(),
        title: candidate.title.into(),
        state: candidate.state.into(),
        hue: candidate.hue,
        identity: vec![
            span(format!("{} · {}", candidate.task, candidate.sha))
                .mono()
                .dim(),
        ],
        selected: candidate.task == "T-868",
        action: Action::Select(candidate.task),
    };
    vec![
        CandidateGroup {
            label: "Your verdict",
            count: 1,
            hue: Hue::You,
            rows: CANDIDATES[..1].iter().map(row).collect(),
        },
        CandidateGroup {
            label: "Verifying",
            count: 3,
            hue: Hue::Corral,
            rows: CANDIDATES[1..].iter().map(row).collect(),
        },
    ]
}

fn decide(env: Env) -> DecideBar {
    let phone = env.form == Form::Phone;
    let mut approve = button("Approve 85b45e1e", Action::Approve, ButtonStyle::Primary);
    approve.refusal = env.conn.refuses().then(|| APPROVAL_REFUSED.to_owned());
    let changes = button(
        if phone { "Changes" } else { "Request changes" },
        Action::RequestChanges,
        ButtonStyle::Ghost,
    );
    DecideBar {
        buttons: if phone {
            vec![changes, approve]
        } else {
            vec![approve, changes]
        },
        secondary: Vec::new(),
        explanation: (!phone).then(|| {
            vec![
                span("Verdicts apply to "),
                span("85b45e1e").mono(),
                span(" only; a new candidate makes them stale. Approving needs a fresh confirm."),
            ]
        }),
        attribution: (!phone)
            .then(|| vec![span(format!("actor=owner · {}", env.via())).mono().dim()]),
        composer: None,
        status: Vec::new(),
    }
}

/// Builds the fixture replay with a wide desktop assumption; phone and terminal stay unified.
pub fn page(state: &State, env: Env) -> PageView {
    page_for_width(state, env, f32::INFINITY)
}

/// Builds the fixture phone variant even if the caller's environment has another form.
pub fn phone(state: &State, env: Env) -> PageView {
    page_for_width(
        state,
        Env {
            form: Form::Phone,
            ..env
        },
        0.0,
    )
}

/// Builds the fixture replay at the available width, applying the 900 pt split threshold.
pub fn page_for_width(state: &State, env: Env, width: f32) -> PageView {
    let phone = env.form == Form::Phone;
    let layout = effective_layout(state, env, width);
    let specs: &[(Tab, &str)] = if phone {
        &[
            (Tab::Diff, "Diff"),
            (Tab::Report, "Report"),
            (Tab::Shot, "Screen"),
        ]
    } else {
        &[
            (Tab::Diff, "Diff · 5 files"),
            (Tab::Report, "Report"),
            (Tab::Shot, "Screenshot"),
            (Tab::Log, "Gate log"),
        ]
    };
    let body = match state.tab {
        Tab::Diff => Body::Diff(DiffView {
            layout,
            notice: phone.then_some("Unified on phones; side by side needs ≥ 900 pt."),
            files: files(layout),
            empty: None,
        }),
        Tab::Report => Body::Report(report()),
        Tab::Shot => Body::Screenshot(screenshot()),
        Tab::Log if phone => Body::Screenshot(screenshot()),
        Tab::Log => Body::Log {
            lines: GATE_LOG
                .iter()
                .map(|line| LogView {
                    text: line.text.into(),
                    hue: line.hue,
                })
                .collect(),
            note: GATE_LOG_NOTE.into(),
        },
    };
    PageView {
        title: "Review".into(),
        back_label: phone.then_some("Back"),
        phone,
        stale: env.conn.refuses(),
        conn: env.conn,
        candidates: if phone {
            Vec::new()
        } else {
            candidate_groups()
        },
        candidate_note: (!phone).then(|| {
            "T-868 here is a replay of its 2026-11-08 review; the rest follow the snapshot.".into()
        }),
        detail: Some(DetailView {
            header: Header {
                title: "host-queue-ledger".into(),
                metadata: if phone {
                    vec![span("T-868 · R1 · 85b45e1e").mono().dim()]
                } else {
                    vec![
                        span("T-868 · R1 · candidate 85b45e1e").mono(),
                        span(" base main · 5 files · +334 −28"),
                    ]
                },
                chips: if phone {
                    vec![chip("replay", Hue::Neutral)]
                } else {
                    Vec::new()
                },
            },
            notice: None,
            verdict_heading: if phone {
                "Verdicts"
            } else {
                "Verdicts · bound to candidate SHA"
            },
            verdict_binding: phone.then_some("bound to SHA"),
            verdicts: verdicts(phone),
            verdicts_empty: None,
            tabs: tabs(state, specs),
            layouts: layouts(state, env),
            body,
        }),
        empty: None,
        decide: decide(env),
    }
}
