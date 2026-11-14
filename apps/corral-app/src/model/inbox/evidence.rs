//! The fixture's independently loading evidence and the selected artifact contents.

use super::sheet::{Assessment, ButtonKind, DiffLine, EvidenceBody, EvidenceTab, Notice, button};
use super::{Action, EXCERPT, Evidence, LOGTAIL, State};
use crate::model::board::{self, LineKind};
use crate::model::effect::{DataMode, Env};
use crate::model::hue::Hue;
use crate::model::text::span;

/// Build A with loading/error branches independent of the still-usable projected options.
pub fn assessment(state: &State, env: Env) -> Assessment {
    if env.mode == DataMode::Loading {
        return Assessment::Loading {
            skeleton_count: 1,
            note: "Evidence streams in after the sheet renders.",
        };
    }
    if env.mode == DataMode::Error {
        return Assessment::Error(Notice {
            stamp: None,
            hue: Hue::Fail,
            body: vec![
                span("Evidence did not load.").bold(),
                span(
                    " Artifact 9a87439d timed out from the artifact store. The options and consequences below come from the board projection and are current as of rev 2. ",
                ),
            ],
            retry: Some(button(
                "Retry",
                Action::Toast("Retrying evidence fetch…"),
                ButtonKind::Quiet,
            )),
        });
    }
    let tabs = [
        (Evidence::Diff, "Diff hunk", "diff"),
        (Evidence::File, "File excerpt", "file"),
        (Evidence::Log, "Gate log tail", "log"),
        (Evidence::Shot, "Screenshot", "shot"),
    ]
    .into_iter()
    .map(|(evidence, label, slug)| EvidenceTab {
        label: label.into(),
        test_id: format!("seg-evidence-{slug}"),
        selected: state.evidence == evidence,
        action: Action::Evidence(evidence),
    })
    .collect();
    let body = match state.evidence {
        Evidence::Diff => {
            let hunk = board::hunk();
            let (old, new) = hunk.start;
            EvidenceBody::Diff {
                file: hunk.file.into(),
                at: hunk.at.into(),
                lines: number_lines(
                    old,
                    new,
                    hunk.lines
                        .iter()
                        .map(|&(kind, text)| (kind, text.to_owned())),
                ),
                note: vec![
                    span("With option 1, "),
                    span("restoreInput").mono(),
                    span(" returns "),
                    span("undefined").mono(),
                    span(" for Corral/extension items, so "),
                    span("continue").mono(),
                    span(" keeps them queued."),
                ],
            }
        }
        Evidence::File => EvidenceBody::Text {
            file: EXCERPT.file.into(),
            attribution: None,
            lines: EXCERPT
                .lines
                .iter()
                .map(|line| vec![span(*line).mono()])
                .collect(),
            note: None,
        },
        Evidence::Log => EvidenceBody::Text {
            file: LOGTAIL.file.into(),
            attribution: Some("tail illustrative · title real"),
            lines: LOGTAIL
                .lines
                .iter()
                .map(|line| {
                    vec![span(*line).mono().hue(if line.contains("PASS") {
                        Hue::Ok
                    } else {
                        Hue::Neutral
                    })]
                })
                .collect(),
            note: None,
        },
        Evidence::Shot => EvidenceBody::Shot {
            file: "artifacts/T-868/R1/7bc302c8-52x24.ansi · rendered",
            attribution: "content illustrative",
            lines: vec![
                vec![span("pi · bound session · T-868 R1").bold()],
                vec![],
                vec![
                    span("›").dim(),
                    span(" restore the queued edit to my draft"),
                ],
                vec![],
                vec![span("queued").bold(), span("  1 steering · 2 follow-ups")],
                vec![
                    span("Esc").bold(),
                    span("     restored 1 pane item to the editor"),
                ],
                vec![
                    span("        "),
                    span("kept 2 Corral/extension items queued").hue(Hue::Ok),
                ],
                vec![],
                vec![span("›").dim(), span(" ")],
            ],
            cursor: true,
            note: "52x24 plain-mode terminal after pane-only native dequeue (real artifact title).",
        },
    };
    Assessment::Ready {
        label: "Evidence",
        tabs,
        body,
    }
}

/// Number unified lines from the hunk's first old and new line.
pub(crate) fn number_lines(
    mut old: u32,
    mut new: u32,
    lines: impl Iterator<Item = (LineKind, String)>,
) -> Vec<DiffLine> {
    lines
        .map(|(kind, text)| {
            let (old_number, new_number, prefix) = match kind {
                LineKind::Context => {
                    let pair = (Some(old), Some(new), "");
                    old += 1;
                    new += 1;
                    pair
                }
                LineKind::Del => {
                    let pair = (Some(old), None, "-");
                    old += 1;
                    pair
                }
                LineKind::Add => {
                    let pair = (None, Some(new), "+");
                    new += 1;
                    pair
                }
            };
            DiffLine {
                old: old_number,
                new: new_number,
                kind,
                prefix,
                text,
            }
        })
        .collect()
}
