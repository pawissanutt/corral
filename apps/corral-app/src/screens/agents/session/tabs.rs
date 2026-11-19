//! The session page's tab strip (`session::TabItem`): the tabs the model offers for the role and
//! what the host serves, each with its count or mark, Chat's unread `•`, and a sub-view named on
//! its tab (`Agents › a2`). Desktop: 13 pt labels under a hairline, the selected one in ink with
//! a 2 pt underline; phone: the same strip scrolling sideways, 44 pt targets; terminal: each tab
//! led by its terminal key (`F9 Chat •`), a joined rule under the strip. A tab the host declares
//! but cannot serve now stays drawn, muted, with its reason, and does not open. A tab list for
//! screen readers; the arrow keys move between tabs. test ids `tab-<slug>`.

use nsh_base::{tab, tabs};
use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::session::{SessionPage, Tab, TabItem};
use crate::state::AppState;
use crate::ui::kit::hint::hint_on_live;
use crate::ui::kit::marks::hue_ink;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color, text_size};

use super::super::run;

/// The phone's least touch target.
const TARGET: f32 = 44.0;
const RING: f32 = 2.0;

pub(super) fn strip(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let form = app.form;
    let shape =
        computed(move || page.with(|p| p.tabs.iter().map(|item| item.tab).collect::<Vec<_>>()));
    let strip = dynamic(move || {
        let tabs_shown = shape.get();
        untracked(|| {
            let strip = tabs().flex_row().items_end();
            let strip = match form {
                Form::Terminal => strip.gap(px(8.0)).px(term::cells(1.0)),
                Form::Desktop | Form::Phone => strip
                    .gap(px(2.0))
                    .px(px(if form == Form::Phone { 10.0 } else { 14.0 }))
                    .indicator_style(|s| {
                        s.bottom(px(-1.0))
                            .h(px(2.0))
                            .mx(px(8.0))
                            .rounded(px(2.0))
                            .bg(color(Token::Ink))
                    }),
            };
            strip
                .children(
                    tabs_shown
                        .into_iter()
                        .map(|shown| one(app, page, shown))
                        .collect::<Vec<_>>(),
                )
                .into_element()
        })
    });
    match form {
        Form::Terminal => view()
            .flex_none()
            .child(strip)
            .child(term::hrule())
            .into_element(),
        Form::Desktop => view()
            .flex_none()
            .border_b(px(1.0))
            .border_color(color(Token::Rule))
            .child(strip)
            .into_element(),
        Form::Phone => scroll()
            .horizontal()
            .flex_none()
            .border_b(px(1.0))
            .border_color(color(Token::Rule))
            .child(strip)
            .into_element(),
    }
}

/// One tab, following its own item.
fn one(app: AppState, page: Memo<SessionPage>, shown: Tab) -> AnyElement {
    let form = app.form;
    let item =
        computed(move || page.with(|p| p.tabs.iter().find(|item| item.tab == shown).cloned()));
    let on = move || item.with(|item| item.as_ref().is_some_and(|item| item.selected));
    let muted =
        move || item.with(|item| item.as_ref().is_some_and(|item| item.unavailable.is_some()));
    let test_id = format!("tab-{}", shown.slug());
    let label = text(move || {
        item.with(|item| {
            item.as_ref()
                .map(|item| item.label.clone())
                .unwrap_or_default()
        })
    })
    .number_of_lines(1u32)
    .text_color(move || {
        if muted() {
            color(Token::Ink3)
        } else if on() || form == Form::Terminal {
            color(Token::Ink)
        } else {
            color(Token::Ink3)
        }
    })
    .when(form != Form::Terminal, |t| {
        t.font_family(FONT_TEXT)
            .text_size(px(if form == Form::Phone {
                15.0
            } else {
                text_size(form)
            }))
            .font_weight(FontWeight::MEDIUM)
    })
    .style_when(on, |s| s.font_weight(FontWeight::SEMIBOLD));
    let key = (form == Form::Terminal).then(|| {
        text(move || {
            item.with(|item| {
                item.as_ref()
                    .and_then(|item| item.key)
                    .unwrap_or_default()
                    .to_owned()
            })
        })
        .number_of_lines(1u32)
        .text_color(color(Token::Ink3))
    });
    let badge = dynamic(move || {
        item.with(|item| {
            let item = item.as_ref()?;
            let badge = item.badge.as_ref()?;
            let words = text(badge.text.clone()).number_of_lines(1u32).text_color(
                if badge.hue == crate::model::Hue::Neutral {
                    color(Token::Ink3)
                } else {
                    hue_ink(badge.hue)
                },
            );
            Some(if form == Form::Terminal {
                words.into_element()
            } else {
                words
                    .font_family(FONT_MONO)
                    .tabular_nums()
                    .text_size(px(11.5))
                    .into_element()
            })
        })
    });
    let unread = dynamic(move || {
        item.with(|item| {
            item.as_ref()
                .filter(|item| item.unread)
                .map(|_| text("•").text_color(color(Token::Ink)).into_element())
        })
    });
    let face = view()
        .test_id(&test_id)
        .flex_row()
        .items_center()
        .gap(px(if form == Form::Terminal { 8.0 } else { 5.0 }))
        .children(key)
        .child(label)
        .child(unread)
        .child(badge);
    let handle = element_ref();
    let press = move || {
        let action = item.with_untracked(|item| {
            item.as_ref()
                .filter(|item| item.unavailable.is_none())
                .map(|item| item.action.clone())
        });
        if let Some(action) = action {
            run(app, action);
        }
    };
    let tab_item = tab()
        .element_ref(&handle)
        .selected(on)
        .on_press(move |_: &()| press())
        .cursor_pointer()
        .items_center()
        .justify_center();
    let tab_item = match form {
        Form::Terminal => tab_item
            .flex_row()
            .child(term::focus_cell(term::focus_of(handle))),
        Form::Desktop => tab_item
            .pt(px(7.0))
            .pb(px(8.0))
            .px(px(10.0))
            .rounded(px(4.0))
            .hover(|s| s.bg(color(Token::Hover)))
            .focus_visible(|s| {
                s.outline(px(RING))
                    .outline_color(color(Token::Focus))
                    .outline_offset(px(RING))
            }),
        Form::Phone => tab_item
            .min_h(px(TARGET))
            .px(px(12.0))
            .rounded(px(6.0))
            .active(|s| s.bg(color(Token::Hover)))
            .focus_visible(|s| {
                s.outline(px(RING))
                    .outline_color(color(Token::Focus))
                    .outline_offset(px(RING))
            }),
    };
    tab_item
        .child(hint_on_live(app, face, move || hint(item)))
        .into_element()
}

/// What the tab's hint says: its label, and why it cannot open now when it cannot.
fn hint(item: Memo<Option<TabItem>>) -> SharedString {
    item.with(|item| match item {
        Some(TabItem {
            label,
            unavailable: Some(why),
            ..
        }) => format!("{label} · unavailable: {why}"),
        Some(item) => item.label.clone(),
        None => String::new(),
    })
    .into()
}
