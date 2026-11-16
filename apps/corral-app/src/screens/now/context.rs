//! A task's context beside its story: the terminal tail (`.term`), which the Session tab and the
//! phone's task page show, and the plan section's body on the phone's task page.

use nsh_core::prelude::*;

use crate::model::work::{self, Notice, Tail, TailTone};
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::kit::chip::{chip, illus};
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::text::{dim, mono};
use crate::ui::tokens::{FONT_MONO, Token, color};

use super::parts::press;

const RING: f32 = 2.0;
const CURSOR_BLINK_MS: f32 = 1100.0;

/// The terminal's ground: dark in both appearances (`--term-bg`). The light theme's terminal is
/// the dark theme's sunk ground; the dark theme's is its shell.
fn term_ground() -> AdaptiveColor {
    AdaptiveColor::new(color(Token::BgSunk).dark, color(Token::Shell).dark)
}

/// A tail line's ink on that dark ground: the dark theme's ink, worker cyan for commands, green
/// for passes (`.term`, `.term .p`, `.term .ok`).
fn term_ink(tone: TailTone) -> AdaptiveColor {
    let token = match tone {
        TailTone::Plain => Token::Ink,
        TailTone::Command => Token::Worker,
        TailTone::Ok => Token::Ok,
    };
    color(token).dark.into()
}

/// The same tones as a terminal shows them: its own accents, ink for plain output.
fn cell_ink(tone: TailTone) -> AdaptiveColor {
    match tone {
        TailTone::Plain => color(Token::Ink),
        TailTone::Command => hue_ink(Hue::Worker),
        TailTone::Ok => hue_ink(Hue::Ok),
    }
}

/// The live or capture tag of a tail: a capture reads as held, a stream as passing.
fn tag(app: AppState, tail: &Tail) -> AnyElement {
    chip(
        app,
        tail.label.clone(),
        if tail.snapshot { Hue::Held } else { Hue::Ok },
    )
}

/// A terminal tail in the mock's terminal look: its pane, runtime and live or capture tag over
/// the lines, with any illustration tag at the trailing edge; the lines mono on the dark
/// terminal ground, the cursor blinking after a live stream. A terminal shows the lines in its
/// own cells. With a `test_id`, the lines are pressable and open the session.
pub(super) fn tail(app: AppState, tail: &Tail, test_id: Option<String>) -> AnyElement {
    let form = app.form;
    let terminal = form == Form::Terminal;
    let head = view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .child(mono(app, tail.pane.clone()).text_color(color(Token::Ink3)))
        .when(!tail.runtime.is_empty(), |head| {
            head.child(dim(app, tail.runtime.clone()))
        })
        .child(tag(app, tail))
        .child(view().flex_1().min_w(px(0.0)))
        .children(tail.illustrative.map(|note| illus(app, note)));
    let size = if form == Form::Phone { 11.0 } else { 11.5 };
    let lines = tail.lines.iter().map(|line| {
        let words = text(line.text.clone()).number_of_lines(1u32);
        if terminal {
            words.text_color(cell_ink(line.tone)).into_element()
        } else {
            words
                .font_family(FONT_MONO)
                .text_size(px(size))
                .line_height(px((size * 1.55).round()))
                .text_color(term_ink(line.tone))
                .into_element()
        }
    });
    let mut screen = view()
        .flex_col()
        .overflow_hidden()
        .children(lines.collect::<Vec<_>>());
    if !terminal {
        screen = screen
            .bg(term_ground())
            .rounded(px(6.0))
            .px(px(12.0))
            .py(px(10.0));
        if tail.cursor {
            let blink = motion().entering(
                MotionSpec::new(KeyframeTiming::new(CURSOR_BLINK_MS).infinite())
                    .values(MotionChannel::Opacity, [1.0, 0.0, 1.0]),
            );
            screen = screen.child(
                view()
                    .w(px(7.0))
                    .h(px(13.0))
                    .mt(px(3.0))
                    .bg(term_ink(TailTone::Plain))
                    .accessibility_elements_hidden(true)
                    .bind_motion(&blink),
            );
        }
    }
    let screen = match (test_id, tail.action.clone()) {
        (Some(id), Some(action)) => screen
            .cursor_pointer()
            .role(AccessibilityRole::Button)
            .label(format!("{} {}", tail.id, tail.label))
            .test_id(&id)
            .on_press(move || press(app, action.clone()))
            .focus_visible(|s| {
                s.outline(px(RING))
                    .outline_color(color(Token::Focus))
                    .outline_offset(px(RING))
            }),
        _ => screen,
    };
    view()
        .gap(px(if terminal { 0.0 } else { 6.0 }))
        .child(head)
        .child(screen)
        .into_element()
}

/// What a plan section holds under its heading: the Plans screen's live graph, or why there is
/// none.
pub(super) fn plan_body(app: AppState, context: &work::LivePlan) -> AnyElement {
    match &context.plan {
        Some(plan) => crate::screens::plans::live_graph(app, plan.clone(), context.member.clone()),
        None => view()
            .children(
                context
                    .empty
                    .as_ref()
                    .map(|empty: &Notice| dim(app, empty.text().to_owned())),
            )
            .into_element(),
    }
}
