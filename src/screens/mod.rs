//! One page per destination over the model. This build has no destination content, so every page is
//! the same frame: the title, the count, and the snapshot's one body line.

pub mod now;

use nsh_core::prelude::*;

use crate::model::{Destination, Form, SettingsSection};
use crate::state::AppState;
use crate::ui::atoms::count;
use crate::ui::term;
use crate::ui::tokens::{Token, color, row_height, text_size};

/// The page for a destination: title (test_id "page-title", the destination's label), its count, and the
/// snapshot's body line (test_id "page-body"). Settings also shows the selected section's label
/// (test_id "page-section"). Destination::Now on Form::Desktop and Form::Terminal is now::workspace(app).
pub fn destination(app: AppState, destination: Destination) -> AnyElement {
    match destination {
        Destination::Now if app.form != Form::Phone => now::workspace(app),
        Destination::Settings => page(
            app,
            destination.label(),
            Some(count(app, destination)),
            Some(section_label(app)),
        ),
        _ => page(
            app,
            destination.label(),
            Some(count(app, destination)),
            None,
        ),
    }
}

/// The page for one Settings section (the phone pushes it): its label as the title, the body line.
pub fn settings_section(app: AppState, section: SettingsSection) -> AnyElement {
    page(app, section.label(), None, None)
}

/// The phone's Settings page head: only the title "Settings" (test_id "page-title"), sized to its
/// content (it must not grow to fill the page), no section label, no body line. The phone lists the
/// sections under it.
pub fn settings_root(app: AppState) -> AnyElement {
    head(app, Destination::Settings.label(), None, None)
}

/// How the frame sits on each form: the desktop's page head of the mock, the phone's large title, and
/// whole cells (8 by 16 points) in a terminal.
struct Frame {
    pad_x: f32,
    head_height: Option<f32>,
    head_pad_y: f32,
    title_scale: f32,
    detail_scale: f32,
    gap: f32,
    body_pad_y: f32,
    /// How the head is set off from the body: a border under it on the desktop; in a terminal,
    /// where a one-row box draws no border, a line of its own that joins the frame around the
    /// page (`├───┤`).
    rule: Rule,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Rule {
    None,
    Border,
    Line,
}

impl Frame {
    fn of(form: Form) -> Frame {
        match form {
            Form::Desktop => Frame {
                pad_x: 18.0,
                head_height: Some(row_height(form) + 16.0),
                head_pad_y: 0.0,
                title_scale: 15.0 / 13.0,
                detail_scale: 12.0 / 13.0,
                gap: 12.0,
                body_pad_y: 16.0,
                rule: Rule::Border,
            },
            Form::Phone => Frame {
                pad_x: 18.0,
                head_height: None,
                head_pad_y: 8.0,
                title_scale: 30.0 / 16.0,
                detail_scale: 1.0,
                gap: 10.0,
                body_pad_y: 8.0,
                rule: Rule::None,
            },
            // One Cell between the head's parts; a blank row between the line and the body (a box
            // around one text keeps no padding rows there).
            Form::Terminal => Frame {
                pad_x: 8.0,
                head_height: Some(row_height(form)),
                head_pad_y: 0.0,
                title_scale: 1.0,
                detail_scale: 1.0,
                gap: 16.0,
                body_pad_y: 0.0,
                rule: Rule::Line,
            },
        }
    }
}

fn page(
    app: AppState,
    title: &'static str,
    count: Option<AnyElement>,
    detail: Option<AnyElement>,
) -> AnyElement {
    let frame = Frame::of(app.form);
    let body = view().px(px(frame.pad_x)).pt(px(frame.body_pad_y)).child(
        text(move || app.snapshot().body_line())
            .test_id("page-body")
            .text_size(px(text_size(app.form)))
            .text_color(color(Token::Ink3)),
    );
    view()
        .size_full()
        .flex_col()
        .bg(color(Token::Bg))
        .child(head(app, title, count, detail))
        .when(frame.rule == Rule::Line, |page| {
            page.child(term::hrule()).child(term::blank())
        })
        .child(body)
        .into_element()
}

/// The page head: the title, then the count and the detail beside it. It never grows.
fn head(
    app: AppState,
    title: &'static str,
    count: Option<AnyElement>,
    detail: Option<AnyElement>,
) -> AnyElement {
    let frame = Frame::of(app.form);
    view()
        .flex_row()
        .flex_none()
        .items_center()
        .gap(px(frame.gap))
        .px(px(frame.pad_x))
        .py(px(frame.head_pad_y))
        .when_some(frame.head_height, |head, height| head.h(px(height)))
        .when(frame.rule == Rule::Border, |head| {
            head.border_b_1().border_color(color(Token::Rule))
        })
        .child(
            text(title)
                .test_id("page-title")
                .text_size(px(text_size(app.form) * frame.title_scale))
                .font_weight(FontWeight::BOLD)
                .text_color(color(Token::Ink))
                .number_of_lines(1),
        )
        .child(count)
        .child(detail)
        .into_element()
}

/// The Settings section shown, after the title as a trail: `Settings › Devices`. The separator is
/// its own text, so the title and the section label each stay exactly what they name; on a terminal
/// grid it keeps the two from reading as one phrase.
fn section_label(app: AppState) -> AnyElement {
    let frame = Frame::of(app.form);
    let size = px(text_size(app.form) * frame.detail_scale);
    view()
        .flex_row()
        .items_center()
        .gap(px(frame.gap))
        .child(
            text("›")
                .text_size(size)
                .text_color(color(Token::Ink4))
                .accessibility_elements_hidden(true),
        )
        .child(
            text(move || app.nav.with(|nav| nav.settings.label()))
                .test_id("page-section")
                .text_size(size)
                .text_color(color(Token::Ink3))
                .number_of_lines(1),
        )
        .into_element()
}
