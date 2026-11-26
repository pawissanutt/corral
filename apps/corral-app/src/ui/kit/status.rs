//! The marks of the engine's words: a task's status, a session's state and a pipeline stage,
//! each a shape in its state's colour (`model::status`). Desktop and phone draw the shape from
//! the mark files in `assets/icons/`, a dot or a ring set inside the ring where the shape has
//! one; a terminal shows the mark's glyph in one Cell. Colour says state only: a neutral mark
//! is ink, or the quiet ink where it recedes.

use corral_types::Status;
use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::hue::Hue;
use crate::model::pipeline::PipelineStep;
use crate::model::status::{
    Look, SessionState, Shape, session_look, stage_look, status_look, status_word,
};
use crate::state::AppState;
use crate::ui::kit::hint::hinted;
use crate::ui::kit::marks::hue_ink;
use crate::ui::term::{self, cells};
use crate::ui::tokens::{FONT_TEXT, Token, color, text_size};

const RING: ImageAsset = asset!("icons/mk-next.svg");
const DASHED: ImageAsset = asset!("icons/mk-stale.svg");
const HALF: ImageAsset = asset!("icons/mk-active.svg");
const SOLID: ImageAsset = asset!("icons/actor-solid.svg");
const CHECK: ImageAsset = asset!("icons/mk-done.svg");
const CROSS: ImageAsset = asset!("icons/mk-fail.svg");
const DOT: ImageAsset = asset!("icons/mk-na.svg");

/// `.mk` is 12 points; a phone row's 13.
fn size(form: Form) -> f32 {
    match form {
        Form::Phone => 13.0,
        Form::Desktop | Form::Terminal => 12.0,
    }
}

/// The mark's colour: its state's, ink for a neutral one, the quiet ink where it recedes (a
/// terminal fades ink-4 to nothing, so there ink-3).
fn ink(form: Form, look: Look) -> AdaptiveColor {
    match (look.hue, look.quiet) {
        (Hue::Neutral, true) if form == Form::Terminal => color(Token::Ink3),
        (Hue::Neutral, true) => color(Token::Ink4),
        (hue, _) => hue_ink(hue),
    }
}

/// A status, state or stage mark, named `label` for screen readers.
pub fn look_mark(app: AppState, look: Look, label: impl Into<SharedString>) -> AnyElement {
    let form = app.form;
    let tint = ink(form, look);
    if form == Form::Terminal {
        return text(look.glyph.to_string())
            .flex_shrink_0()
            .number_of_lines(1u32)
            .text_color(tint)
            .role(AccessibilityRole::Image)
            .label(label.into())
            .into_element();
    }
    let size = size(form);
    let drawn = |asset: ImageAsset| {
        image(asset)
            .size(px(size))
            .object_fit(ResizeMode::Contain)
            .tint_color(tint)
            .into_element()
    };
    // What sits inside the ring: a dot, a second ring, or for a dash the bar alone.
    let centred = |inner: View| {
        view()
            .absolute()
            .inset_0()
            .items_center()
            .justify_center()
            .child(inner)
    };
    let face = view().relative().size(px(size)).flex_shrink_0();
    let face = match look.shape {
        Shape::Ring => face.child(drawn(RING)),
        Shape::Dashed => face.child(drawn(DASHED)),
        Shape::Half => face.child(drawn(HALF)),
        Shape::Solid => face.child(drawn(SOLID)),
        Shape::Check => face.child(drawn(CHECK)),
        Shape::Cross => face.child(drawn(CROSS)),
        Shape::Dot => face.child(drawn(DOT)),
        Shape::RingDot => face
            .child(drawn(RING))
            .child(centred(view().size(px(3.5)).rounded_full().bg(tint))),
        Shape::RingRing => face.child(drawn(RING)).child(centred(
            view()
                .size(px(4.5))
                .rounded_full()
                .border(px(1.2))
                .border_color(tint),
        )),
        Shape::Dash => face.child(centred(view().w(px(7.0)).h(px(1.5)).bg(tint))),
    };
    face.role(AccessibilityRole::Image)
        .label(label.into())
        .into_element()
}

/// A task's status mark.
pub fn status_mark(app: AppState, status: Status) -> AnyElement {
    look_mark(app, status_look(status), status_word(status))
}

/// A session's state mark.
pub fn session_mark(app: AppState, state: SessionState) -> AnyElement {
    look_mark(app, session_look(state), state.word())
}

/// A mark and its word in the mark's colour (`.st`): `● Blocked`. One element for screen
/// readers, which hear the word once.
pub fn look_word(app: AppState, look: Look, word: &'static str) -> AnyElement {
    let form = app.form;
    let words = text(word)
        .number_of_lines(1u32)
        .flex_shrink_0()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(match (look.hue, form) {
            (Hue::Neutral, _) => color(Token::Ink),
            (hue, _) => hue_ink(hue),
        });
    let row = view().flex_row().items_center().flex_shrink_0();
    match form {
        Form::Terminal => row
            .child(look_mark(app, look, word))
            .child(term::space(1.0))
            .child(words.accessibility_elements_hidden(true))
            .into_element(),
        Form::Desktop | Form::Phone => row
            .gap(px(6.0))
            .child(look_mark(app, look, word))
            .child(
                words
                    .font_family(FONT_TEXT)
                    .text_size(px(text_size(form) - 1.0))
                    .accessibility_elements_hidden(true),
            )
            .into_element(),
    }
}

/// [`look_word`] for a task's status.
pub fn status_word_mark(app: AppState, status: Status) -> AnyElement {
    look_word(app, status_look(status), status_word(status))
}

/// [`look_word`] for a session's state.
pub fn session_word_mark(app: AppState, state: SessionState) -> AnyElement {
    look_word(app, session_look(state), state.word())
}

/// Run, gate, review and land as four marks in a row (5 pt apart; a Cell apart in a
/// terminal, seven Cells), each with its sentence as its hint. A row's pipeline column.
pub fn pipeline_marks(app: AppState, steps: &[PipelineStep; 4]) -> AnyElement {
    let form = app.form;
    let marks = steps.iter().map(|step| {
        let mark = look_mark(app, stage_look(step.stage), step.spoken());
        let mark = if let Some(open) = step.open.clone() {
            view()
                .test_id(&format!("gate-mark-{}", open.task))
                .role(AccessibilityRole::Button)
                .label(step.spoken())
                .cursor_pointer()
                .on_press(move || app.gate_open.set(Some(open.clone())))
                .child(mark)
                .into_element()
        } else {
            mark
        };
        hinted(app, mark, step.spoken())
    });
    let row = view().flex_row().items_center().flex_shrink_0();
    match form {
        Form::Terminal => {
            let mut row = row;
            for (at, mark) in marks.enumerate() {
                if at > 0 {
                    row = row.child(term::space(1.0));
                }
                row = row.child(mark);
            }
            row.into_element()
        }
        Form::Desktop | Form::Phone => row.gap(px(5.0)).children(marks).into_element(),
    }
}

/// Run, gate, review and land spelled out, one line each: the mark, the stage's name and its
/// sentence. The inspector's and the task page's pipeline. A stage that is not part of where
/// the task is stays listed, quiet, so the four always read in order.
pub fn pipeline_rows(app: AppState, steps: &[PipelineStep; 4]) -> AnyElement {
    let form = app.form;
    let name_width = match form {
        Form::Terminal => cells(7.0),
        Form::Desktop => px(52.0),
        Form::Phone => px(64.0),
    };
    view()
        .test_id("pipeline")
        .role(AccessibilityRole::List)
        .gap(px(if form == Form::Terminal { 0.0 } else { 5.0 }))
        .children(steps.iter().map(|step| {
            let name = text(step.name)
                .number_of_lines(1u32)
                .text_color(color(Token::Ink3));
            let sentence = text(if step.sentence.is_empty() {
                "–".to_owned()
            } else {
                step.sentence.clone()
            })
            .flex_1()
            .min_w(px(0.0))
            .text_color(color(if step.sentence.is_empty() {
                Token::Ink3
            } else {
                Token::Ink
            }));
            let mut row = view()
                .flex_row()
                .items_start()
                .accessible(true)
                .label(step.spoken());
            if let Some(open) = step.open.clone() {
                row = row
                    .test_id(&format!("gate-stage-{}", open.task))
                    .role(AccessibilityRole::Button)
                    .cursor_pointer()
                    .on_press(move || app.gate_open.set(Some(open.clone())));
            }
            let mark = look_mark(app, stage_look(step.stage), step.stage.word());
            match form {
                Form::Terminal => row
                    .child(mark)
                    .child(term::space(1.0))
                    .child(view().w(name_width).flex_shrink_0().child(name))
                    .child(sentence)
                    .into_element(),
                Form::Desktop | Form::Phone => {
                    let size = if form == Form::Phone { 14.0 } else { 12.5 };
                    row.gap(px(8.0))
                        .child(view().h(px(size * 1.45)).justify_center().child(mark))
                        .child(
                            view()
                                .w(name_width)
                                .flex_shrink_0()
                                .child(name.font_family(FONT_TEXT).text_size(px(size))),
                        )
                        .child(
                            sentence
                                .font_family(FONT_TEXT)
                                .text_size(px(size))
                                .line_height(px((size * 1.45_f32).round())),
                        )
                        .into_element()
                }
            }
        }))
        .into_element()
}
