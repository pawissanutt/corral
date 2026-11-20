//! The Proof tab of a task-bound session (`session::ProofTab`): where the read of the task's
//! checks stands, the newest checks in words as chips, the ways out (Artifacts narrowed to the
//! task, Open task), then the newest gate and review drawn with the task page's readiness
//! chain (each reviewer's notes quoted), the earlier rounds with theirs, and the task's
//! artifacts. A check is the engine's result on one commit, never acceptance, and the tab says
//! so. test ids `session-proof`, `proof-checks`, `proof-rounds`, `proof-artifacts`,
//! `btn-proof-<link>`.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::review::Rounds;
use crate::model::session::{Body, ProofTab, SessionPage};
use crate::screens::task::candidate::brief::chain;
use crate::state::AppState;
use crate::ui::kit::chip::chip;
use crate::ui::kit::hint::hinted;
use crate::ui::kit::text::h3;
use crate::ui::tokens::{FONT_MONO, Token, color};

use super::cards::{quiet, words};
use super::tabs::{part, surface};
use super::task::link_button;

pub(super) fn tab(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let shown = computed(move || {
        page.with(|p| match &p.body {
            Body::Proof(view) => Some(view.clone()),
            _ => None,
        })
    });
    let content = dynamic(move || {
        shown.with(|proof| {
            proof
                .as_ref()
                .map(|proof| untracked(|| content(app, proof)))
        })
    });
    surface(app, "session-proof", content)
}

fn gap(app: AppState) -> f32 {
    if app.form == Form::Terminal {
        0.0
    } else {
        12.0
    }
}

fn content(app: AppState, proof: &ProofTab) -> AnyElement {
    let form = app.form;
    let mut out = vec![part(app, &proof.part, "proof")];
    if !proof.chips.is_empty() || !proof.links.is_empty() {
        out.push(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap(px(8.0))
                .children(
                    proof
                        .chips
                        .iter()
                        .map(|(words, hue)| chip(app, words.clone(), *hue))
                        .collect::<Vec<_>>(),
                )
                .child(view().flex_1())
                .children(
                    proof
                        .links
                        .iter()
                        .map(|link| link_button(app, link))
                        .collect::<Vec<_>>(),
                )
                .into_element(),
        );
    }
    if !proof.checks.is_empty() {
        out.push(
            view()
                .test_id("proof-checks")
                .child(h3(app, "Gate and review", None))
                .child(chain(app, &proof.checks))
                .child(quiet(form, proof.note))
                .into_element(),
        );
    }
    if let Some(rounds) = &proof.rounds {
        out.push(rounds_block(app, rounds));
    }
    if !proof.links.is_empty() {
        out.push(artifacts(app, proof));
    }
    view().gap(px(gap(app))).children(out).into_element()
}

/// The earlier rounds, each over the checks that judged it, and the reviewer that stays.
fn rounds_block(app: AppState, rounds: &Rounds) -> AnyElement {
    let form = app.form;
    let reviewer = rounds.reviewer.as_ref().map(|line| {
        hinted(
            app,
            words(form, line.text.clone(), color(Token::Ink2)),
            line.hint.clone(),
        )
    });
    let each: Vec<AnyElement> = rounds
        .rounds
        .iter()
        .map(|round| {
            view()
                .gap(px(if form == Form::Terminal { 0.0 } else { 6.0 }))
                .child(
                    words(form, round.title.clone(), color(Token::Ink))
                        .role(AccessibilityRole::Header)
                        .font_weight(if round.current {
                            FontWeight::BOLD
                        } else {
                            FontWeight::SEMIBOLD
                        }),
                )
                .child(chain(app, &round.links))
                .into_element()
        })
        .collect();
    view()
        .test_id("proof-rounds")
        .child(hinted(
            app,
            h3(app, rounds.heading, rounds.count.clone()),
            rounds.hint,
        ))
        .children(reviewer)
        .child(
            view()
                .gap(px(if form == Form::Terminal { 0.0 } else { 12.0 }))
                .children(each),
        )
        .into_element()
}

/// The task's artifacts: each title over its reference, or that none was recorded.
fn artifacts(app: AppState, proof: &ProofTab) -> AnyElement {
    let form = app.form;
    let rows: Vec<AnyElement> = proof
        .artifacts
        .iter()
        .map(|(title, reference)| {
            let reference = text(reference.clone())
                .number_of_lines(1u32)
                .truncate()
                .min_w(px(0.0))
                .text_color(color(Token::Ink3));
            let reference = match form {
                Form::Terminal => reference,
                _ => reference.font_family(FONT_MONO).text_size(px(11.5)),
            };
            view()
                .gap(px(1.0))
                .child(words(form, title.clone(), color(Token::Ink)))
                .child(reference)
                .into_element()
        })
        .collect();
    view()
        .test_id("proof-artifacts")
        .child(h3(
            app,
            "Task artifacts",
            Some(proof.artifacts.len().to_string()),
        ))
        .children(proof.artifacts_empty.map(|empty| quiet(form, empty)))
        .child(
            view()
                .role(AccessibilityRole::List)
                .gap(px(if form == Form::Terminal { 0.0 } else { 8.0 }))
                .children(rows),
        )
        .into_element()
}
