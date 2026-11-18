//! The selected task's story in Now's side column and on the phone's task page (`.insp`): its
//! Story (header, a Blocked task's reason, detail and next steps, stepper, body and key/value
//! facts, every engine operation and this device's last operation's outcome), the Evidence list
//! of its artifacts, and the Session hand-off with the run's terminal capture, each with its
//! detail's loading, stale or failed state.

use nsh_core::prelude::*;

use crate::model::board::Mark;
use crate::model::text::span;
use crate::model::work::{
    self, BlockedCard, Dependency, DetailNote, Fact, Inspector, LiveEvidence, LiveSession,
    LiveStory, Press, Stage,
};
use crate::model::{Form, Hue};
use crate::screens::markdown::{markdown, provide_styles};
use crate::state::AppState;
use crate::ui::kit::chip::chip;
use crate::ui::kit::marks::{hue_ink, mark};
use crate::ui::kit::notice::{loading, notice};
use crate::ui::kit::table::{kv, steps};
use crate::ui::kit::text::{dim, h3, mono, plain, rich, rich_small};
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::context::tail;
use super::parts::{model_button, press, press_id, pressable, quiet};
use super::verbs::reasons;

/// The phone's least touch target: a dependency id or the plan link is pressed as a button is.
const TARGET: f32 = 44.0;

/// `.insp`'s padding: the side column's 14 18 24 on desktop, 14 18 on the phone, a Cell in on a
/// terminal.
fn padded(app: AppState, child: AnyElement) -> AnyElement {
    let frame = view();
    match app.form {
        Form::Desktop => frame.px(px(18.0)).pt(px(14.0)).pb(px(24.0)),
        Form::Phone => frame.px(px(18.0)).py(px(14.0)),
        Form::Terminal => frame.px(px(8.0)),
    }
    .child(child)
    .into_element()
}

/// The inspector's body for `inspector`, padded as `.insp` is; the phone's task page shows it
/// too.
pub(super) fn content(app: AppState, inspector: &Inspector) -> AnyElement {
    let body = match inspector {
        Inspector::Loading => loading(
            app,
            work::INSPECTOR_LOADING,
            &[(0.3, 10.0), (0.6, 18.0), (1.0, 60.0)],
        ),
        Inspector::Quiet(words) => return quiet(app, *words),
        Inspector::Unavailable(reason) => note(app, reason),
        Inspector::LiveStory(story) => live_story(app, story),
        Inspector::LiveEvidence(evidence) => live_evidence(app, evidence),
        Inspector::LiveSession(session) => live_session(app, session),
    };
    padded(app, body)
}

fn gap(app: AppState) -> f32 {
    if app.form == Form::Terminal {
        0.0
    } else {
        12.0
    }
}

/// The model's action row under a story: each button by what it does, then why each command
/// that cannot run now cannot.
fn actions(app: AppState, buttons: &[work::Button]) -> AnyElement {
    view()
        .pt(px(if app.form == Form::Terminal {
            0.0
        } else {
            16.0
        }))
        .child(
            view().flex_row().flex_wrap().gap(px(8.0)).children(
                buttons
                    .iter()
                    .map(|button| pressable(app, button, false))
                    .collect::<Vec<_>>(),
            ),
        )
        .children(reasons(app, buttons))
        .into_element()
}

/// A goal or summary paragraph under the title.
fn paragraph(app: AppState, child: AnyElement) -> AnyElement {
    view()
        .pb(px(if app.form == Form::Terminal {
            0.0
        } else {
            14.0
        }))
        .child(child)
        .into_element()
}

/// A detail's loading, stale or failed line in its hue.
fn note(app: AppState, note: &DetailNote) -> AnyElement {
    let words = rich_small(app, &vec![span(note.text.clone()).hue(note.hue)]);
    view()
        .pb(px(if app.form == Form::Terminal {
            0.0
        } else {
            10.0
        }))
        .child(words)
        .into_element()
}

/// A Blocked task's card: the engine's reason on the needs-you ground, its detail as served, and
/// one button per next step, the first primary (`btn-next-<n>`). A forced step reads "… anyway";
/// the app asks once before it runs.
fn blocked(app: AppState, card: &BlockedCard) -> AnyElement {
    let space = |pixels: f32| {
        px(if app.form == Form::Terminal {
            0.0
        } else {
            pixels
        })
    };
    view()
        .gap(space(8.0))
        .pb(space(14.0))
        .test_id("blocked-card")
        .child(notice(
            app,
            Hue::You,
            &vec![span(card.reason.clone()).bold()],
            None,
        ))
        .children(
            card.detail
                .as_ref()
                .map(|detail| plain(app, detail.clone()).into_element()),
        )
        .when(!card.next.is_empty(), |panel| {
            panel.child(
                view().flex_row().flex_wrap().gap(px(8.0)).children(
                    card.next
                        .iter()
                        .map(|button| pressable(app, button, false))
                        .collect::<Vec<_>>(),
                ),
            )
        })
        .into_element()
}

/// The selected task's story: the header, title, stepper and facts from the board row and the
/// task detail.
fn live_story(app: AppState, story: &LiveStory) -> AnyElement {
    let mut out: Vec<AnyElement> = vec![
        head(
            app,
            story.id.clone(),
            Some((story.who, story.who_hue)),
            story.plan.clone(),
            story.changed.clone(),
        ),
        title(app, story.title.clone()),
    ];
    if let Some(detail) = &story.note {
        out.push(note(app, detail));
    }
    if let Some(card) = &story.blocked {
        out.push(blocked(app, card));
    }
    if !story.summary.is_empty() {
        out.push(paragraph(app, rich_small(app, &story.summary)));
    }
    out.push(stepper(app, &story.stages));
    if let Some(source) = &story.body {
        // The task's canonical Markdown, laid out as the transcripts lay out Markdown.
        provide_styles(app);
        out.push(paragraph(app, markdown(app, source, false)));
    }
    let rows = story
        .facts
        .iter()
        .map(|(label, value)| {
            let value = match value {
                Fact::Text(words) => rich(app, words),
                Fact::Tasks(tasks) => ids(app, tasks),
                Fact::Answer(button) => view()
                    .flex_row()
                    .child(model_button(app, button, "btn-answer".into(), false))
                    .into_element(),
            };
            ((*label).to_owned(), value)
        })
        .collect();
    out.push(
        view()
            .pt(px(if app.form == Form::Terminal {
                0.0
            } else {
                14.0
            }))
            .child(kv(app, rows))
            .into_element(),
    );
    if let Some(sent) = &story.sent {
        out.push(
            view()
                .pt(px(if app.form == Form::Terminal {
                    0.0
                } else {
                    12.0
                }))
                .child(rich_small(
                    app,
                    &vec![span(sent.text.clone()).hue(sent.hue)],
                ))
                .into_element(),
        );
    }
    if !story.actions.is_empty() {
        out.push(actions(app, &story.actions));
    }
    view().children(out).into_element()
}

/// The selected daemon task's artifacts: each one's title over its source reference.
fn live_evidence(app: AppState, evidence: &LiveEvidence) -> AnyElement {
    view()
        .child(h3(app, evidence.title.clone(), None))
        .children(evidence.note.as_ref().map(|detail| note(app, detail)))
        .children(evidence.empty.map(|empty| dim(app, empty).into_element()))
        .when(!evidence.rows.is_empty(), |list| {
            list.child(steps(
                app,
                evidence
                    .rows
                    .iter()
                    .map(|(title, source)| {
                        let row = view()
                            .flex_row()
                            .flex_wrap()
                            .gap(px(8.0))
                            .child(plain(app, title.clone()))
                            .child(mono(app, source.clone()).text_color(color(Token::Ink3)));
                        (Mark::Na, Hue::Neutral, row.into_element())
                    })
                    .collect(),
            ))
        })
        .into_element()
}

/// The selected daemon task's Session tab: the hand-off to its transcript, its run, and the
/// captured log tail.
fn live_session(app: AppState, session: &LiveSession) -> AnyElement {
    view()
        .gap(px(gap(app)))
        .child(dim(app, session.description))
        .children(session.note.as_ref().map(|detail| note(app, detail)))
        .children(session.open.as_ref().map(|open| {
            view()
                .flex_row()
                .child(model_button(app, open, "btn-session-open".into(), false))
                .into_element()
        }))
        .when(!session.run.is_empty(), |panel| {
            panel.child(rich_small(app, &session.run))
        })
        .children(
            session
                .tail
                .as_ref()
                .map(|captured| tail(app, captured, Some("btn-live-session".into()))),
        )
        .children(session.empty.map(|empty| dim(app, empty).into_element()))
        .into_element()
}

/// The id, who acts next (in the Daemon state), the plan it belongs to (a press opens the plan),
/// and the changed-since chip.
fn head(
    app: AppState,
    id: impl Into<SharedString>,
    who: Option<(&'static str, Hue)>,
    plan: Option<(String, Press)>,
    changed: Option<String>,
) -> AnyElement {
    let form = app.form;
    let plan = plan.map(|(label, action)| {
        let words = text(label.clone())
            .number_of_lines(1u32)
            .text_color(color(Token::Ink3));
        let words = if form == Form::Terminal {
            words
        } else {
            words.font_family(FONT_TEXT).text_size(px(12.0))
        };
        view()
            .when(form == Form::Phone, |link| {
                link.justify_center().min_h(px(TARGET))
            })
            .cursor_pointer()
            .role(AccessibilityRole::Link)
            .label(label)
            .test_id(&press_id(&action))
            .on_press(move || press(app, action.clone()))
            .child(words)
            .into_element()
    });
    view()
        .flex_row()
        .items_center()
        .flex_wrap()
        .gap(px(8.0))
        // Ids are one token: cut at the edge, never broken across lines (TT-19).
        .child(
            mono(app, id)
                .number_of_lines(1u32)
                .truncate()
                .text_color(color(Token::Ink3)),
        )
        .children(who.map(|(who, hue)| chip(app, who, hue)))
        .children(plan)
        .children(changed.map(|changed| chip(app, changed, Hue::Neutral)))
        .into_element()
}

/// The task's title (`.insp h2`): 16 pt bold on desktop, 18 pt on the phone, bold in a terminal.
fn title(app: AppState, title: impl Into<SharedString>) -> AnyElement {
    let words = text(title.into())
        .role(AccessibilityRole::Header)
        .font_weight(FontWeight::BOLD)
        .text_color(color(Token::Ink));
    match app.form {
        Form::Terminal => words.into_element(),
        form => words
            .font_family(FONT_TEXT)
            .text_size(px(if form == Form::Phone { 18.0 } else { 16.0 }))
            .letter_spacing(px(-0.16))
            .mt(px(2.0))
            .mb(px(2.0))
            .into_element(),
    }
}

/// Run, gate, review, land side by side (`.stepper`): each stage's name in faint capitals, its
/// mark and state word, its detail in mono; the active stage under a 2 pt line in its hue.
fn stepper(app: AppState, stages: &[Stage; 4]) -> AnyElement {
    let form = app.form;
    let phone = form == Form::Phone;
    let columns = stages.iter().map(|stage| {
        let edge = if stage.active {
            hue_ink(stage.hue)
        } else {
            color(Token::Rule)
        };
        let label = text(stage.label)
            .uppercase()
            .number_of_lines(1u32)
            .text_color(color(Token::Ink3));
        let word = text(stage.word)
            .number_of_lines(1u32)
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(color(Token::Ink));
        let detail = text(stage.detail.clone())
            .number_of_lines(1u32)
            .truncate()
            .text_color(color(Token::Ink3));
        let (label, word, detail) = if form == Form::Terminal {
            (label, word, detail)
        } else {
            (
                label
                    .font_family(FONT_TEXT)
                    .text_size(px(if phone { 12.0 } else { 11.0 }))
                    .letter_spacing(px(0.66)),
                word.font_family(FONT_TEXT)
                    .text_size(px(if phone { 16.0 } else { 13.0 })),
                detail
                    .font_family(FONT_MONO)
                    .text_size(px(if phone { 12.0 } else { 11.0 })),
            )
        };
        let mark_hue = if stage.active {
            stage.hue
        } else {
            Hue::Neutral
        };
        view()
            .flex_1()
            .min_w(px(0.0))
            .when(form != Form::Terminal, |column| {
                column
                    .pr(px(10.0))
                    .pt(px(if stage.active { 8.0 } else { 9.0 }))
                    .border_t(px(if stage.active { 2.0 } else { 1.0 }))
                    .border_t_color(edge)
            })
            .role(AccessibilityRole::Text)
            .label(format!("{} {} {}", stage.label, stage.word, stage.detail))
            .child(label)
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .py(px(if form == Form::Terminal { 0.0 } else { 2.0 }))
                    .child(mark(app, stage.mark, mark_hue))
                    .child(word),
            )
            .child(detail)
            .into_element()
    });
    view()
        .flex_row()
        .gap(px(if form == Form::Terminal { 8.0 } else { 0.0 }))
        .pt(px(if form == Form::Terminal { 0.0 } else { 4.0 }))
        .pb(px(if form == Form::Terminal { 0.0 } else { 16.0 }))
        .children(columns.collect::<Vec<_>>())
        .into_element()
}

/// Task ids with their marks where the model shows them; pressing an id opens that task.
fn ids(app: AppState, dependencies: &[Dependency]) -> AnyElement {
    view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(10.0))
        .children(
            dependencies
                .iter()
                .map(|dependency| {
                    let action = dependency.action.clone();
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(4.0))
                        .when(app.form == Form::Phone, |link| {
                            link.justify_center().min_w(px(TARGET)).min_h(px(TARGET))
                        })
                        .cursor_pointer()
                        .role(AccessibilityRole::Link)
                        .label(format!("{} {}", dependency.id, dependency.mark.label()))
                        .test_id(&format!("btn-dep-{}", dependency.id))
                        .on_press(move || press(app, action.clone()))
                        .child(
                            mono(app, dependency.id.clone())
                                .number_of_lines(1u32)
                                .truncate(),
                        )
                        .when(dependency.show_mark, |link| {
                            link.child(mark(app, dependency.mark, Hue::Neutral))
                        })
                        .into_element()
                })
                .collect::<Vec<_>>(),
        )
        .into_element()
}
