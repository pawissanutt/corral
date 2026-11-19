//! The typed card an assistant reply carries: the host starting. A question the session asks
//! is answered in the session page's dock, not in the thread.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::assistant::Chip;
use crate::model::board::Mark;
use crate::state::AppState;
use crate::ui::kit::marks::{hue_fill, mark};
use crate::ui::term;
use crate::ui::tokens::{Token, text_size};

use super::parts::{strong_run, words};

/// The host starting (`.hoststate`): the running mark, the state in bold and what it means, on
/// the corral ground.
pub fn starting(app: AppState, state: &Chip, explanation: &str) -> AnyElement {
    let form = app.form;
    let size = if form == Form::Phone {
        15.0
    } else {
        text_size(form)
    };
    let sentence = words(app, "", size, Token::Ink)
        .flex_1()
        .min_w(px(0.0))
        .child(strong_run(state.label.clone()))
        .child(text(format!(" · {explanation}")));
    let line = view()
        .flex_row()
        .items_center()
        .bg(hue_fill(state.hue))
        .child(mark(app, Mark::Active, state.hue))
        .when(form == Form::Terminal, |r| r.child(term::space(1.0)))
        .child(sentence);
    if form == Form::Terminal {
        return line.into_element();
    }
    line.gap(px(10.0))
        .px(px(12.0))
        .py(px(10.0))
        .rounded(px(8.0))
        .into_element()
}
