//! The composer's receiver picker.

use nsh_base::{menu, menu_item};
use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::session::ReceiverChoice;
use crate::state::AppState;
use crate::ui::kit::overlay::counted;
use crate::ui::term;
use crate::ui::tokens::{Token, color};

use super::super::{PHONE_CHIP, chip_slop};
use super::{act, small};

/// `→ parent ▾`: the parent and every child that takes a message, the current one marked. While
/// the host cannot take a message for its children it is muted with the reason and does not
/// open.
pub(super) fn receiver_menu(
    app: AppState,
    label: String,
    choices: Vec<ReceiverChoice>,
    unavailable: Option<String>,
) -> AnyElement {
    let form = app.form;
    if let Some(why) = unavailable {
        return small(
            form,
            format!("{label} · unavailable: {why}"),
            color(Token::Ink3),
        )
        .test_id("btn-receiver")
        .truncate()
        .flex_shrink(1.0)
        .min_w(px(0.0))
        .into_element();
    }
    // On a phone the chip drops the disclosure mark and keeps a full touch target by hit area.
    let shown = if form == Form::Phone {
        label
            .trim_end_matches(" ▾")
            .trim_end_matches('▾')
            .trim_end()
            .to_owned()
    } else {
        label.clone()
    };
    let trigger = view()
        .test_id("btn-receiver")
        .role(AccessibilityRole::Button)
        .label(format!("Receiver {label}"))
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .cursor_pointer()
        .when(form != Form::Terminal, |t| {
            t.px(px(8.0))
                .h(px(if form == Form::Phone {
                    PHONE_CHIP
                } else {
                    24.0
                }))
                .rounded(px(6.0))
                .border_1()
                .border_color(color(Token::Rule2))
                .hover(|s| s.bg(color(Token::Hover)))
        })
        .when(form == Form::Phone, |t| t.hit_slop(chip_slop()))
        .child(small(form, shown, color(Token::Ink)));
    menu()
        .popup_style(move |s| {
            let s = s
                .bg(color(Token::Surface))
                .border_1()
                .border_color(color(Token::Rule2));
            match form {
                Form::Desktop => s.min_w(px(200.0)).py(px(4.0)).rounded(px(8.0)).shadow_lg(),
                Form::Phone => s.rounded_t(px(14.0)).pt(px(8.0)).pb(px(24.0)),
                Form::Terminal => s.min_w(term::cells(24.0)),
            }
        })
        .backdrop_style(|s| s.bg(color(Token::Shell).opacity(0.5)))
        .on_open_change(counted(app))
        .trigger(trigger)
        .child(
            view().children(
                choices
                    .into_iter()
                    .map(|choice| {
                        let action = choice.action.clone();
                        let test_id = format!(
                            "seg-receiver-{}",
                            choice.child.as_deref().unwrap_or("parent")
                        );
                        menu_item()
                            .accessibility_label(choice.label.clone())
                            .on_select(move || act(app, action.clone()))
                            .cursor_pointer()
                            .hover(|s| s.bg(color(Token::Hover)))
                            .focus(|s| s.bg(color(Token::Hover)))
                            .child(
                                view()
                                    .test_id(&test_id)
                                    .px(if form == Form::Terminal {
                                        term::cells(1.0)
                                    } else {
                                        px(12.0)
                                    })
                                    .py(px(if form == Form::Terminal { 0.0 } else { 6.0 }))
                                    .when(choice.selected, |row| row.bg(color(Token::Sel)))
                                    .child(small(form, choice.label, color(Token::Ink))),
                            )
                            .into_element()
                    })
                    .collect::<Vec<_>>(),
            ),
        )
        .into_element()
}
