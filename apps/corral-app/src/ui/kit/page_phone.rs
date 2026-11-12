//! The page pieces the phone mockup introduced, each with its desktop and terminal look: the
//! head of a pushed page (`.pnav`), the group head (`.ph-sec`, the desktop's `.grp`), the page
//! section (`.pband`) and the rounded card group. Re-exported from `kit::page`, so screens reach
//! them as `kit::page::*`.

use nsh_core::prelude::*;

use crate::model::nav::SEARCH_LABEL;
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::band::band_head;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::page::{Title, spacer, title_text};
use crate::ui::kit::text::h3_flush;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color, text_size};

/// The minimum touch target on the phone.
const TARGET: f32 = 44.0;

/// A section of a page (`.pband` on the phone): an optional head (caps label, a note at the
/// trailing edge) over the content, a rule under the section. Desktop: padding 12 24, the head
/// as `text::h3_flush` (the mockup's `.h3` with no space above, as its sections set
/// `margin-top: 0`), a rule running to the note. Phone: padding 14 18 16, the head as
/// `band::band_head` in `hue` when it is not neutral (`.pband .bh`), 8 pt above the content.
/// Terminal: the `text::h3_flush` head, the content a Cell in, a joined rule under them. For the
/// lettered handover sections use `band::band`.
pub fn section(
    app: AppState,
    heading: Option<String>,
    note: Option<String>,
    hue: Hue,
    content: AnyElement,
) -> AnyElement {
    match app.form {
        Form::Desktop => view()
            .px(px(24.0))
            .py(px(12.0))
            .border_b(px(1.0))
            .border_color(color(Token::Rule))
            .when_some(heading, |section, heading| {
                section.child(h3_flush(app, heading, note))
            })
            .child(content)
            .into_element(),
        Form::Phone => view()
            .px(px(18.0))
            .pt(px(14.0))
            .pb(px(16.0))
            .border_b(px(1.0))
            .border_color(color(Token::Rule))
            .when_some(heading, |section, heading| {
                section.child(
                    view()
                        .mb(px(8.0))
                        .child(band_head(app, None, heading, note, hue)),
                )
            })
            .child(content)
            .into_element(),
        Form::Terminal => view()
            .child(
                view()
                    .px(term::cells(1.0))
                    .when_some(heading, |section, heading| {
                        section.child(h3_flush(app, heading, note))
                    })
                    .child(content),
            )
            .child(term::hrule())
            .into_element(),
    }
}

/// The search button beside a phone tab root's large title (`.plarge .rt button`): the
/// magnifier alone, 16 pt in the secondary ink, centred in a round 44 pt target that fills on
/// press; it opens the palette. Pass it as a [`super::page::head`] action. Labelled
/// `nav::SEARCH_LABEL`, test_id "btn-search". Desktop and terminal reach the palette from their
/// shells, so there it is nothing.
pub fn search(app: AppState) -> AnyElement {
    if app.form != Form::Phone {
        return dynamic(|| None::<AnyElement>);
    }
    view()
        .size(px(TARGET))
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .rounded_full()
        .role(AccessibilityRole::Button)
        .label(SEARCH_LABEL)
        .test_id("btn-search")
        .cursor_pointer()
        .active(|s| s.bg(color(Token::Hover)))
        .focus_visible(|s| {
            s.outline(px(2.0))
                .outline_color(color(Token::Focus))
                .outline_offset(px(-2.0))
        })
        .on_press(move || app.open_palette())
        .child(icon(Icon::Search, 16.0, color(Token::Ink2)))
        .into_element()
}

/// The head of a pushed page: a back control naming the page below, the title, an optional
/// trailing control. Phone: the mockup's `.pnav` (back chevron and label in the secondary ink,
/// the title centred in 16 pt bold, a 44 pt trailing slot, a rule under it); pass it to
/// `nav_page().header(..)` as `shell/phone.rs` does for its pushed pages. Desktop: the page
/// head with a quiet back button before the title. Terminal: `‹ <back>` as a pressable entry,
/// the title in bold, a joined rule under them. test_id "page-back" on the back control,
/// "page-title" on the title; the back control is labelled "Back to <back>".
pub fn pushed_head(
    app: AppState,
    title: impl Into<SharedString>,
    back: &'static str,
    on_back: impl Fn() + 'static,
    trailing: Option<AnyElement>,
) -> AnyElement {
    let form = app.form;
    let title: SharedString = title.into();
    let back_label = format!("Back to {back}");
    match form {
        Form::Desktop => view()
            .flex_row()
            .flex_none()
            .items_center()
            .gap(px(12.0))
            .h(px(46.0))
            .px(px(18.0))
            .border_b(px(1.0))
            .border_color(color(Token::Rule))
            .child(button_with(
                app,
                back,
                ButtonKind::Quiet,
                ButtonOptions {
                    icon: Some(Icon::Back),
                    ..ButtonOptions::default()
                },
                "page-back",
                on_back,
            ))
            .child(title_text(form, title, "page-title", Title::Page))
            .child(spacer())
            .children(trailing)
            .into_element(),
        Form::Phone => view()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .min_h(px(TARGET))
            .px(px(16.0))
            .pt(px(8.0))
            .pb(px(6.0))
            .bg(color(Token::Bg))
            .border_b(px(1.0))
            .border_color(color(Token::Rule))
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .flex_shrink_0()
                    .gap(px(2.0))
                    .min_w(px(TARGET))
                    .min_h(px(TARGET))
                    .ml(px(-8.0))
                    .pl(px(4.0))
                    .pr(px(6.0))
                    .rounded(px(10.0))
                    .role(AccessibilityRole::Button)
                    .label(back_label)
                    .test_id("page-back")
                    .cursor_pointer()
                    .active(|s| s.opacity(0.6))
                    .focus_visible(|s| {
                        s.outline(px(2.0))
                            .outline_color(color(Token::Focus))
                            .outline_offset(px(2.0))
                    })
                    .on_press(move || on_back())
                    .child(icon(Icon::Back, 20.0, color(Token::Ink2)))
                    .child(
                        text(back)
                            .font_family(FONT_TEXT)
                            .text_size(px(16.0))
                            .text_color(color(Token::Ink2)),
                    ),
            )
            .child(
                text(title)
                    .test_id("page-title")
                    .role(AccessibilityRole::Header)
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .text_center()
                    .font_family(FONT_TEXT)
                    .text_size(px(16.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(color(Token::Ink)),
            )
            .child(
                view()
                    .flex_row()
                    .justify_end()
                    .flex_shrink_0()
                    .min_w(px(TARGET))
                    .children(trailing),
            )
            .into_element(),
        Form::Terminal => view()
            .flex_none()
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .h(term::rows(1.0))
                    .px(term::cells(1.0))
                    .child(
                        view()
                            .flex_row()
                            .flex_shrink_0()
                            .role(AccessibilityRole::Button)
                            .label(back_label)
                            .test_id("page-back")
                            .cursor_pointer()
                            .on_press(move || on_back())
                            .child(term::quiet("‹ "))
                            .child(text(back).text_color(color(Token::Ink))),
                    )
                    .child(term::space(2.0))
                    .child(title_text(form, title, "page-title", Title::Page))
                    .child(spacer())
                    .children(trailing),
            )
            .child(term::hrule())
            .into_element(),
    }
}

/// A group head in a list (`.grp` on desktop, `.ph-sec` on the phone): the label in its hue,
/// a count or short figure in mono, then a quiet note and a trailing control. Desktop: 12 pt,
/// padding 12 16 4, a rule running to the edge before the note; the label bold for `Hue::You`,
/// semibold otherwise, the secondary ink for `Hue::Neutral`. Phone: 13 pt bold, padding
/// 18 18 6, the note and control at the trailing edge. Terminal: the label bold in its hue,
/// the count quiet, a line to the edge, the note. The label has the header role.
pub fn group_head(
    app: AppState,
    label: impl Into<SharedString>,
    count: Option<String>,
    hue: Hue,
    note: Option<String>,
    trailing: Option<AnyElement>,
) -> AnyElement {
    let form = app.form;
    let label: SharedString = label.into();
    match form {
        Form::Desktop => {
            let (ink, weight) = match hue {
                Hue::Neutral => (color(Token::Ink2), FontWeight::SEMIBOLD),
                Hue::You => (hue_ink(hue), FontWeight::BOLD),
                _ => (hue_ink(hue), FontWeight::SEMIBOLD),
            };
            view()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .px(px(16.0))
                .pt(px(12.0))
                .pb(px(4.0))
                .child(
                    text(label)
                        .role(AccessibilityRole::Header)
                        .flex_shrink_0()
                        .number_of_lines(1u32)
                        .font_family(FONT_TEXT)
                        .text_size(px(12.0))
                        .font_weight(weight)
                        .text_color(ink),
                )
                .when_some(count, |row, count| row.child(figure(form, count)))
                .child(view().flex_1().h(px(1.0)).bg(color(Token::Rule)))
                .when_some(note, |row, note| {
                    row.child(
                        text(note)
                            .flex_shrink_0()
                            .number_of_lines(1u32)
                            .font_family(FONT_TEXT)
                            .text_size(px(11.0))
                            .text_color(color(Token::Ink4)),
                    )
                })
                .children(trailing)
                .into_element()
        }
        Form::Phone => {
            let ink = match hue {
                Hue::Neutral => color(Token::Ink),
                _ => hue_ink(hue),
            };
            view()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .px(px(18.0))
                .pt(px(18.0))
                .pb(px(6.0))
                .child(
                    text(label)
                        .role(AccessibilityRole::Header)
                        .flex_shrink_0()
                        .number_of_lines(1u32)
                        .font_family(FONT_TEXT)
                        .text_size(px(13.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(ink),
                )
                .when_some(count, |row, count| row.child(figure(form, count)))
                .child(spacer())
                .when_some(note, |row, note| {
                    row.child(
                        text(note)
                            .flex_shrink(1.0)
                            .min_w(px(0.0))
                            .truncate()
                            .font_family(FONT_TEXT)
                            .text_size(px(12.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(color(Token::Ink3)),
                    )
                })
                .children(trailing)
                .into_element()
        }
        Form::Terminal => {
            let ink = match hue {
                Hue::Neutral => color(Token::Ink),
                _ => hue_ink(hue),
            };
            view()
                .flex_row()
                .items_center()
                .h(term::rows(1.0))
                .px(term::cells(1.0))
                .child(
                    text(label)
                        .role(AccessibilityRole::Header)
                        .flex_shrink_0()
                        .number_of_lines(1u32)
                        .font_weight(FontWeight::BOLD)
                        .text_color(ink),
                )
                .when_some(count, |row, count| {
                    row.child(term::space(1.0)).child(term::quiet(count))
                })
                .child(term::space(1.0))
                .child(view().flex_1().min_w(px(0.0)).child(term::hrule()))
                .when_some(note, |row, note| {
                    row.child(term::space(1.0)).child(term::quiet(note))
                })
                .when(trailing.is_some(), |row| row.child(term::space(1.0)))
                .children(trailing)
                .into_element()
        }
    }
}

/// Rows in one rounded group with a hairline between them (the phone's grouped list). Phone:
/// surface, radius 14, a `rule` line, as `shell/phone.rs` groups its rows. Desktop: the same at
/// radius 8. Terminal: a framed region with a joined line between the rows.
pub fn card(app: AppState, rows: Vec<AnyElement>) -> AnyElement {
    match app.form {
        Form::Desktop | Form::Phone => view()
            .bg(color(Token::Surface))
            .rounded(px(if app.form == Form::Phone { 14.0 } else { 8.0 }))
            .border_1()
            .border_color(color(Token::Rule))
            .overflow_hidden()
            .children(rows.into_iter().enumerate().map(|(at, row)| {
                view()
                    .when(at > 0, |v| {
                        v.border_t_1().border_t_color(color(Token::Rule))
                    })
                    .child(row)
                    .into_element()
            }))
            .into_element(),
        Form::Terminal => {
            let mut parts = Vec::new();
            for (at, row) in rows.into_iter().enumerate() {
                if at > 0 {
                    parts.push(term::hrule());
                }
                parts.push(row);
            }
            term::framed(view().children(parts)).into_element()
        }
    }
}

/// A count or short figure beside a group label: mono and quiet.
fn figure(form: Form, content: String) -> AnyElement {
    let size = match form {
        Form::Desktop => 11.0,
        Form::Phone => 12.0,
        Form::Terminal => text_size(form),
    };
    text(content)
        .flex_shrink_0()
        .font_family(FONT_MONO)
        .tabular_nums()
        .text_size(px(size))
        .font_weight(FontWeight::MEDIUM)
        .text_color(color(Token::Ink3))
        .into_element()
}
