//! The assistant workspace's landing: what a daemon with no configured projects offers. It says
//! why the board is absent, opens Ask, and says how a project is added. Its board destinations
//! are not shown at all, so this is where such a workspace lands. test ids: "landing",
//! "page-title", "btn-ask".

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::projects::assistant_landing;
use crate::state::AppState;
use crate::ui::icons::Icon;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::page::{body, measure, measured_head, search};
use crate::ui::kit::text::{h3, plain};
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, MEASURE_PAGE, Token, color, text_size};

/// The measure of the landing's prose on desktop and phone.
const MEASURE: f32 = 560.0;

pub fn screen(app: AppState) -> AnyElement {
    let form = app.form;
    let landing = assistant_landing();
    let ask = button_with(
        app,
        landing.ask_label,
        ButtonKind::Primary,
        ButtonOptions {
            icon: Some(Icon::Ask),
            ..ButtonOptions::default()
        },
        "btn-ask",
        move || app.open_ask(),
    );
    let content = if form == Form::Terminal {
        view()
            .test_id("landing")
            .child(term::blank())
            .child(text(landing.explanation).text_color(color(Token::Ink)))
            .child(term::blank())
            .child(view().flex_row().child(ask))
            .child(term::blank())
            .child(h3(app, landing.instructions_heading, None))
            .child(text(landing.instructions).text_color(color(Token::Ink)))
    } else {
        let size = text_size(form) + 1.0;
        view()
            .test_id("landing")
            .max_w(px(MEASURE))
            .when(form == Form::Phone, |v| v.px(px(18.0)).pt(px(6.0)))
            .gap(px(16.0))
            .child(
                text(landing.explanation)
                    .font_family(FONT_TEXT)
                    .text_size(px(size))
                    .line_height(px((size * 1.5).round()))
                    .text_color(color(Token::Ink2)),
            )
            // A phone's floating Ask is this page's one Ask; desktop has no such button.
            .when(form != Form::Phone, |v| {
                v.child(view().flex_row().child(ask))
            })
            .child(
                view()
                    .mt(px(8.0))
                    .pt(px(4.0))
                    .border_t_1()
                    .border_color(color(Token::Rule))
                    .child(h3(app, landing.instructions_heading, None))
                    .child(plain(app, landing.instructions).text_color(color(Token::Ink2))),
            )
    };
    let actions = if form == Form::Phone {
        vec![search(app)]
    } else {
        Vec::new()
    };
    view()
        .flex_1()
        .flex_col()
        .min_w(px(0.0))
        .min_h(px(0.0))
        // Home's place, so a desktop keeps it to Home's measure, centred, as the dashboard.
        .child(measured_head(
            app,
            MEASURE_PAGE,
            landing.title,
            None,
            actions,
        ))
        .child(body(
            app,
            "landing-pane",
            measure(app, MEASURE_PAGE, content.into_element()),
        ))
        .into_element()
}
