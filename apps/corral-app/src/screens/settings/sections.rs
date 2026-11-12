//! Settings › Machines, Notifications, Assistant, Autopilot & grants and General, and the
//! phone's root list of sections.

use nsh_base::toggle;
use nsh_core::prelude::*;

use super::parts::{
    act, action_button, cell, first_heading, frame, intro, mono_cell, read, ring, space, test_id,
    under,
};
use crate::model::settings::{self, AssistantView, NotificationView, OwnerView};
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::chip::{chip, illus};
use crate::ui::kit::notice::notice;
use crate::ui::kit::page::{Width, pane};
use crate::ui::kit::rows::list_row;
use crate::ui::kit::table::{devtable, qtable};
use crate::ui::kit::text::{h3, rich_small};
use crate::ui::tokens::{FONT_TEXT, Token, color};

/// Machines: the machine table (the name with its illustrative tag, role, runtime, agents,
/// reach in mono), then the notice that more machines come later.
pub fn machines(app: AppState) -> AnyElement {
    let v = untracked(|| read(app, settings::machines));
    let widths = [
        Width::Fill,
        Width::Fixed(120.0),
        Width::Fill,
        Width::Fixed(72.0),
        Width::Fixed(80.0),
    ];
    let rows = v
        .machines
        .iter()
        .map(|machine| {
            vec![
                view()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex_row()
                    .flex_wrap()
                    .items_baseline()
                    .gap(px(6.0))
                    .child(cell(app, machine.name, Token::Ink, true))
                    .child(illus(app, machine.illustrative))
                    .into_element(),
                cell(app, machine.role, Token::Ink, false).into_element(),
                cell(app, machine.runtime, Token::Ink, false).into_element(),
                cell(app, machine.agents, Token::Ink, false).into_element(),
                mono_cell(app, machine.reach, Token::Ink).into_element(),
            ]
        })
        .collect();
    // The phone's records carry the page's 18 pt sides themselves.
    let bleed = if app.form == Form::Phone { -18.0 } else { 0.0 };
    frame(
        app,
        v.title,
        12.0,
        vec![
            view()
                .mx(px(bleed))
                .child(devtable(app, &widths, &v.columns, rows))
                .into_element(),
            space(app, 14.0),
            notice(app, Hue::Neutral, &v.notice, None),
        ],
    )
}

/// Notifications: each kind with what it covers, and its switch at the trailing edge (the
/// on or off chip; pressing it flips the kind); then how push works and the Ask hotkey.
pub fn notifications(app: AppState) -> AnyElement {
    let v = untracked(|| read(app, settings::notifications));
    let rows = v
        .rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            vec![
                kind(app, row),
                view()
                    .flex_1()
                    .flex_row()
                    .justify_end()
                    .child(switch(app, index, row))
                    .into_element(),
            ]
        })
        .collect();
    frame(
        app,
        v.title,
        12.0,
        vec![
            qtable(app, &[Width::Fill, Width::Fixed(72.0)], rows),
            under(app, 8.0, rich_small(app, &v.note)),
        ],
    )
}

/// A notification kind in bold over what it covers.
fn kind(app: AppState, row: &NotificationView) -> AnyElement {
    let detail = text(row.notification.detail).text_color(color(Token::Ink3));
    let detail = if app.form == Form::Terminal {
        detail
    } else {
        let size = if app.form == Form::Phone { 13.0 } else { 12.0 };
        detail
            .font_family(FONT_TEXT)
            .text_size(px(size))
            .line_height(px((size * 1.4).round()))
    };
    view()
        .flex_1()
        .min_w(px(0.0))
        .child(cell(app, row.notification.label, Token::Ink, true))
        .child(detail)
        .into_element()
}

/// A kind's switch: a toggle showing the model's on or off chip, read live, so it redraws
/// alone; pressing it runs the model's toggle. Phone: a 44 pt target around the chip.
fn switch(app: AppState, index: usize, row: &NotificationView) -> AnyElement {
    let action = row.action.clone();
    let shown = move || {
        read(app, |state, env| {
            settings::notifications(state, env)
                .rows
                .get(index)
                .map(|row| (row.on, row.label, row.hue))
        })
    };
    let face = dynamic(move || {
        let current = shown();
        untracked(move || current.map(|(_, label, hue)| chip(app, label, hue)))
    });
    toggle()
        .pressed(move || shown().is_some_and(|(on, _, _)| on))
        .on_change(move |_: &bool| act(app, action.clone()))
        .accessibility_label(row.notification.label)
        .flex_row()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .when(app.form == Form::Desktop, |t| {
            t.rounded(px(4.0))
                .hover(|s| s.opacity(0.85))
                .focus_visible(ring)
        })
        .when(app.form == Form::Phone, |t| {
            t.min_w(px(44.0))
                .min_h(px(44.0))
                .rounded(px(8.0))
                .active(|s| s.opacity(0.7))
                .focus_visible(ring)
        })
        .child(view().test_id(&test_id(&row.action)).child(face))
        .into_element()
}

/// Assistant: what the quick thread answers with, the models its selector offers (the default
/// with its chip, the others with Remove), Add a profile, and who writes the summary when the
/// quick thread rolls over.
pub fn assistant(app: AppState) -> AnyElement {
    let v = untracked(|| read(app, settings::assistant));
    frame(
        app,
        v.title,
        12.0,
        vec![
            intro(app, v.intro),
            space(app, 12.0),
            first_heading(app, v.models_heading, Some(v.models_key)),
            profiles(app, &v),
            space(app, 8.0),
            view()
                .flex_row()
                .child(action_button(app, &v.add))
                .into_element(),
            h3(app, v.summary_heading, Some(v.summary_key.to_owned())),
            summary(app, &v),
            under(app, 6.0, illus(app, v.illustrative)),
        ],
    )
}

/// The selector's profiles: name in bold, model in quiet mono, then the default's chip or
/// Remove at the trailing edge.
fn profiles(app: AppState, v: &AssistantView) -> AnyElement {
    let widths: &[Width] = match app.form {
        Form::Desktop => &[Width::Fixed(230.0), Width::Fill, Width::Fixed(220.0)],
        Form::Phone | Form::Terminal => &[Width::Fixed(96.0), Width::Fill, Width::Fill],
    };
    let rows = v
        .profiles
        .iter()
        .map(|row| {
            let end = match (&row.chip, &row.remove) {
                (Some((label, hue)), _) => chip(app, *label, *hue),
                (None, Some(button)) => action_button(app, button),
                (None, None) => view().into_element(),
            };
            vec![
                cell(app, row.profile.name, Token::Ink, true).into_element(),
                mono_cell(app, row.profile.model, Token::Ink3).into_element(),
                view()
                    .flex_1()
                    .flex_row()
                    .justify_end()
                    .child(end)
                    .into_element(),
            ]
        })
        .collect();
    qtable(app, widths, rows)
}

/// The rollover summary: which profile writes it and at what price, and what runs when it
/// cannot.
fn summary(app: AppState, v: &AssistantView) -> AnyElement {
    let s = &v.summary;
    let widths: &[Width] = match app.form {
        Form::Desktop => &[Width::Fixed(224.0), Width::Fixed(260.0), Width::Fill],
        Form::Phone | Form::Terminal => &[Width::Fill, Width::Fill, Width::Fill],
    };
    let writer = view()
        .flex_1()
        .min_w(px(0.0))
        .flex_row()
        .flex_wrap()
        .items_baseline()
        .gap(px(4.0))
        .child(cell(app, s.profile, Token::Ink, true))
        .child(mono_cell(app, s.model, Token::Ink3))
        .into_element();
    let rows = vec![
        vec![
            cell(app, s.label, Token::Ink, false).into_element(),
            writer,
            cell(app, s.detail, Token::Ink3, false).into_element(),
        ],
        vec![
            cell(app, s.fallback_label, Token::Ink, false).into_element(),
            cell(app, s.fallback, Token::Ink, false).into_element(),
            cell(app, s.fallback_detail, Token::Ink3, false).into_element(),
        ],
    ];
    qtable(app, widths, rows)
}

/// Autopilot & grants: the owner-only explanation.
pub fn autopilot(app: AppState) -> AnyElement {
    owner(app, untracked(|| read(app, settings::autopilot)))
}

/// General: the owner-only explanation.
pub fn general(app: AppState) -> AnyElement {
    owner(app, untracked(|| read(app, settings::general)))
}

fn owner(app: AppState, v: OwnerView) -> AnyElement {
    frame(app, v.title, 12.0, vec![rich_small(app, &v.description)])
}

/// The phone's Settings root: one row per section with the model's subtitle (the connection's
/// follows the current reach) and a chevron; a row opens its section through the model. It
/// scrolls itself, the Ask clearance under it.
pub fn root(app: AppState) -> AnyElement {
    let v = untracked(|| read(app, settings::phone_root));
    let rows = v
        .rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let action = row.action.clone();
            let subtitle = move || {
                read(app, |state, env| {
                    settings::phone_root(state, env)
                        .rows
                        .get(index)
                        .map_or("", |row| row.subtitle)
                })
            };
            let words = view()
                .flex_1()
                .min_w(px(0.0))
                .gap(px(2.0))
                .child(
                    text(row.label)
                        .font_family(FONT_TEXT)
                        .text_size(px(16.0))
                        .text_color(color(Token::Ink)),
                )
                .child(
                    text(subtitle)
                        .font_family(FONT_TEXT)
                        .text_size(px(13.0))
                        .line_height(px(18.0))
                        .text_color(color(Token::Ink3)),
                )
                .into_element();
            list_row(
                app,
                row.label,
                test_id(&row.action),
                || false,
                move || act(app, action.clone()),
                vec![words, icon(Icon::Chevron, 16.0, color(Token::Ink3))],
            )
        })
        .collect::<Vec<_>>();
    pane(
        app,
        "settings-root",
        view()
            .mt(px(8.0))
            .border_t_1()
            .border_t_color(color(Token::Rule))
            .children(rows)
            .into_element(),
    )
}
