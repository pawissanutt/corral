//! A plan's document at reading measure (`.doc`): the parent task's body as Markdown, under a
//! caption like the members block's. And the side column's parts: its title (`.phd`) and its
//! blocks (`.sec`).

use nsh_core::prelude::*;

use crate::model::Form;
use crate::screens::markdown::{markdown, provide_styles};
use crate::state::AppState;
use crate::ui::kit::hint::hinted;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

/// The reading measure of a document (`.doc` 76ch at 13.5 pt).
const MEASURE: f32 = 620.0;
const DOCUMENT: &str = "Document";
const DOCUMENT_HINT: &str = "The plan's task body as written; edit it from the plan's Actions menu.";
const NO_DOCUMENT: &str = "The plan's task has no body yet.";

fn element_view() -> View {
    view().min_w(px(0.0))
}

/// The document block: ruled off from what is above it, its caption, then the Markdown at
/// reading measure, or a quiet line when the task has no body. test id "plan-document".
pub(super) fn document(app: AppState, source: &str) -> AnyElement {
    let form = app.form;
    let caption = text(DOCUMENT)
        .role(AccessibilityRole::Header)
        .number_of_lines(1u32)
        .font_weight(FontWeight::SEMIBOLD);
    let caption = if form == Form::Terminal {
        caption
            .font_weight(FontWeight::BOLD)
            .text_color(color(Token::Ink))
    } else {
        caption
            .font_family(FONT_TEXT)
            .text_size(px(if form == Form::Phone { 13.0 } else { 12.0 }))
            .text_color(color(Token::Ink3))
    };
    let body = if source.is_empty() {
        text(NO_DOCUMENT)
            .text_color(color(Token::Ink3))
            .when(form != Form::Terminal, |t| {
                t.font_family(FONT_TEXT).text_size(px(13.0))
            })
            .into_element()
    } else {
        provide_styles(app);
        markdown(app, source, false)
    };
    let block = match form {
        Form::Desktop => element_view()
            .px(px(20.0))
            .pt(px(16.0))
            .pb(px(24.0))
            .border_t(px(1.0))
            .border_color(color(Token::Rule)),
        Form::Phone => element_view()
            .px(px(18.0))
            .pt(px(14.0))
            .pb(px(18.0))
            .border_t(px(1.0))
            .border_color(color(Token::Rule)),
        Form::Terminal => element_view().px(term::cells(1.0)).pt(term::rows(1.0)),
    };
    let block = block
        .child(
            element_view()
                .flex_row()
                .when(form != Form::Terminal, |v| v.mb(px(10.0)))
                .child(hinted(app, caption, DOCUMENT_HINT)),
        )
        .child(
            element_view()
                .when(form == Form::Desktop, |doc| doc.max_w(px(MEASURE)))
                .child(body),
        );
    // A terminal joins its rule across the whole column, outside the Cell of inset.
    let block = if form == Form::Terminal {
        element_view().child(term::hrule()).child(block)
    } else {
        block
    };
    block.test_id("plan-document").into_element()
}

/// A side column's title (`.phd h2`): 16 pt semibold over a rule, padding 14 18 12, its hint on
/// the title. Phone 17 pt, padding 16 18 10. Terminal: bold, a joined rule under it.
pub(super) fn side_title(
    app: AppState,
    title: impl Into<SharedString>,
    hint: &'static str,
) -> AnyElement {
    let form = app.form;
    let words = text(title.into())
        .role(AccessibilityRole::Header)
        .number_of_lines(1u32)
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(color(Token::Ink));
    match form {
        Form::Terminal => element_view()
            .child(element_view().flex_row().px(term::cells(1.0)).child(hinted(
                app,
                words.font_weight(FontWeight::BOLD),
                hint,
            )))
            .child(term::hrule())
            .into_element(),
        Form::Desktop | Form::Phone => {
            let phone = form == Form::Phone;
            element_view()
                .flex_row()
                .px(px(18.0))
                .pt(px(if phone { 16.0 } else { 14.0 }))
                .pb(px(if phone { 10.0 } else { 12.0 }))
                .border_b(px(1.0))
                .border_color(color(Token::Rule))
                .child(hinted(
                    app,
                    words
                        .font_family(FONT_TEXT)
                        .text_size(px(if phone { 17.0 } else { 16.0 })),
                    hint,
                ))
                .into_element()
        }
    }
}

/// A side block (`.sec`): padding 14 18 with a rule under it; a terminal a Cell in, a blank row
/// under it.
pub(super) fn side_block(app: AppState) -> View {
    match app.form {
        Form::Terminal => element_view().px(term::cells(1.0)).pb(term::rows(1.0)),
        Form::Desktop | Form::Phone => element_view()
            .px(px(18.0))
            .py(px(14.0))
            .border_b(px(1.0))
            .border_color(color(Token::Rule)),
    }
}
