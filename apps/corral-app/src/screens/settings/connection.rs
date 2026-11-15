//! Settings › Connection. Without a connection: why, the guide to connecting this form (a native
//! app's socket and enrolment, a browser's pairing) and the way to the preview. Against corrald:
//! this device's enrolment and the live connection facts. In the fixture preview: the five
//! connection states to try (pressing one switches the app's connection, so the health line and
//! banner follow), what the current one reports, the local queue while corrald cannot be
//! reached, and which actions queue or refuse then.

use nsh_base::{toggle, toggle_group};
use nsh_core::prelude::*;

use super::parts::{
    act, button_row, cell, first_heading, frame, model_button, mono_cell, read, ring, space,
    test_id, under,
};
use crate::model::settings::connect::{self, Guide};
use crate::model::settings::{self, ConnectionCard, ConnectionView, LocalQueueRow, QueueRule};
use crate::model::{DataState, Form, Hue};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::chip::{chip, illus};
use crate::ui::kit::marks::{hue_ink, mark};
use crate::ui::kit::notice::tone_mark;
use crate::ui::kit::page::Width;
use crate::ui::kit::table::qtable;
use crate::ui::kit::text::{h3, rich};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

/// Connection on every form. Desktop: the state cards in one row of five, the report, the
/// local queue when there is one, the policy table. Phone: the cards two to a row. Terminal:
/// one card per row, the pressed one banded by NSH.
pub fn page(app: AppState) -> AnyElement {
    if untracked(|| app.data.get() == DataState::Disconnected) {
        return disconnected(app);
    }
    if untracked(|| app.daemon()) {
        let details = dynamic(move || {
            let rows = app.feed.with(settings::daemon_connection_details);
            untracked(move || {
                qtable(
                    app,
                    &[Width::Fixed(120.0), Width::Fill],
                    rows.into_iter()
                        .map(|(key, value)| {
                            vec![
                                cell(app, key, Token::Ink3, false).into_element(),
                                mono_cell(app, value, Token::Ink).into_element(),
                            ]
                        })
                        .collect(),
                )
            })
        });
        return frame(
            app,
            "Connection",
            12.0,
            vec![super::enrolment::this_device(app), details],
        );
    }
    let v = untracked(|| read(app, settings::connection));
    let title = if app.form == Form::Phone {
        v.phone_title
    } else {
        v.title
    };
    let report = dynamic(move || {
        let v = read(app, settings::connection);
        untracked(move || report(app, &v))
    });
    let mut parts = Vec::new();
    if crate::client::pairing_supported() {
        parts.extend(pairing(app));
    }
    parts.extend([
        first_heading(app, v.state_heading, None),
        cards(app, &v.cards),
        report,
        h3(app, v.rules_heading, None),
        rules(app, v.rules),
    ]);
    frame(app, title, 12.0, parts)
}

/// This form's guide, read live.
fn guide(app: AppState) -> Memo<Guide> {
    let terminal = app.form == Form::Terminal;
    let origin = crate::client::page_origin();
    computed(move || {
        app.feed.with(|feed| {
            if crate::client::pairing_supported() {
                connect::web_guide(feed, origin.as_deref())
            } else {
                connect::native_guide(feed, terminal)
            }
        })
    })
}

/// A browser's pairing: where it stands (and the code to compare), then the form while a
/// code may be entered.
fn pairing(app: AppState) -> Vec<AnyElement> {
    let shown = guide(app);
    vec![
        dynamic(move || {
            let guide = shown.get();
            untracked(move || super::guide::status(app, &guide))
        }),
        space(app, 12.0),
        super::enrolment::pair_form(app),
    ]
}

/// Connection without a connection: a browser's pairing, or a native app's start; the steps,
/// commands and diagnostics; forgetting a stored pairing that no longer works; the preview.
fn disconnected(app: AppState) -> AnyElement {
    let web = crate::client::pairing_supported();
    let shown = guide(app);
    let mut parts = if web {
        pairing(app)
    } else {
        vec![dynamic(move || {
            let guide = shown.get();
            untracked(move || super::guide::status(app, &guide))
        })]
    };
    if web && crate::client::configured() {
        parts.push(super::enrolment::forget(app));
    }
    parts.push(dynamic(move || {
        let guide = shown.get();
        untracked(move || {
            view()
                .min_w(px(0.0))
                .children(super::guide::body(app, &guide))
                .into_element()
        })
    }));
    parts.push(h3(app, "Without a connection", None));
    parts.push(
        cell(
            app,
            connect::data_source(DataState::Disconnected, false).line,
            Token::Ink3,
            false,
        )
        .into_element(),
    );
    parts.push(space(app, 8.0));
    parts.push(button_row(
        app,
        vec![button_with(
            app,
            connect::PREVIEW,
            ButtonKind::Ghost,
            ButtonOptions::default(),
            "btn-preview",
            move || app.set_data(DataState::Fixture),
        )],
    ));
    frame(app, "Connection", 12.0, parts)
}

/// The state cards as one group of toggles, the pressed one the current connection.
fn cards(app: AppState, cards: &[ConnectionCard]) -> AnyElement {
    let form = app.form;
    let items = cards
        .iter()
        .enumerate()
        .map(|(index, c)| card(app, index, c))
        .collect::<Vec<_>>();
    let group = toggle_group();
    match form {
        Form::Desktop => group
            .flex_row()
            .items_stretch()
            .gap(px(10.0))
            .children(items)
            .into_element(),
        Form::Phone => {
            let mut rows = Vec::new();
            let mut items = items.into_iter();
            while let Some(first) = items.next() {
                let second = items
                    .next()
                    .unwrap_or_else(|| view().flex_1().into_element());
                rows.push(
                    view()
                        .flex_row()
                        .items_stretch()
                        .gap(px(10.0))
                        .child(first)
                        .child(second)
                        .into_element(),
                );
            }
            group.gap(px(10.0)).children(rows).into_element()
        }
        Form::Terminal => group.children(items).into_element(),
    }
}

/// One state card (`.stcard`): the state's dot and name in mono, what it means under it.
/// Pressed (the model's `pressed`, read live) it takes an ink edge; pressing it switches the
/// connection. Terminal: the tone's shaped mark, the name and the meaning on one row.
fn card(app: AppState, index: usize, c: &ConnectionCard) -> AnyElement {
    let form = app.form;
    let action = c.action.clone();
    let pressed = move || {
        read(app, |state, env| {
            settings::connection(state, env)
                .cards
                .get(index)
                .is_some_and(|card| card.pressed)
        })
    };
    let target = toggle()
        .pressed(pressed)
        .on_change(move |_: &bool| act(app, action.clone()))
        .cursor_pointer();
    let id = test_id(&c.action);
    if form == Form::Terminal {
        return target
            .h(term::rows(1.0))
            .child(
                view()
                    .test_id(&id)
                    .flex_row()
                    .items_center()
                    .child(
                        text(tone_mark(c.hue))
                            .flex_shrink_0()
                            .font_weight(FontWeight::BOLD)
                            .text_color(hue_ink(c.hue)),
                    )
                    .child(term::space(1.0))
                    .child(
                        text(c.title)
                            .flex_shrink_0()
                            .number_of_lines(1u32)
                            .text_color(color(Token::Ink)),
                    )
                    .child(term::space(2.0))
                    .child(term::quiet(c.detail)),
            )
            .into_element();
    }
    target
        .flex_1()
        .min_w(px(0.0))
        .px(px(12.0))
        .py(px(10.0))
        .rounded(px(8.0))
        .bg(color(Token::Surface))
        .border(px(1.5))
        .border_color(color(Token::Rule))
        .pressed_style(|s| s.border_color(color(Token::Ink)))
        .hover(|s| s.bg(color(Token::Hover)))
        .focus_visible(ring)
        .child(
            view()
                .test_id(&id)
                .gap(px(6.0))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            view()
                                .size(px(7.0))
                                .flex_shrink_0()
                                .rounded_full()
                                .bg(hue_ink(c.hue))
                                .accessibility_elements_hidden(true),
                        )
                        .child(
                            text(c.title)
                                .flex_1()
                                .min_w(px(0.0))
                                .font_family(FONT_MONO)
                                .text_size(px(11.0))
                                .line_height(px(16.0))
                                .text_color(color(Token::Ink)),
                        ),
                )
                .child(
                    text(c.detail)
                        .font_family(FONT_TEXT)
                        .text_size(px(12.0))
                        .line_height(px(17.0))
                        .text_color(color(Token::Ink3)),
                ),
        )
        .into_element()
}

/// What the current connection reports, its illustrative tag, and the local queue while the
/// daemon cannot be reached.
fn report(app: AppState, v: &ConnectionView) -> Vec<AnyElement> {
    let details = qtable(
        app,
        &[Width::Fixed(120.0), Width::Fill],
        v.details
            .iter()
            .map(|&(key, value)| {
                vec![
                    cell(app, key, Token::Ink3, false).into_element(),
                    mono_cell(app, value, Token::Ink).into_element(),
                ]
            })
            .collect(),
    );
    let mut parts = vec![
        h3(app, v.now_heading, None),
        details,
        under(app, 4.0, illus(app, v.illustrative)),
    ];
    if let Some(heading) = v.local_heading {
        parts.push(h3(app, heading, None));
        parts.push(local_queue(app, &v.local_queue));
    }
    parts
}

/// The local queue: each entry's mark, what it is, when and why (refused in the failure ink),
/// and Discard for what is still queued.
fn local_queue(app: AppState, rows: &[LocalQueueRow]) -> AnyElement {
    let widths = [
        Width::Fixed(32.0),
        Width::Fill,
        Width::Fill,
        Width::Fixed(96.0),
    ];
    qtable(
        app,
        &widths,
        rows.iter()
            .enumerate()
            .map(|(index, row)| {
                let detail_ink = if row.hue == Hue::Fail {
                    Token::Fail
                } else {
                    Token::Ink3
                };
                let discard = match &row.discard {
                    Some(button) => model_button(
                        app,
                        button,
                        ButtonOptions::default(),
                        format!("{}-{index}", test_id(&button.action)),
                    ),
                    None => view().into_element(),
                };
                vec![
                    mark(app, row.mark, Hue::Neutral),
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .child(rich(app, &row.text))
                        .into_element(),
                    cell(app, row.detail, detail_ink, false).into_element(),
                    discard,
                ]
            })
            .collect(),
    )
}

/// Which actions queue and which refuse without a connection, and why.
fn rules(app: AppState, rules: &'static [QueueRule]) -> AnyElement {
    let widths: &[Width] = match app.form {
        Form::Desktop => &[Width::Fixed(284.0), Width::Fixed(70.0), Width::Fill],
        Form::Phone | Form::Terminal => &[Width::Fill, Width::Fixed(80.0), Width::Fill],
    };
    qtable(
        app,
        widths,
        rules
            .iter()
            .map(|rule| {
                let (label, hue) = rule.chip();
                vec![
                    cell(app, rule.action, Token::Ink, false).into_element(),
                    chip(app, label, hue),
                    cell(app, rule.reason, Token::Ink3, false).into_element(),
                ]
            })
            .collect(),
    )
}
