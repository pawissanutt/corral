//! The session page's head (`session::Header`): back to Agents, who the session is (`build ·
//! auth-refactor`) with its id, the needs-you and decisions chips, the state word with its mark
//! (a current notice in its place for a few seconds), and the ⋯ menu of the session's actions;
//! under it the muted second line. Desktop: one 46 pt row and the line under it; terminal: the
//! same as two rows of Cells; phone: the shell's head names the session and holds ⋯, the page
//! leads with one state line. The facts redraw on their own, so the open menu and its trigger
//! stay mounted while the state word ticks.

use nsh_base::{menu, menu_item};
use nsh_core::element::StyleRefinement;
use nsh_core::prelude::*;

use crate::model::session::{ActionRow, Header, SessionPage};
use crate::model::{Form, Hue};
use crate::screens::plans::menu_trigger;
use crate::state::AppState;
use crate::ui::icons::Icon;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::chip::chip;
use crate::ui::kit::overlay::counted;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::super::run;
use super::{hue_words, state_mark};

/// The phone's least touch target.
const TARGET: f32 = 44.0;
/// The desktop menu's width range.
const MENU_MIN: f32 = 220.0;
const MENU_MAX: f32 = 340.0;

/// The page head on desktop and in a terminal.
pub(super) fn head(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let form = app.form;
    let header = computed(move || page.with(|p| p.header.clone()));
    let back = untracked(|| header.with(|h| h.back.clone()));
    let target = move || header.with(|h| h.target.clone());
    let shown_id = move || header.with(|h| h.shown_id.clone());
    let line2 = computed(move || header.with(|h| h.line2.clone()));
    let back_action = back.1.clone();
    if form == Form::Terminal {
        let first = view()
            .flex_row()
            .items_center()
            .h(term::rows(1.0))
            .px(term::cells(1.0))
            .child(
                view()
                    .flex_row()
                    .flex_shrink_0()
                    .role(AccessibilityRole::Button)
                    .label(format!("Back to {}", back.0))
                    .test_id("page-back")
                    .cursor_pointer()
                    .on_press(move || run(app, back_action.clone()))
                    .child(term::quiet("‹ "))
                    .child(text(back.0).text_color(color(Token::Ink))),
            )
            .child(term::space(2.0))
            .child(
                text(target)
                    .test_id("page-title")
                    .role(AccessibilityRole::Header)
                    .number_of_lines(1u32)
                    .truncate()
                    .flex_shrink(1.0)
                    .min_w(px(0.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(color(Token::Ink)),
            )
            .child(term::space(1.0))
            .child(
                text(shown_id)
                    .number_of_lines(1u32)
                    .flex_shrink_0()
                    .text_color(color(Token::Ink3)),
            )
            .child(view().flex_1().min_w(px(0.0)))
            .child(facts(app, header))
            .child(term::space(1.0))
            .child(actions(app, page));
        return view()
            .flex_none()
            .child(first)
            .child(dynamic(move || {
                line2.get().map(|words| {
                    view()
                        .px(term::cells(1.0))
                        .child(term::quiet(words).number_of_lines(1u32).truncate())
                        .into_element()
                })
            }))
            .into_element();
    }
    let first = view()
        .flex_row()
        .items_center()
        .gap(px(12.0))
        .h(px(46.0))
        .px(px(18.0))
        .child(button_with(
            app,
            back.0,
            ButtonKind::Quiet,
            ButtonOptions {
                icon: Some(Icon::Back),
                ..ButtonOptions::default()
            },
            "page-back",
            move || run(app, back_action.clone()),
        ))
        .child(
            text(target)
                .test_id("page-title")
                .role(AccessibilityRole::Header)
                .number_of_lines(1u32)
                .truncate()
                .flex_shrink(1.0)
                .min_w(px(0.0))
                .font_family(FONT_TEXT)
                .text_size(px(15.0))
                .letter_spacing(px(-0.08))
                .font_weight(FontWeight::BOLD)
                .text_color(color(Token::Ink)),
        )
        .child(
            text(shown_id)
                .number_of_lines(1u32)
                .flex_shrink_0()
                .font_family(FONT_MONO)
                .text_size(px(12.5))
                .text_color(color(Token::Ink3)),
        )
        .child(view().flex_1().min_w(px(0.0)))
        .child(facts(app, header))
        .child(actions(app, page));
    view()
        .flex_none()
        .border_b(px(1.0))
        .border_color(color(Token::Rule))
        .child(first)
        .child(dynamic(move || {
            line2.get().map(|words| {
                view()
                    .px(px(18.0))
                    .pb(px(8.0))
                    .child(
                        text(words)
                            .number_of_lines(1u32)
                            .truncate()
                            .font_family(FONT_TEXT)
                            .text_size(px(12.0))
                            .text_color(color(Token::Ink3)),
                    )
                    .into_element()
            })
        }))
        .into_element()
}

/// The phone's state line under the shell's head: the state word (or a current notice), the
/// needs-you count and the session's id.
pub(super) fn phone_line(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let header = computed(move || page.with(|p| p.header.clone()));
    view()
        .flex_row()
        .flex_none()
        .items_center()
        .gap(px(8.0))
        .px(px(16.0))
        .pt(px(6.0))
        .pb(px(4.0))
        .child(dynamic(move || {
            header.with(|h| untracked(|| state(app, h)))
        }))
        .child(dynamic(move || {
            header.with(|h| {
                h.needs_you
                    .as_ref()
                    .map(|needs| untracked(|| chip(app, needs.short.clone(), Hue::You)))
            })
        }))
        .child(view().flex_1().min_w(px(0.0)))
        .child(
            text(move || header.with(|h| h.shown_id.clone()))
                .number_of_lines(1u32)
                .flex_shrink_0()
                .font_family(FONT_MONO)
                .text_size(px(13.0))
                .text_color(color(Token::Ink3)),
        )
        .into_element()
}

/// The trailing facts: the needs-you chip, the decisions chip (pressing opens the task in Now),
/// then the state word.
fn facts(app: AppState, header: Memo<Header>) -> AnyElement {
    let form = app.form;
    view()
        .flex_row()
        .flex_shrink_0()
        .items_center()
        .gap(px(if form == Form::Terminal { 8.0 } else { 10.0 }))
        .child(dynamic(move || {
            header.with(|h| {
                h.needs_you
                    .as_ref()
                    .map(|needs| untracked(|| chip(app, needs.label.clone(), Hue::You)))
            })
        }))
        .child(dynamic(move || {
            header.with(|h| {
                h.decisions.clone().map(|decisions| {
                    untracked(|| {
                        let action = decisions.action.clone();
                        button_with(
                            app,
                            decisions.label,
                            ButtonKind::Ghost,
                            ButtonOptions::default(),
                            "btn-decisions",
                            move || run(app, action.clone()),
                        )
                    })
                })
            })
        }))
        .child(dynamic(move || {
            header.with(|h| untracked(|| state(app, h)))
        }))
        .into_element()
}

/// The state word with its mark, or the current notice in its place in its tone.
fn state(app: AppState, header: &Header) -> AnyElement {
    let form = app.form;
    let words = |content: String, hue: Hue| {
        let words = text(content)
            .test_id("session-state")
            .number_of_lines(1u32)
            .text_color(hue_words(form, hue));
        match form {
            Form::Terminal => words,
            Form::Desktop => words.font_family(FONT_TEXT).text_size(px(12.5)),
            Form::Phone => words.font_family(FONT_TEXT).text_size(px(13.5)),
        }
    };
    if let Some(notice) = &header.notice {
        return words(notice.text.clone(), notice.hue).into_element();
    }
    let word = &header.state;
    view()
        .flex_row()
        .items_center()
        .gap(px(if form == Form::Terminal { 8.0 } else { 5.0 }))
        .children(state_mark(app, word.spinner, word.glyph, word.hue))
        .child(words(word.words.clone(), word.hue))
        .into_element()
}

/// The ⋯ menu of the session's actions, every group's rows in order; a row the host cannot run
/// now stays listed, muted with its reason, and does not run. Its rows are read when it opens.
pub(super) fn actions(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    menu()
        .popup_style(popup(app.form))
        .backdrop_style(|s| s.bg(color(Token::Shell).opacity(0.5)))
        .on_open_change(counted(app))
        .trigger(menu_trigger(
            app,
            "⋯",
            "Session actions",
            "btn-session-actions",
        ))
        .child(dynamic(move || {
            untracked(|| {
                let rows: Vec<ActionRow> = page.with(|p| p.actions.rows().cloned().collect());
                view()
                    .children(
                        rows.into_iter()
                            .map(|row| item(app, row))
                            .collect::<Vec<_>>(),
                    )
                    .into_element()
            })
        }))
        .into_element()
}

/// A menu's surface for the form: a raised card on desktop, the sheet's top on the phone, a
/// framed box in a terminal.
pub(in crate::screens::agents) fn popup(
    form: Form,
) -> impl Fn(StyleRefinement) -> StyleRefinement + 'static {
    move |s| {
        let s = s
            .bg(color(Token::Surface))
            .border_1()
            .border_color(color(Token::Rule2));
        match form {
            Form::Desktop => s
                .min_w(px(MENU_MIN))
                .max_w(px(MENU_MAX))
                .py(px(4.0))
                .rounded(px(8.0))
                .shadow_lg()
                .shadow_color(color(Token::Shell)),
            Form::Phone => s.rounded_t(px(14.0)).pt(px(8.0)).pb(px(24.0)),
            Form::Terminal => s.min_w(term::cells(28.0)),
        }
    }
}

/// One action: its label with its key, and under it (beside it in a terminal) why it cannot run.
fn item(app: AppState, row: ActionRow) -> AnyElement {
    let form = app.form;
    let ActionRow {
        id,
        label,
        key,
        unavailable,
        action,
        ..
    } = row;
    let enabled = unavailable.is_none();
    let spoken = match &unavailable {
        Some(why) => format!("{label}, unavailable: {why}"),
        None => label.clone(),
    };
    let words = text(label)
        .number_of_lines(1u32)
        .truncate()
        .flex_1()
        .min_w(px(0.0))
        .text_color(color(if enabled { Token::Ink } else { Token::Ink3 }));
    let cap = key.filter(|_| form != Form::Phone).map(|key| {
        let cap = text(key)
            .number_of_lines(1u32)
            .text_color(color(Token::Ink3));
        if form == Form::Terminal {
            cap.into_element()
        } else {
            cap.font_family(FONT_MONO)
                .text_size(px(11.0))
                .into_element()
        }
    });
    let why = unavailable.map(|why| {
        let why = text(format!("unavailable: {why}"))
            .number_of_lines(2u32)
            .truncate()
            .text_color(color(Token::Ink3));
        match form {
            Form::Terminal => why.into_element(),
            Form::Desktop => why
                .font_family(FONT_TEXT)
                .text_size(px(11.5))
                .into_element(),
            Form::Phone => why
                .font_family(FONT_TEXT)
                .text_size(px(13.0))
                .into_element(),
        }
    });
    let line = view()
        .flex_row()
        .items_center()
        .gap(px(if form == Form::Terminal { 16.0 } else { 12.0 }))
        .child(match form {
            Form::Terminal => words,
            Form::Desktop => words.font_family(FONT_TEXT).text_size(px(12.5)),
            Form::Phone => words.font_family(FONT_TEXT).text_size(px(16.0)),
        })
        .children(cap);
    let content = match form {
        Form::Terminal => view()
            .px(term::cells(1.0))
            .child(line.h(term::rows(1.0)))
            .children(why),
        Form::Desktop => view()
            .gap(px(1.0))
            .px(px(12.0))
            .py(px(6.0))
            .child(line)
            .children(why),
        Form::Phone => view()
            .justify_center()
            .gap(px(2.0))
            .min_h(px(TARGET + 4.0))
            .px(px(18.0))
            .py(px(8.0))
            .child(line)
            .children(why),
    };
    menu_item()
        .accessibility_label(spoken)
        .disabled(!enabled)
        .on_select(move || run(app, action.clone()))
        .cursor_pointer()
        .hover(|s| s.bg(color(Token::Hover)))
        .focus(|s| s.bg(color(Token::Hover)))
        .child(content.test_id(&id))
        .into_element()
}
