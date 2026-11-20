//! The session page's head (`session::Header`): back to Sessions, the session's id, name and
//! role word, the needs-you and decisions chips, the state word with its mark (a current notice
//! in its place, with × to dismiss it), Details, More (the session's controls, as on its row)
//! and the ⋯ menu of everything its host offers; under it the meta line (project, branch, task,
//! model and effort, what it does). Pressing the needs-you chip docks the oldest waiting card; a
//! build's `choose exit` presses to Finish…, which docks the exit choice. Desktop: one 46 pt row
//! and the line under it; terminal: the same as two rows of Cells; phone: the shell's head names
//! the session and holds ⋯, the page leads with one state line with Details. The facts redraw on
//! their own, so the open menu and its trigger stay mounted while the state word ticks. test ids
//! `btn-needs-you`, `btn-choose-exit`, `btn-dismiss-notice`, `btn-details`, `more-<session>`,
//! `session-meta`, `meta-task`.

use corral_protocol::session_state::ExitPhase;
use nsh_core::element::StyleRefinement;
use nsh_core::prelude::*;

use crate::model::session::{Action, CommandAction, DockAction, Header, Meta, SessionPage};
use crate::model::work::Record;
use crate::model::{Form, Hue, Id};
use crate::state::AppState;
use crate::ui::icons::Icon;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::chip::chip;
use crate::ui::kit::next::more_button;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::super::run;
use super::{hue_words, state_mark};

#[path = "actions_menu.rs"]
mod actions_menu;

pub(super) use actions_menu::actions;

/// The phone's least touch target.
const TARGET: f32 = 44.0;
/// The desktop menu's width range.
const MENU_MIN: f32 = 220.0;
const MENU_MAX: f32 = 340.0;

/// `Details`, which opens and closes the panel, while the session has one.
fn details_button(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let toggle =
        computed(move || page.with(|p| p.details.as_ref().map(|d| (d.toggle.clone(), d.shown))));
    dynamic(move || {
        toggle.get().map(|(action, shown)| {
            untracked(|| {
                button_with(
                    app,
                    "Details",
                    // Outlined while open, as a pressed toggle reads.
                    if shown {
                        ButtonKind::Ghost
                    } else {
                        ButtonKind::Quiet
                    },
                    ButtonOptions {
                        // A terminal's key line names Alt+B; its head keeps the room.
                        key_hint: (app.form == Form::Desktop).then_some("Alt+B"),
                        ..ButtonOptions::default()
                    },
                    "btn-details",
                    move || run(app, action.clone()),
                )
            })
        })
    })
}

/// The session's More menu: its controls, then for a run its task's.
fn more(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let id = untracked(|| page.with(|p| p.id.clone()));
    more_button(app, Record::Session(id), app.form == Form::Phone)
}

/// The meta line under the head: project, branch, task (opening its page), model and effort,
/// what the session does.
fn meta(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let form = app.form;
    let facts = computed(move || page.with(|p| p.header.meta.clone()));
    dynamic(move || {
        let facts = facts.get();
        (!facts.is_empty()).then(|| {
            untracked(|| {
                let parts: Vec<AnyElement> =
                    facts.into_iter().map(|fact| meta_fact(app, fact)).collect();
                let line = view()
                    .test_id("session-meta")
                    .flex_row()
                    .items_center()
                    .overflow_hidden()
                    .gap(px(if form == Form::Terminal { 16.0 } else { 14.0 }))
                    .children(parts);
                match form {
                    Form::Terminal => line.px(term::cells(1.0)).into_element(),
                    _ => line.px(px(18.0)).pb(px(8.0)).into_element(),
                }
            })
        })
    })
}

fn meta_fact(app: AppState, fact: Meta) -> AnyElement {
    let form = app.form;
    let sized = |words: Text, mono: bool| match form {
        Form::Terminal => words,
        _ => words
            .font_family(if mono { FONT_MONO } else { FONT_TEXT })
            .text_size(px(12.0)),
    };
    let value = sized(
        text(fact.value.clone())
            .number_of_lines(1u32)
            .truncate()
            .min_w(px(0.0))
            .text_color(color(if fact.label.is_some() {
                Token::Ink2
            } else {
                Token::Ink3
            })),
        fact.mono,
    );
    let face = view()
        .flex_row()
        .items_center()
        .min_w(px(0.0))
        .flex_shrink(if fact.label.is_some() { 0.0 } else { 1.0 })
        .gap(px(if form == Form::Terminal { 8.0 } else { 4.0 }))
        .children(fact.label.map(|label| {
            sized(
                text(label)
                    .number_of_lines(1u32)
                    .text_color(color(Token::Ink3)),
                false,
            )
        }))
        .child(value);
    match fact.action {
        Some(action) => face
            .test_id(&format!("meta-{}", fact.label.unwrap_or("link")))
            .role(AccessibilityRole::Link)
            .label(format!("{} {}", fact.label.unwrap_or_default(), fact.value))
            .cursor_pointer()
            .on_press(move || run(app, action.clone()))
            .into_element(),
        None => face.into_element(),
    }
}

/// The page head on desktop and in a terminal: back to Sessions, the session's id, name and
/// role, its state, Details, More and the session's actions; the meta line under it.
pub(super) fn head(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let form = app.form;
    let header = computed(move || page.with(|p| p.header.clone()));
    let back = untracked(|| header.with(|h| h.back.clone()));
    let name = move || header.with(|h| h.name.clone());
    let role = move || header.with(|h| h.role.clone());
    let shown_id = move || header.with(|h| h.shown_id.clone());
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
                text(shown_id)
                    .number_of_lines(1u32)
                    .flex_shrink_0()
                    .text_color(color(Token::Ink3)),
            )
            .child(term::space(1.0))
            .child(
                text(name)
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
                text(role)
                    .number_of_lines(1u32)
                    .flex_shrink_0()
                    .text_color(color(Token::Ink3)),
            )
            .child(view().flex_1().min_w(px(0.0)))
            .child(facts(app, page, header))
            .child(term::space(1.0))
            .child(details_button(app, page))
            .child(more(app, page))
            .child(actions(app, page));
        return view()
            .flex_none()
            .child(first)
            .child(meta(app, page))
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
            text(shown_id)
                .number_of_lines(1u32)
                .flex_shrink_0()
                .font_family(FONT_MONO)
                .text_size(px(12.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color(Token::Ink2)),
        )
        .child(
            text(name)
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
            text(role)
                .number_of_lines(1u32)
                .flex_shrink_0()
                .font_family(FONT_TEXT)
                .text_size(px(13.0))
                .text_color(color(Token::Ink3)),
        )
        .child(view().flex_1().min_w(px(0.0)))
        .child(facts(app, page, header))
        .child(details_button(app, page))
        .child(more(app, page))
        .child(actions(app, page));
    view()
        .flex_none()
        .border_b(px(1.0))
        .border_color(color(Token::Rule))
        .child(first)
        .child(meta(app, page))
        .into_element()
}

/// The phone's state line under the shell's head: the state word (or a current notice), the
/// needs-you count and the session's id.
pub(super) fn phone_line(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let header = computed(move || page.with(|p| p.header.clone()));
    let session = untracked(|| page.with(|p| p.id.clone()));
    view()
        .flex_row()
        .flex_none()
        .items_center()
        .gap(px(8.0))
        .px(px(16.0))
        .pt(px(6.0))
        .pb(px(4.0))
        .child(dynamic(move || {
            header.with(|h| untracked(|| state(app, &session, h, None)))
        }))
        .child(needs_you(app, page, true))
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

/// The phone head's trailing controls: Details, which opens as a sheet, and ⋯.
pub(super) fn phone_trailing(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    view()
        .flex_row()
        .items_center()
        .gap(px(4.0))
        .child(details_button(app, page))
        .child(actions(app, page))
        .into_element()
}

/// The needs-you chip (`▲ 2 needs you`, `▲ 2` when `short`); pressing it docks the oldest card
/// waiting on the owner.
fn needs_you(app: AppState, page: Memo<SessionPage>, short: bool) -> AnyElement {
    let shown = computed(move || {
        page.with(|p| {
            let needs = p.header.needs_you.as_ref()?;
            let label = if short {
                needs.short.clone()
            } else {
                needs.label.clone()
            };
            Some((label, p.dock.oldest.clone()))
        })
    });
    dynamic(move || {
        let (label, oldest) = shown.get()?;
        Some(untracked(|| {
            let face = chip(app, label.clone(), Hue::You);
            match oldest {
                Some(action) => view()
                    .test_id("btn-needs-you")
                    .role(AccessibilityRole::Button)
                    .label(format!("{label}: show the oldest"))
                    .cursor_pointer()
                    .on_press(move || run(app, action.clone()))
                    .child(face)
                    .into_element(),
                None => face,
            }
        }))
    })
}

/// Finish…, while the state word says a build's exit choice waits and it is not docked.
fn choose_exit(page: Memo<SessionPage>) -> Memo<Option<Action>> {
    computed(move || {
        page.with(|p| {
            let pending = p
                .header
                .state
                .exit
                .as_ref()
                .is_some_and(|exit| exit.phase == ExitPhase::Pending);
            (pending && p.dock.choice.is_none())
                .then(|| Action::Dock(DockAction::Finish(p.id.clone())))
        })
    })
}

/// The trailing facts: the needs-you chip, the decisions chip (pressing opens the task in Now),
/// then the state word.
fn facts(app: AppState, page: Memo<SessionPage>, header: Memo<Header>) -> AnyElement {
    let form = app.form;
    let finish = choose_exit(page);
    let session = untracked(|| page.with(|p| p.id.clone()));
    view()
        .flex_row()
        .flex_shrink_0()
        .items_center()
        .gap(px(if form == Form::Terminal { 8.0 } else { 10.0 }))
        .child(needs_you(app, page, false))
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
            let finish = finish.get();
            header.with(|h| untracked(|| state(app, &session, h, finish)))
        }))
        .into_element()
}

/// The state word with its mark, or the current notice in its place in its tone with × to
/// dismiss one of this device's (a refusal stays until then); pressable as `finish` while a
/// build's exit choice waits undocked.
fn state(app: AppState, session: &Id, header: &Header, finish: Option<Action>) -> AnyElement {
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
        let line = words(notice.text.clone(), notice.hue);
        if !notice.id.starts_with("notice-") {
            return line.into_element();
        }
        let dismiss = Action::Command(CommandAction::DismissNotice {
            session: session.clone(),
            notice: notice.id.clone(),
        });
        let close = view()
            .test_id("btn-dismiss-notice")
            .role(AccessibilityRole::Button)
            .label("Dismiss notice")
            .cursor_pointer()
            .on_press(move || run(app, dismiss.clone()))
            .child(
                text("×")
                    .number_of_lines(1u32)
                    .text_color(color(Token::Ink3)),
            );
        let close = match form {
            Form::Terminal => close,
            Form::Desktop => close
                .px(px(4.0))
                .rounded(px(4.0))
                .hover(|s| s.bg(color(Token::Hover))),
            Form::Phone => close
                .min_w(px(TARGET))
                .min_h(px(TARGET))
                .items_center()
                .justify_center(),
        };
        return view()
            .flex_row()
            .items_center()
            .min_w(px(0.0))
            .gap(px(if form == Form::Terminal { 8.0 } else { 4.0 }))
            .child(line.flex_shrink(1.0).min_w(px(0.0)).truncate())
            .child(close)
            .into_element();
    }
    let word = &header.state;
    let face = view()
        .flex_row()
        .items_center()
        .gap(px(if form == Form::Terminal { 8.0 } else { 5.0 }))
        .children(state_mark(app, word.spinner, word.glyph, word.hue))
        .child(words(word.words.clone(), word.hue));
    match finish {
        Some(action) => face
            .test_id("btn-choose-exit")
            .role(AccessibilityRole::Button)
            .label(format!("{}: Finish…", word.words))
            .cursor_pointer()
            .on_press(move || run(app, action.clone()))
            .into_element(),
        None => face.into_element(),
    }
}

/// The surface of an NSH menu for the form: a raised card on desktop, the sheet's top on the
/// phone, a framed box in a terminal. NSH presents a menu as a sheet wherever the platform's
/// idiom is not the desktop's, also where the app draws its desktop form (a large tablet): the
/// sheet's look then, not a card's width pinned to the window's corner.
pub(in crate::screens::agents) fn popup(
    form: Form,
) -> impl Fn(StyleRefinement) -> StyleRefinement + 'static {
    let sheet = form == Form::Phone || (form == Form::Desktop && !is_desktop());
    move |s| {
        let s = s
            .bg(color(Token::Surface))
            .border_1()
            .border_color(color(Token::Rule2));
        match form {
            _ if sheet => s.rounded_t(px(14.0)).pt(px(8.0)).pb(px(24.0)),
            Form::Desktop | Form::Phone => s
                .min_w(px(MENU_MIN))
                .max_w(px(MENU_MAX))
                .py(px(4.0))
                .rounded(px(8.0))
                .shadow_lg()
                .shadow_color(color(Token::Shell)),
            Form::Terminal => s.min_w(term::cells(28.0)),
        }
    }
}
