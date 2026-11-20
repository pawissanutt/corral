//! The scope switcher: the one project scope of the whole app, always in view. Pressing it
//! opens a menu of All projects and each project with its open-task count; choosing one scopes
//! Home, Tasks, Sessions and Activity together (`AppState::set_scope`). It stands at the top of
//! the sidebar, in the desktop's title bar and in the phone's head.

use nsh_base::{menu, menu_item};
use nsh_core::element::StyleRefinement;
use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::projects::{SCOPE_HEADING, ScopeEntry, scope_menu, scope_note};
use crate::state::AppState;
use crate::ui::atoms::control_height;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::overlay::counted;
use crate::ui::term::{self, cells};
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color, row_height, text_size};

/// The phone's least touch target.
const TARGET: f32 = 44.0;
const MENU_MIN: f32 = 240.0;
const MENU_MAX: f32 = 340.0;

/// Where a switcher stands, which decides its face.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeFace {
    /// The top of the unfolded sidebar: the scope's name over what it covers. `scope-open`.
    Sidebar,
    /// The folded sidebar: the folder alone on desktop, the scope's first letters in a
    /// terminal. `scope-open`.
    Rail,
    /// The desktop's title bar and the phone's head: the folder and the scope's name on the
    /// dark shell. `scope-open-bar`.
    Bar,
}

/// The switcher with its menu.
pub fn scope_switcher(app: AppState, face: ScopeFace) -> AnyElement {
    let form = app.form;
    menu()
        .popup_style(move |s: StyleRefinement| {
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
                Form::Terminal => s.min_w(cells(28.0)),
            }
        })
        .backdrop_style(|s: StyleRefinement| s.bg(color(Token::Shell).opacity(0.5)))
        .on_open_change(counted(app))
        .trigger(trigger(app, face))
        .child(dynamic(move || untracked(move || entries(app))))
        .into_element()
}

/// Tracked: the scope's name.
fn label(app: AppState) -> SharedString {
    SharedString::from(app.project_scope().label())
}

fn trigger(app: AppState, face: ScopeFace) -> AnyElement {
    let form = app.form;
    let spoken = move || SharedString::from(format!("Scope: {}", app.project_scope().label()));
    let face_view = view()
        .test_id(match face {
            ScopeFace::Sidebar | ScopeFace::Rail => "scope-open",
            ScopeFace::Bar => "scope-open-bar",
        })
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(spoken);
    if form == Form::Terminal {
        let words = text(move || label(app))
            .truncate()
            .flex_1()
            .min_w(px(0.0))
            .font_weight(FontWeight::BOLD)
            .text_color(color(Token::Ink));
        return face_view
            .h(term::rows(1.0))
            .child(term::space(1.0))
            .child(words)
            .when(face == ScopeFace::Sidebar, |row| {
                row.child(term::quiet("▾")).child(term::space(1.0))
            })
            .into_element();
    }
    // The shell stays dark in both appearances, and so do its own inks.
    let ink = color(Token::ShellInk);
    let quiet = color(Token::ShellInk2);
    let name = text(move || label(app))
        .truncate()
        .min_w(px(0.0))
        .font_family(FONT_TEXT)
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(ink);
    let folder = icon(Icon::Project, 16.0, quiet);
    let hover = move |s: StyleRefinement| s.bg(ink.opacity(0.1));
    match face {
        ScopeFace::Sidebar => face_view
            .h(px(control_height(40.0)))
            .gap(px(10.0))
            .px(px(8.0))
            .mb(px(8.0))
            .rounded(px(6.0))
            .border(px(1.0))
            .border_color(ink.opacity(0.16))
            .hover(hover)
            .child(folder)
            .child(
                view()
                    .flex_1()
                    .min_w(px(0.0))
                    .child(name.text_size(px(text_size(form))))
                    .child(
                        text(move || {
                            SharedString::from(
                                app.feed.with(|feed| scope_note(feed, &app.project_scope())),
                            )
                        })
                        .truncate()
                        .font_family(FONT_TEXT)
                        .text_size(px(11.0))
                        .text_color(quiet),
                    ),
            )
            .child(
                view()
                    .rotate(deg(90.0))
                    .child(icon(Icon::Chevron, 12.0, quiet)),
            )
            .into_element(),
        ScopeFace::Rail => face_view
            .size(px(control_height(40.0)))
            .justify_center()
            .mb(px(8.0))
            .rounded(px(6.0))
            .border(px(1.0))
            .border_color(ink.opacity(0.16))
            .hover(hover)
            .child(icon(Icon::Project, 18.0, ink))
            .into_element(),
        ScopeFace::Bar => face_view
            .h(px(control_height(if form == Form::Phone {
                28.0
            } else {
                24.0
            })))
            .max_w(px(220.0))
            .gap(px(6.0))
            .px(px(8.0))
            .rounded(px(6.0))
            .bg(ink.opacity(0.08))
            .hover(hover)
            .child(folder)
            .child(name.text_size(px(text_size(Form::Desktop) - 1.0)))
            .into_element(),
    }
}

/// The menu's rows: its heading, All projects, then each project.
fn entries(app: AppState) -> AnyElement {
    let form = app.form;
    let scope = app.project_scope();
    let rows = app.feed.with_untracked(|feed| scope_menu(feed, &scope));
    let heading = text(SCOPE_HEADING)
        .number_of_lines(1u32)
        .text_color(color(Token::Ink3));
    let heading = match form {
        Form::Terminal => view().px(cells(1.0)).child(heading),
        Form::Desktop => view()
            .px(px(12.0))
            .pt(px(6.0))
            .pb(px(4.0))
            .child(heading.font_family(FONT_TEXT).text_size(px(11.0))),
        Form::Phone => view()
            .px(px(18.0))
            .pt(px(6.0))
            .pb(px(6.0))
            .child(heading.font_family(FONT_TEXT).text_size(px(13.0))),
    };
    view()
        .child(heading)
        .children(rows.into_iter().map(|entry| entry_row(app, entry)))
        .into_element()
}

/// One scope: its name, bold with a check while it is the scope, then what waits on the owner
/// there and how many tasks are open.
fn entry_row(app: AppState, entry: ScopeEntry) -> AnyElement {
    let form = app.form;
    let ScopeEntry {
        id,
        label,
        scope,
        note,
        needs,
        selected,
    } = entry;
    let mut spoken = label.clone();
    if let Some(needs) = needs {
        spoken.push_str(&format!(", {}", crate::model::health::waits_on_you(needs)));
    }
    if let Some(note) = &note {
        spoken.push_str(&format!(", {note}"));
    }
    if selected {
        spoken.push_str(", in scope");
    }
    let check = text(if selected { "✓" } else { " " })
        .flex_shrink_0()
        .text_color(color(Token::Ink))
        .accessibility_elements_hidden(true);
    let name = text(label)
        .truncate()
        .flex_1()
        .min_w(px(0.0))
        .text_color(color(Token::Ink))
        .when(selected, |name| name.font_weight(FontWeight::BOLD));
    let waiting = needs.map(|needs| {
        text(needs.to_string())
            .flex_shrink_0()
            .font_weight(FontWeight::BOLD)
            .text_color(color(Token::You))
    });
    let open = note.map(|note| text(note).flex_shrink_0().text_color(color(Token::Ink3)));
    let content = view().test_id(&id).flex_row().items_center();
    let content = match form {
        Form::Terminal => content
            .h(term::rows(1.0))
            .px(cells(1.0))
            .gap(cells(1.0))
            .child(check)
            .child(name)
            .children(waiting)
            .children(open),
        Form::Desktop => content
            .h(px(row_height(form)))
            .px(px(12.0))
            .gap(px(8.0))
            .child(check.font_family(FONT_TEXT).text_size(px(12.0)))
            .child(name.font_family(FONT_TEXT).text_size(px(12.5)))
            .children(waiting.map(|n| n.font_family(FONT_MONO).text_size(px(11.0))))
            .children(open.map(|n| n.font_family(FONT_MONO).text_size(px(11.0)))),
        Form::Phone => content
            .min_h(px(TARGET + 4.0))
            .px(px(18.0))
            .gap(px(10.0))
            .child(check.font_family(FONT_TEXT).text_size(px(16.0)))
            .child(name.font_family(FONT_TEXT).text_size(px(16.0)))
            .children(waiting.map(|n| n.font_family(FONT_MONO).text_size(px(13.0))))
            .children(open.map(|n| n.font_family(FONT_MONO).text_size(px(13.0)))),
    };
    menu_item()
        .accessibility_label(spoken)
        .on_select(move || app.set_scope(scope.clone()))
        .cursor_pointer()
        .hover(|s| s.bg(color(Token::Hover)))
        .focus(|s| s.bg(color(Token::Hover)))
        .child(content)
        .into_element()
}
