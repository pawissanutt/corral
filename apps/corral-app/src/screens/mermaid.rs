//! A Mermaid fence in an artifact's Markdown: on desktop and web its diagram, read by
//! `model::artifacts::mermaid` and drawn on the lens canvas (no script, no fetch, no markup);
//! elsewhere, or when the app cannot read it, why it is not drawn over the fence's source.
//!
//! test ids: "mermaid-diagram", "mermaid-source".

use std::rc::Rc;

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::artifacts::mermaid;
use crate::screens::system::lens_canvas;
use crate::state::AppState;
use crate::ui::kit::text::dim;

const ELSEWHERE: &str = "A Mermaid diagram, drawn in the desktop and web app; its source follows.";

/// The fence's `code` drawn: its diagram, else a note and `source` (the fence as a code block).
pub(crate) fn block(app: AppState, code: &str, source: AnyElement) -> AnyElement {
    let note = match (app.form, mermaid::parse(code)) {
        (Form::Desktop, Ok(diagram)) => {
            let shown = computed(move || Some(diagram.clone()));
            let picked = computed(|| None);
            return view()
                .test_id("mermaid-diagram")
                .w_full()
                .min_w(px(0.0))
                .child(lens_canvas::diagram(app, shown, picked, Rc::new(|_| {})))
                .into_element();
        }
        (_, Ok(_)) => ELSEWHERE.to_owned(),
        (_, Err(why)) => format!("This Mermaid diagram is not drawn: {why}. Its source follows."),
    };
    view()
        .test_id("mermaid-source")
        .gap(px(if app.form == Form::Terminal { 0.0 } else { 4.0 }))
        .child(dim(app, note))
        .child(source)
        .into_element()
}
