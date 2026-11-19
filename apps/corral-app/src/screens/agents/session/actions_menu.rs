//! The ⋯ menu of the session's actions, under the catalog's group headings; a row the host
//! cannot run now stays listed, muted with its reason, and does not run. Its rows are read when
//! it opens.
//!
//! The app draws it on every form rather than through NSH's `Menu`, which presents itself by the
//! platform's idiom, not the app's form: in a browser whose idiom is not the desktop's while the
//! app draws its desktop form it became a bottom sheet pinned to the window's corner, and on the
//! phone its sheet never showed its rows in the shell harness.
//!
//! Desktop and terminal: an overlay anchored to the ⋯ control, its trailing edge on the
//! control's, dropping below it (above when only that side has room), its body bounded by the
//! room the core reports on that side and scrolling. ↑ ↓ Home End walk the rows, Return and Space
//! press one, Esc or a press outside closes it and the focus returns to ⋯. Phone: a sheet along
//! the bottom edge over a dimmed page, its rows full touch targets; pressing the dimmed page or
//! back closes it. Test ids `btn-session-actions`, `session-actions`, `act-<slug>`.

use std::cell::Cell;
use std::rc::Rc;

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::session::{ActionGroup, ActionRow, SessionPage};
use crate::state::AppState;
use crate::ui::kit::walk::walk;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::super::super::run;

#[cfg(test)]
#[path = "actions_menu_tests.rs"]
mod tests;

/// The phone's least touch target.
const TARGET: f32 = 44.0;
/// The desktop menu's width range.
const MENU_MIN: f32 = 220.0;
const MENU_MAX: f32 = 340.0;
/// The ring around the rows: the panel's border and padding.
const RING: f32 = 10.0;

pub(in crate::screens::agents::session) fn actions(
    app: AppState,
    page: Memo<SessionPage>,
) -> AnyElement {
    let open = signal(false);
    let anchor = element_ref();
    held_open(app.menus, open);
    let panel = when(
        move || open.get(),
        move || untracked(|| panel(app, page, open, anchor)),
    );
    view()
        .flex_row()
        .flex_shrink_0()
        .child(trigger(app, open, anchor))
        .child(panel)
        .into_element()
}

/// Counts the open menu in `menus` (`AppState::menus`), as NSH's menus are counted: the key line
/// gives the menu's keys and the page's bare keys hold back while it is open. The count goes
/// back down when the menu closes, or once when its page goes while it is open.
///
/// Whether it is counted is a plain cell shared by the effect and the cleanup, not a `stored`
/// of the owner: NSH frees what an owner made (its signals, stored values and effects) before it
/// runs that owner's cleanups (`clean_node`), so a cleanup that reads a value of its own owner
/// panics as the page goes.
fn held_open(menus: Signal<u32>, open: Signal<bool>) {
    let held = Rc::new(Cell::new(false));
    let counted = held.clone();
    effect(move || {
        let now = open.get();
        if counted.replace(now) != now {
            untracked(|| menus.update(|n| *n = if now { *n + 1 } else { n.saturating_sub(1) }));
        }
    });
    cleanup(move || {
        if held.replace(false) {
            menus.update(|n| *n = n.saturating_sub(1));
        }
    });
}

/// `⋯`: a desktop outlined control 30 pt high, a terminal's band; pressing it opens or closes
/// the menu.
fn trigger(app: AppState, open: Signal<bool>, anchor: ElementRef) -> AnyElement {
    let form = app.form;
    let face = view()
        .test_id("btn-session-actions")
        .element_ref(&anchor)
        .role(AccessibilityRole::Button)
        .label("Session actions")
        .accessibility_state(move || AccessibilityState {
            expanded: Some(open.get()),
            has_popup: true,
            ..AccessibilityState::default()
        })
        .on_press(move || open.update(|open| *open = !*open))
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .cursor_pointer();
    let dots = text("⋯")
        .number_of_lines(1u32)
        .text_color(color(Token::Ink));
    match form {
        Form::Terminal => face
            .px(term::cells(1.0))
            .bg(color(Token::Surface))
            .child(dots)
            .child(term::space(1.0))
            .child(term::quiet("▾"))
            .into_element(),
        Form::Desktop | Form::Phone => face
            .h(px(if form == Form::Phone { TARGET } else { 30.0 }))
            .min_w(px(if form == Form::Phone { TARGET } else { 0.0 }))
            .justify_center()
            .px(px(12.0))
            .rounded(px(if form == Form::Phone { 10.0 } else { 6.0 }))
            .border_1()
            .border_color(color(Token::Rule2))
            .bg(move || {
                if open.get() {
                    color(Token::Surface2)
                } else {
                    AdaptiveColor::TRANSPARENT
                }
            })
            .hover(|s| s.bg(color(Token::Hover)))
            .active(|s| s.bg(color(Token::Sel)))
            .focus_visible(|s| {
                s.outline(px(2.0))
                    .outline_color(color(Token::Focus))
                    .outline_offset(px(2.0))
            })
            .child(
                dots.font_family(FONT_TEXT)
                    .text_size(px(14.0))
                    .font_weight(FontWeight::SEMIBOLD),
            )
            .into_element(),
    }
}

/// The open menu: anchored to ⋯, its rows walked by the arrows, the first one that can run
/// focused as it opens.
fn panel(
    app: AppState,
    page: Memo<SessionPage>,
    open: Signal<bool>,
    anchor: ElementRef,
) -> AnyElement {
    let form = app.form;
    let groups: Vec<ActionGroup> = page.with_untracked(|p| p.actions.groups.clone());
    let mut handles = Vec::new();
    let mut first = None;
    let mut children = Vec::new();
    for (at, group) in groups.into_iter().enumerate() {
        children.push(heading(form, group.title, at == 0));
        for found in group.rows {
            let handle = element_ref();
            if first.is_none() && found.unavailable.is_none() {
                first = Some(handle);
            }
            handles.push(handle);
            children.push(entry(app, found, open, anchor, handle));
        }
    }
    if let Some(first) = first.or_else(|| handles.first().copied()) {
        after_answer(move || first.focus());
    }
    walk(handles);
    let close = move || {
        open.set(false);
        after_answer(move || anchor.focus());
    };
    if form == Form::Phone {
        return phone_sheet(open, children, close);
    }
    // The room the core leaves on the side it placed the menu; until it says, the window's
    // under the head.
    let room = signal(None::<Points>);
    let fallback = move || match form {
        Form::Terminal => term::rows(term::grid_rows().saturating_sub(8).max(4) as f32).0,
        _ => (window().height.0 - 140.0).max(160.0),
    };
    let body = scroll()
        .max_h(move || {
            px(room
                .get()
                .map_or_else(fallback, |room| (room.0 - RING).max(80.0)))
        })
        .child(view().children(children));
    let surface = view()
        .test_id("session-actions")
        .role(AccessibilityRole::Menu)
        .label("Session actions")
        .bg(color(Token::Surface))
        .border_1()
        .border_color(color(Token::Rule2));
    let surface = match form {
        Form::Terminal => surface.min_w(term::cells(28.0)),
        Form::Desktop | Form::Phone => surface
            .min_w(px(MENU_MIN))
            .max_w(px(MENU_MAX))
            .py(px(4.0))
            .rounded(px(8.0))
            .shadow_lg()
            .shadow_color(color(Token::Shell)),
    };
    overlay()
        .anchor(&anchor)
        .placement(Placement::Bottom)
        .align(Align::End)
        .offset(px(4.0))
        .margin(px(8.0))
        .on_position(move |p: &OverlayPosition| room.set(Some(p.available.height)))
        .on_press_outside(move |_: &Point<Points>| open.set(false))
        .on_cancel(close)
        .child(surface.child(body))
        .into_element()
}

/// The phone's: a sheet along the bottom edge over a dimmed page; pressing the dimmed page, back
/// or Esc closes it.
fn phone_sheet(
    open: Signal<bool>,
    children: Vec<AnyElement>,
    close: impl Fn() + 'static,
) -> AnyElement {
    let tall = move || (window().height.0 * 0.85).max(240.0);
    let backdrop = overlay()
        .inset_0()
        .focusable(false)
        .bg(color(Token::Shell).opacity(0.5))
        .on_press(move || open.set(false));
    let sheet = overlay()
        .left_0()
        .right_0()
        .bottom(px(0.0))
        .max_h(move || px(tall()))
        .test_id("session-actions")
        .role(AccessibilityRole::Menu)
        .label("Session actions")
        .bg(color(Token::Surface))
        .border_t(px(1.0))
        .border_color(color(Token::Rule2))
        .rounded_t(px(14.0))
        .pt(px(8.0))
        .pb(px(24.0))
        .on_cancel(close)
        .child(
            scroll()
                .max_h(move || px(tall() - 32.0))
                .child(view().children(children)),
        );
    view().child(backdrop).child(sheet).into_element()
}

/// A group's heading: spaced faint capitals on desktop and phone (as the palette's), bold quiet
/// words in a terminal.
fn heading(form: Form, title: &'static str, first: bool) -> AnyElement {
    let words = text(title).number_of_lines(1u32);
    let line = view().role(AccessibilityRole::Header).label(title);
    match form {
        Form::Terminal => line
            .px(term::cells(1.0))
            .child(
                words
                    .font_weight(FontWeight::BOLD)
                    .text_color(color(Token::Ink3)),
            )
            .into_element(),
        Form::Desktop | Form::Phone => line
            .px(px(if form == Form::Phone { 18.0 } else { 12.0 }))
            .pt(px(if first { 4.0 } else { 10.0 }))
            .pb(px(3.0))
            .child(
                words
                    .uppercase()
                    .font_family(FONT_TEXT)
                    .text_size(px(if form == Form::Phone { 11.5 } else { 10.5 }))
                    .letter_spacing(px(0.8))
                    .text_color(color(Token::Ink4)),
            )
            .into_element(),
    }
}

/// A row's words: its label with its key, and under it (beside it in a terminal) why it cannot
/// run; what screen readers say for it.
fn content(form: Form, row: &ActionRow) -> (View, String) {
    let enabled = row.unavailable.is_none();
    let spoken = match &row.unavailable {
        Some(why) => format!("{}, unavailable: {why}", row.label),
        None => row.label.clone(),
    };
    let words = text(row.label.clone())
        .number_of_lines(1u32)
        .truncate()
        .flex_1()
        .min_w(px(0.0))
        .text_color(color(if enabled { Token::Ink } else { Token::Ink3 }));
    let cap = row.key.filter(|_| form != Form::Phone).map(|key| {
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
    let why = row.unavailable.clone().map(|why| {
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
    let view = match form {
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
    (view, spoken)
}

/// One row of the anchored menu: pressing one that can run closes the menu and runs it.
fn entry(
    app: AppState,
    row: ActionRow,
    open: Signal<bool>,
    anchor: ElementRef,
    handle: ElementRef,
) -> AnyElement {
    let (words, spoken) = content(app.form, &row);
    let enabled = row.unavailable.is_none();
    let action = row.action;
    words
        .test_id(&row.id)
        .element_ref(&handle)
        .role(AccessibilityRole::Menuitem)
        .label(spoken)
        .accessible(true)
        .focusable(true)
        .accessibility_state(AccessibilityState {
            disabled: !enabled,
            ..AccessibilityState::default()
        })
        .hover(|s| s.bg(color(Token::Hover)))
        .focus(|s| s.bg(color(Token::Hover)))
        .when(enabled, |v| {
            v.cursor_pointer().on_press(move || {
                open.set(false);
                after_answer(move || anchor.focus());
                run(app, action.clone());
            })
        })
        .into_element()
}
